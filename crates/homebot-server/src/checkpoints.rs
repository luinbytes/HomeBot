//! Server-owned turn checkpoints, exact diffs, and guarded restore.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use homebot_protocol::{
    CheckpointDiffResponse, CheckpointPhase, CheckpointRestoreSummary, ConversationReconciliation,
    RestoreCheckpointRequest, ServerEventBody, TurnCheckpointSummary,
};
use homebot_storage::{CheckpointRestoreRecord, IdempotencyClaim, TurnCheckpointRecord};
use serde::Deserialize;
use uuid::Uuid;

use crate::{
    AppState,
    bots::{ApiError, claim},
    persist_event, unix_time_ms,
};

#[derive(Deserialize)]
pub(super) struct DiffQuery {
    from_checkpoint_id: Uuid,
    to_checkpoint_id: Uuid,
}

pub(super) async fn list(
    State(state): State<AppState>,
    Path(chat_id): Path<Uuid>,
) -> Result<Json<Vec<TurnCheckpointSummary>>, ApiError> {
    let _ = state
        .storage
        .get_direct_chat(state.owner_id, chat_id)
        .await?;
    Ok(Json(
        state
            .storage
            .turn_checkpoints(state.owner_id, chat_id)
            .await?
            .into_iter()
            .map(|checkpoint| summary(&checkpoint))
            .collect(),
    ))
}

pub(super) async fn diff(
    State(state): State<AppState>,
    Path(chat_id): Path<Uuid>,
    Query(query): Query<DiffQuery>,
) -> Result<Json<CheckpointDiffResponse>, ApiError> {
    let from = state
        .storage
        .turn_checkpoint(state.owner_id, query.from_checkpoint_id)
        .await?;
    let to = state
        .storage
        .turn_checkpoint(state.owner_id, query.to_checkpoint_id)
        .await?;
    validate_pair(chat_id, &from, &to)?;
    let (_, effective_path) = workspace_path(&state, chat_id).await?;
    let exact = state
        .git_runtime
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("Git is not installed"))?
        .checkpoint_diff(
            std::path::Path::new(&effective_path),
            &from.commit_oid,
            &to.commit_oid,
        )
        .await
        .map_err(|error| super::workspaces::vcs_error(&error))?;
    Ok(Json(CheckpointDiffResponse {
        from_checkpoint_id: from.id,
        to_checkpoint_id: to.id,
        patch: exact.patch,
        files: exact.files,
    }))
}

pub(super) async fn full_diff(
    State(state): State<AppState>,
    Path(chat_id): Path<Uuid>,
) -> Result<Json<CheckpointDiffResponse>, ApiError> {
    let checkpoints = state
        .storage
        .turn_checkpoints(state.owner_id, chat_id)
        .await?;
    let from = checkpoints
        .iter()
        .find(|checkpoint| checkpoint.phase == CheckpointPhase::BeforeTurn)
        .ok_or(homebot_storage::StorageError::CheckpointNotFound)?;
    let to = checkpoints
        .iter()
        .rev()
        .find(|checkpoint| checkpoint.phase == CheckpointPhase::AfterTurn)
        .ok_or(homebot_storage::StorageError::CheckpointNotFound)?;
    let query = DiffQuery {
        from_checkpoint_id: from.id,
        to_checkpoint_id: to.id,
    };
    diff(State(state), Path(chat_id), Query(query)).await
}

pub(super) async fn restore(
    State(state): State<AppState>,
    Path(checkpoint_id): Path<Uuid>,
    Json(request): Json<RestoreCheckpointRequest>,
) -> Result<(StatusCode, Json<CheckpointRestoreSummary>), ApiError> {
    let replayed = matches!(
        claim(
            &state,
            request.idempotency_key,
            &format!("restore_checkpoint:{checkpoint_id}"),
            &request,
        )
        .await?,
        IdempotencyClaim::Replayed { .. }
    );
    if replayed {
        let restore = state
            .storage
            .checkpoint_restore(state.owner_id, request.idempotency_key)
            .await?;
        return Ok((StatusCode::OK, Json(restore_summary(&restore))));
    }
    restore_new(&state, checkpoint_id, request.idempotency_key).await
}

