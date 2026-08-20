//! Authenticated repository registration and per-chat isolated Git worktrees.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use homebot_protocol::{
    AttachChatWorkspaceRequest, ChatWorkspaceSummary, CreateRepositoryWorkspaceRequest,
    DetachChatWorkspaceRequest, RepositoryWorkspaceSummary, ServerEventBody, WorkingTreeCondition,
    WorkspaceBranchesResponse, WorkspaceMode,
};
use homebot_storage::{ChatWorkspaceRecord, IdempotencyClaim, RepositoryWorkspaceRecord};
use homebot_vcs::{GitRuntime, VcsError};
use std::sync::Arc;
use uuid::Uuid;

use crate::{
    AppState,
    bots::{ApiError, claim},
    persist_event, unix_time_ms,
};

pub(super) async fn list(
    State(state): State<AppState>,
) -> Result<Json<Vec<RepositoryWorkspaceSummary>>, ApiError> {
    Ok(Json(repository_summaries(&state).await?))
}

pub(super) async fn create(
    State(state): State<AppState>,
    Json(request): Json<CreateRepositoryWorkspaceRequest>,
) -> Result<(StatusCode, Json<RepositoryWorkspaceSummary>), ApiError> {
    let replayed = matches!(
        claim(
            &state,
            request.idempotency_key,
            "create_repository_workspace",
            &request,
        )
        .await?,
        IdempotencyClaim::Replayed { .. }
    );
    if replayed {
        let record = state
            .storage
            .repository_workspace(state.owner_id, request.idempotency_key)
            .await?;
        return Ok((
            StatusCode::OK,
            Json(repository_summary(&state, &record).await),
        ));
    }
    let runtime = runtime(&state)?;
    let inspection = runtime
        .inspect_repository(std::path::Path::new(&request.root_path))
        .await
        .map_err(|error| vcs_error(&error))?;
    let root_path = inspection
        .root
        .to_str()
        .ok_or_else(|| ApiError::validation("Repository path is not valid UTF-8"))?
        .to_owned();
    let now = unix_time_ms();
    let record = RepositoryWorkspaceRecord {
        id: request.idempotency_key,
        owner_id: state.owner_id,
        name: request
            .name
            .as_deref()
            .map(|value| visible(value, 80, "Workspace name"))
            .transpose()?
            .unwrap_or(inspection.display_name),
        root_path,
        created_at_ms: now,
        updated_at_ms: now,
    };
    state.storage.create_repository_workspace(&record).await?;
    let workspace = repository_summary(&state, &record).await;
    publish(
        &state,
        "repository_workspace_changed",
        ServerEventBody::RepositoryWorkspaceChanged {
            workspace: workspace.clone(),
        },
    )
    .await?;
    Ok((StatusCode::CREATED, Json(workspace)))
}

pub(super) async fn branches(
    State(state): State<AppState>,
    Path(workspace_id): Path<Uuid>,
) -> Result<Json<WorkspaceBranchesResponse>, ApiError> {
    let workspace = state
        .storage
        .repository_workspace(state.owner_id, workspace_id)
        .await?;
    Ok(Json(WorkspaceBranchesResponse {
        branches: runtime(&state)?
            .branches(std::path::Path::new(&workspace.root_path))
            .await
            .map_err(|error| vcs_error(&error))?,
    }))
}

pub(super) async fn chat(
    State(state): State<AppState>,
    Path(chat_id): Path<Uuid>,
) -> Result<Json<ChatWorkspaceSummary>, ApiError> {
    let record = state
        .storage
        .chat_workspace(state.owner_id, chat_id)
        .await?
        .ok_or(homebot_storage::StorageError::WorkspaceNotFound)?;
    Ok(Json(chat_summary(&state, &record).await?))
}

pub(super) async fn attach(
    State(state): State<AppState>,
    Path(chat_id): Path<Uuid>,
    Json(request): Json<AttachChatWorkspaceRequest>,
) -> Result<(StatusCode, Json<ChatWorkspaceSummary>), ApiError> {
    let replayed = matches!(
        claim(
            &state,
            request.idempotency_key,
            &format!("attach_chat_workspace:{chat_id}"),
            &request,
        )
        .await?,
        IdempotencyClaim::Replayed { .. }
    );
    if replayed {
        let existing = state
            .storage
            .chat_workspace(state.owner_id, chat_id)
            .await?
            .ok_or(homebot_storage::StorageError::WorkspaceNotFound)?;
        return Ok((StatusCode::OK, Json(chat_summary(&state, &existing).await?)));
    }
    if state
        .storage
        .chat_workspace(state.owner_id, chat_id)
        .await?
        .is_some()
    {
        return Err(ApiError::conflict("This chat already has a workspace"));
    }
    let workspace = state
        .storage
        .repository_workspace(state.owner_id, request.workspace_id)
        .await?;
    let runtime = runtime(&state)?;
    let inspection = runtime
        .inspect_repository(std::path::Path::new(&workspace.root_path))
        .await
        .map_err(|error| vcs_error(&error))?;
    let (worktree_path, branch_name, base_ref) =
        association_paths(&state, &runtime, chat_id, &inspection, &request).await?;
    let now = unix_time_ms();
    let record = ChatWorkspaceRecord {
        owner_id: state.owner_id,
        chat_id,
        workspace_id: workspace.id,
        mode: request.mode,
        worktree_path,
        branch_name,
        base_ref,
        created_at_ms: now,
        updated_at_ms: now,
    };
    if let Err(error) = state.storage.attach_chat_workspace(&record).await {
        if let Some(path) = &record.worktree_path {
            let _ = runtime
                .remove_worktree(
                    std::path::Path::new(&workspace.root_path),
                    &state.worktree_root,
                    std::path::Path::new(path),
                )
                .await;
        }
        return Err(error.into());
    }
    let summary = chat_summary(&state, &record).await?;
    publish(
        &state,
        "chat_workspace_changed",
        ServerEventBody::ChatWorkspaceChanged {
            workspace: summary.clone(),
        },
    )
    .await?;
    Ok((StatusCode::CREATED, Json(summary)))
}