async fn restore_new(
    state: &AppState,
    checkpoint_id: Uuid,
    restore_id: Uuid,
) -> Result<(StatusCode, Json<CheckpointRestoreSummary>), ApiError> {
    let target = state
        .storage
        .turn_checkpoint(state.owner_id, checkpoint_id)
        .await?;
    let chat = state
        .storage
        .get_direct_chat(state.owner_id, target.chat_id)
        .await?;
    if chat.running {
        return Err(ApiError::conflict(
            "Stop the Bot before restoring a checkpoint",
        ));
    }
    let (workspace, effective_path) = workspace_path(state, target.chat_id).await?;
    if workspace.workspace_id != target.workspace_id {
        return Err(ApiError::conflict(
            "The chat workspace changed after this checkpoint",
        ));
    }
    let safety_id = Uuid::now_v7();
    let restored = state
        .git_runtime
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("Git is not installed"))?
        .restore_checkpoint(
            std::path::Path::new(&effective_path),
            &checkpoint_metadata_root(state),
            target.chat_id,
            &target.commit_oid,
            safety_id,
        )
        .await
        .map_err(|error| super::workspaces::vcs_error(&error))?;
    let previous_conversation = match target.provider_profile_id {
        Some(profile_id) => {
            state
                .storage
                .provider_conversation(chat.bot_id, chat.id, profile_id)
                .await?
        }
        None => None,
    };
    let reconciliation = if previous_conversation.is_some() {
        ConversationReconciliation::Forked
    } else {
        ConversationReconciliation::Unchanged
    };
    let now = unix_time_ms();
    let safety = TurnCheckpointRecord {
        id: safety_id,
        owner_id: state.owner_id,
        chat_id: target.chat_id,
        workspace_id: target.workspace_id,
        message_id: None,
        phase: CheckpointPhase::RestoreSafety,
        git_ref: restored.safety_checkpoint.git_ref,
        commit_oid: restored.safety_checkpoint.commit_oid,
        provider_profile_id: target.provider_profile_id,
        provider_conversation_id: previous_conversation.clone(),
        created_at_ms: now,
    };
    let restore = CheckpointRestoreRecord {
        id: restore_id,
        owner_id: state.owner_id,
        chat_id: target.chat_id,
        checkpoint_id: target.id,
        safety_checkpoint_id: safety.id,
        reconciliation,
        previous_provider_conversation_id: previous_conversation,
        created_at_ms: now,
    };
    state
        .storage
        .record_checkpoint_restore(&safety, &restore, chat.bot_id)
        .await?;
    publish(
        state,
        "turn_checkpoint_changed",
        ServerEventBody::TurnCheckpointChanged {
            checkpoint: summary(&safety),
        },
    )
    .await?;
    let response = restore_summary(&restore);
    publish(
        state,
        "checkpoint_restored",
        ServerEventBody::CheckpointRestored {
            restore: response.clone(),
        },
    )
    .await?;
    Ok((StatusCode::CREATED, Json(response)))
}

pub(super) async fn capture_for_turn(
    state: &AppState,
    chat_id: Uuid,
    message_id: Uuid,
    profile_id: Uuid,
    conversation_id: Option<String>,
    phase: CheckpointPhase,
) -> Result<Option<Uuid>, ApiError> {
    let Some(workspace) = state
        .storage
        .chat_workspace(state.owner_id, chat_id)
        .await?
    else {
        return Ok(None);
    };
    let (_, effective_path) = workspace_path(state, chat_id).await?;
    let checkpoint_id = Uuid::now_v7();
    let capture = state
        .git_runtime
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("Git is not installed"))?
        .capture_checkpoint(
            std::path::Path::new(&effective_path),
            &checkpoint_metadata_root(state),
            chat_id,
            checkpoint_id,
        )
        .await
        .map_err(|error| super::workspaces::vcs_error(&error))?;
    let record = TurnCheckpointRecord {
        id: checkpoint_id,
        owner_id: state.owner_id,
        chat_id,
        workspace_id: workspace.workspace_id,
        message_id: Some(message_id),
        phase,
        git_ref: capture.git_ref,
        commit_oid: capture.commit_oid,
        provider_profile_id: Some(profile_id),
        provider_conversation_id: conversation_id,
        created_at_ms: unix_time_ms(),
    };
    state.storage.create_turn_checkpoint(&record).await?;
    publish(
        state,
        "turn_checkpoint_changed",
        ServerEventBody::TurnCheckpointChanged {
            checkpoint: summary(&record),
        },
    )
    .await?;
    Ok(Some(checkpoint_id))
}

pub(super) fn summary(record: &TurnCheckpointRecord) -> TurnCheckpointSummary {
    TurnCheckpointSummary {
        id: record.id,
        chat_id: record.chat_id,
        workspace_id: record.workspace_id,
        message_id: record.message_id,
        phase: record.phase,
        created_at_unix_ms: u64::try_from(record.created_at_ms).unwrap_or_default(),
    }
}

fn restore_summary(record: &CheckpointRestoreRecord) -> CheckpointRestoreSummary {
    CheckpointRestoreSummary {
        id: record.id,
        chat_id: record.chat_id,
        checkpoint_id: record.checkpoint_id,
        safety_checkpoint_id: record.safety_checkpoint_id,
        reconciliation: record.reconciliation,
        created_at_unix_ms: u64::try_from(record.created_at_ms).unwrap_or_default(),
    }
}

fn validate_pair(
    chat_id: Uuid,
    from: &TurnCheckpointRecord,
    to: &TurnCheckpointRecord,
) -> Result<(), ApiError> {
    if from.chat_id != chat_id || to.chat_id != chat_id || from.workspace_id != to.workspace_id {
        return Err(ApiError::validation(
            "Checkpoints must belong to this chat and workspace",
        ));
    }
    Ok(())
}

async fn workspace_path(
    state: &AppState,
    chat_id: Uuid,
) -> Result<(homebot_storage::ChatWorkspaceRecord, String), ApiError> {
    let workspace = state
        .storage
        .chat_workspace(state.owner_id, chat_id)
        .await?
        .ok_or(homebot_storage::StorageError::WorkspaceNotFound)?;
    let repository = state
        .storage
        .repository_workspace(state.owner_id, workspace.workspace_id)
        .await?;
    let path = workspace
        .worktree_path
        .clone()
        .unwrap_or(repository.root_path);
    Ok((workspace, path))
}

fn checkpoint_metadata_root(state: &AppState) -> std::path::PathBuf {
    state.worktree_root.join(".checkpoint-indexes")
}

async fn publish(state: &AppState, kind: &str, body: ServerEventBody) -> Result<(), ApiError> {
    persist_event(state, kind, body)
        .await
        .map(|_| ())
        .map_err(|()| ApiError::internal())
}