pub(super) async fn detach(
    State(state): State<AppState>,
    Path(chat_id): Path<Uuid>,
    Json(request): Json<DetachChatWorkspaceRequest>,
) -> Result<StatusCode, ApiError> {
    let replayed = matches!(
        claim(
            &state,
            request.idempotency_key,
            &format!("detach_chat_workspace:{chat_id}"),
            &request,
        )
        .await?,
        IdempotencyClaim::Replayed { .. }
    );
    let Some(record) = state
        .storage
        .chat_workspace(state.owner_id, chat_id)
        .await?
    else {
        return if replayed {
            Ok(StatusCode::NO_CONTENT)
        } else {
            Err(homebot_storage::StorageError::WorkspaceNotFound.into())
        };
    };
    if record.mode == WorkspaceMode::Isolated {
        let workspace = state
            .storage
            .repository_workspace(state.owner_id, record.workspace_id)
            .await?;
        runtime(&state)?
            .remove_worktree(
                std::path::Path::new(&workspace.root_path),
                &state.worktree_root,
                std::path::Path::new(
                    record
                        .worktree_path
                        .as_deref()
                        .ok_or_else(ApiError::internal)?,
                ),
            )
            .await
            .map_err(|error| vcs_error(&error))?;
    }
    state
        .storage
        .detach_chat_workspace(state.owner_id, chat_id)
        .await?;
    publish(
        &state,
        "chat_workspace_removed",
        ServerEventBody::ChatWorkspaceRemoved { chat_id },
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn repository_summaries(
    state: &AppState,
) -> Result<Vec<RepositoryWorkspaceSummary>, ApiError> {
    let records = state
        .storage
        .list_repository_workspaces(state.owner_id)
        .await?;
    let mut summaries = Vec::with_capacity(records.len());
    for record in records {
        summaries.push(repository_summary(state, &record).await);
    }
    Ok(summaries)
}

pub(super) async fn chat_summaries(
    state: &AppState,
) -> Result<Vec<ChatWorkspaceSummary>, ApiError> {
    let records = state.storage.list_chat_workspaces(state.owner_id).await?;
    let mut summaries = Vec::with_capacity(records.len());
    for record in records {
        summaries.push(chat_summary(state, &record).await?);
    }
    Ok(summaries)
}

async fn repository_summary(
    state: &AppState,
    record: &RepositoryWorkspaceRecord,
) -> RepositoryWorkspaceSummary {
    let inspected = match &state.git_runtime {
        Some(runtime) => runtime
            .inspect_repository(std::path::Path::new(&record.root_path))
            .await
            .ok(),
        None => None,
    };
    RepositoryWorkspaceSummary {
        id: record.id,
        name: record.name.clone(),
        root_path: record.root_path.clone(),
        current_branch: inspected
            .as_ref()
            .and_then(|inspection| inspection.branch.clone()),
        condition: inspected.map_or(WorkingTreeCondition::Unavailable, |inspection| {
            inspection.condition
        }),
        created_at_unix_ms: u64::try_from(record.created_at_ms).unwrap_or_default(),
        updated_at_unix_ms: u64::try_from(record.updated_at_ms).unwrap_or_default(),
    }
}

async fn chat_summary(
    state: &AppState,
    record: &ChatWorkspaceRecord,
) -> Result<ChatWorkspaceSummary, ApiError> {
    let repository = state
        .storage
        .repository_workspace(state.owner_id, record.workspace_id)
        .await?;
    let path = record
        .worktree_path
        .as_deref()
        .unwrap_or(&repository.root_path);
    let condition = match &state.git_runtime {
        Some(runtime) => runtime
            .inspect_repository(std::path::Path::new(path))
            .await
            .map_or(WorkingTreeCondition::Unavailable, |inspection| {
                inspection.condition
            }),
        None => WorkingTreeCondition::Unavailable,
    };
    Ok(ChatWorkspaceSummary {
        chat_id: record.chat_id,
        workspace_id: record.workspace_id,
        mode: record.mode,
        effective_path: path.to_owned(),
        branch_name: record.branch_name.clone(),
        base_ref: record.base_ref.clone(),
        condition,
        updated_at_unix_ms: u64::try_from(record.updated_at_ms).unwrap_or_default(),
    })
}

fn runtime(state: &AppState) -> Result<Arc<GitRuntime>, ApiError> {
    state
        .git_runtime
        .clone()
        .ok_or_else(|| ApiError::unavailable("Git is not installed"))
}

async fn association_paths(
    state: &AppState,
    runtime: &GitRuntime,
    chat_id: Uuid,
    inspection: &homebot_vcs::RepositoryInspection,
    request: &AttachChatWorkspaceRequest,
) -> Result<(Option<String>, Option<String>, Option<String>), ApiError> {
    if request.mode == WorkspaceMode::Primary {
        return Ok((None, inspection.branch.clone(), None));
    }
    let base = request
        .base_ref
        .as_deref()
        .or(inspection.branch.as_deref())
        .unwrap_or("HEAD");
    let worktree = runtime
        .create_worktree(
            &inspection.root,
            &state.worktree_root,
            chat_id,
            base,
            request.branch_name.as_deref(),
        )
        .await
        .map_err(|error| vcs_error(&error))?;
    let path = worktree
        .path
        .to_str()
        .ok_or_else(|| ApiError::validation("Worktree path is not valid UTF-8"))?
        .to_owned();
    Ok((Some(path), Some(worktree.branch), Some(base.to_owned())))
}

pub(super) fn vcs_error(error: &VcsError) -> ApiError {
    match error {
        VcsError::GitUnavailable => ApiError::unavailable("Git is not installed"),
        VcsError::InvalidPath | VcsError::NotRepository => ApiError::validation(&error.to_string()),
        VcsError::DirtyWorktree | VcsError::RestoreConflict | VcsError::UnsafeWorktreePath => {
            ApiError::conflict(&error.to_string())
        }
        VcsError::Git(_) => ApiError::conflict(&error.to_string()),
        VcsError::Timeout | VcsError::OutputLimit | VcsError::Io(_) => ApiError::internal(),
    }
}

async fn publish(state: &AppState, kind: &str, body: ServerEventBody) -> Result<(), ApiError> {
    persist_event(state, kind, body)
        .await
        .map(|_| ())
        .map_err(|()| ApiError::internal())
}

fn visible(value: &str, max: usize, label: &str) -> Result<String, ApiError> {
    let value = value.trim();
    if value.is_empty() || value.len() > max || value.chars().any(char::is_control) {
        return Err(ApiError::validation(&format!("{label} is invalid")));
    }
    Ok(value.to_owned())
}
