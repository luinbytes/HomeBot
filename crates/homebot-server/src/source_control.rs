//! Authenticated, server-owned source-control surfaces and remote-operation approvals.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use homebot_domain::chat::{ApprovalStatus as DomainApprovalStatus, ChatApproval};
use homebot_protocol::{
    ApprovalSummary, CreatePullRequestRequest, PullRequestMetadata, PullRequestMutationResponse,
    ServerEventBody, VcsCommitRequest, VcsCommitResult, VcsCreateBranchRequest, VcsMutationStatus,
    VcsPushRequest, VcsRemoteMutationResponse, VcsStatus, WorkingTreeDiffResponse,
};
use homebot_storage::{IdempotencyClaim, StorageError, VcsOperationResultRecord};
use homebot_tools::{CapabilityClass, CapabilityRequest, OperationContext, ToolError};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use uuid::Uuid;

use crate::{
    AppState,
    bots::{ApiError, claim},
    unix_time_ms,
};

#[derive(Deserialize)]
pub(super) struct DiffQuery {
    #[serde(default)]
    staged: bool,
}

#[derive(Deserialize)]
pub(super) struct PullRequestQuery {
    remote: String,
    head_branch: String,
    base_branch: String,
}

pub(super) async fn status(
    State(state): State<AppState>,
    Path(chat_id): Path<Uuid>,
) -> Result<Json<VcsStatus>, ApiError> {
    let (_, path) = super::checkpoints::workspace_path(&state, chat_id).await?;
    Ok(Json(
        runtime(&state)?
            .source_status(path.as_ref())
            .await
            .map_err(|error| vcs_error(&error))?,
    ))
}

pub(super) async fn diff(
    State(state): State<AppState>,
    Path(chat_id): Path<Uuid>,
    Query(query): Query<DiffQuery>,
) -> Result<Json<WorkingTreeDiffResponse>, ApiError> {
    let (_, path) = super::checkpoints::workspace_path(&state, chat_id).await?;
    let diff = runtime(&state)?
        .working_diff(path.as_ref(), query.staged)
        .await
        .map_err(|error| vcs_error(&error))?;
    Ok(Json(WorkingTreeDiffResponse {
        staged: query.staged,
        patch: diff.patch,
        files: diff.files,
    }))
}

pub(super) async fn commit(
    State(state): State<AppState>,
    Path(chat_id): Path<Uuid>,
    Json(request): Json<VcsCommitRequest>,
) -> Result<(StatusCode, Json<VcsCommitResult>), ApiError> {
    let (_, path) = super::checkpoints::workspace_path(&state, chat_id).await?;
    let replayed =
        claim_mutation(&state, chat_id, request.idempotency_key, "commit", &request).await?;
    if replayed {
        return Ok((
            StatusCode::OK,
            Json(replay_result(&state, chat_id, request.idempotency_key, "commit").await?),
        ));
    }
    let result = runtime(&state)?
        .commit(path.as_ref(), &request.message, request.stage_all)
        .await
        .map_err(|error| vcs_error(&error))?;
    record_result(&state, chat_id, request.idempotency_key, "commit", &result).await?;
    publish_status(&state, chat_id, path.as_ref()).await?;
    Ok((StatusCode::CREATED, Json(result)))
}

pub(super) async fn create_branch(
    State(state): State<AppState>,
    Path(chat_id): Path<Uuid>,
    Json(request): Json<VcsCreateBranchRequest>,
) -> Result<(StatusCode, Json<VcsStatus>), ApiError> {
    let (_, path) = super::checkpoints::workspace_path(&state, chat_id).await?;
    let replayed = claim_mutation(
        &state,
        chat_id,
        request.idempotency_key,
        "create_branch",
        &request,
    )
    .await?;
    if replayed {
        return Ok((
            StatusCode::OK,
            Json(replay_result(&state, chat_id, request.idempotency_key, "create_branch").await?),
        ));
    }
    runtime(&state)?
        .create_branch(
            path.as_ref(),
            &request.branch,
            request.start_point.as_deref(),
        )
        .await
        .map_err(|error| vcs_error(&error))?;
    let result = runtime(&state)?
        .source_status(path.as_ref())
        .await
        .map_err(|error| vcs_error(&error))?;
    record_result(
        &state,
        chat_id,
        request.idempotency_key,
        "create_branch",
        &result,
    )
    .await?;
    publish(&state, chat_id, result.clone()).await?;
    Ok((StatusCode::CREATED, Json(result)))
}

pub(super) async fn push(
    State(state): State<AppState>,
    Path(chat_id): Path<Uuid>,
    Json(request): Json<VcsPushRequest>,
) -> Result<(StatusCode, Json<VcsRemoteMutationResponse>), ApiError> {
    let (workspace, path) = super::checkpoints::workspace_path(&state, chat_id).await?;
    if state
        .storage
        .vcs_operation_result(state.owner_id, chat_id, request.idempotency_key, "push")
        .await?
        .is_some()
    {
        let replayed =
            claim_mutation(&state, chat_id, request.idempotency_key, "push", &request).await?;
        if !replayed {
            return Err(ApiError::internal());
        }
        return Ok((
            StatusCode::OK,
            Json(replay_result(&state, chat_id, request.idempotency_key, "push").await?),
        ));
    }
    let chat = state
        .storage
        .get_direct_chat(state.owner_id, chat_id)
        .await?;
    let capability = CapabilityRequest {
        context: OperationContext {
            operation_id: request.request_id,
            owner_id: state.owner_id,
            device_id: Uuid::nil(),
            bot_id: chat.bot_id,
            chat_id,
            workspace_id: workspace.workspace_id,
        },
        capability: CapabilityClass::GitRemote,
        action: "git.push".to_owned(),
        canonical_resource: format!(
            "workspace:{}:remote:{}:branch:{}",
            workspace.workspace_id, request.remote, request.branch
        ),
        summary: format!(
            "Push branch {} to remote {}",
            request.branch, request.remote
        ),
        destructive: true,
    };
    match state
        .policy_engine
        .authorize(&capability, request.approval_id)
        .await
    {
        Ok(_authorization) => {}
        Err(ToolError::ApprovalRequired(ticket)) => {
            let approval = persist_approval(
                &state,
                chat_id,
                &ticket,
                "homebot.git.remote",
                "Approve Git push",
            )
            .await?;
            return Ok((
                StatusCode::ACCEPTED,
                Json(VcsRemoteMutationResponse {
                    status: VcsMutationStatus::ApprovalRequired,
                    approval: Some(approval),
                    result: None,
                }),
            ));
        }
        Err(error) => return Err(policy_error(&error)),
    }
    let replayed =
        claim_mutation(&state, chat_id, request.idempotency_key, "push", &request).await?;
    if replayed {
        return Ok((
            StatusCode::OK,
            Json(replay_result(&state, chat_id, request.idempotency_key, "push").await?),
        ));
    }
    let result = runtime(&state)?
        .push(
            path.as_ref(),
            &request.remote,
            &request.branch,
            request.set_upstream,
        )
        .await
        .map_err(|error| vcs_error(&error))?;
    let response = VcsRemoteMutationResponse {
        status: VcsMutationStatus::Completed,
        approval: None,
        result: Some(result),
    };
    record_result(&state, chat_id, request.idempotency_key, "push", &response).await?;
    publish_status(&state, chat_id, path.as_ref()).await?;
    Ok((StatusCode::CREATED, Json(response)))
}

pub(super) async fn pull_request_metadata(
    State(state): State<AppState>,
    Path(chat_id): Path<Uuid>,
    Query(query): Query<PullRequestQuery>,
) -> Result<Json<PullRequestMetadata>, ApiError> {
    let (_, path) = super::checkpoints::workspace_path(&state, chat_id).await?;
    Ok(Json(
        runtime(&state)?
            .pull_request_metadata(
                path.as_ref(),
                &query.remote,
                &query.head_branch,
                &query.base_branch,
            )
            .await
            .map_err(|error| vcs_error(&error))?,
    ))
}

#[allow(clippy::too_many_lines)]
pub(super) async fn create_pull_request(
    State(state): State<AppState>,
    Path(chat_id): Path<Uuid>,
    Json(request): Json<CreatePullRequestRequest>,
) -> Result<(StatusCode, Json<PullRequestMutationResponse>), ApiError> {
    let (workspace, path) = super::checkpoints::workspace_path(&state, chat_id).await?;
    if state
        .storage
        .vcs_operation_result(
            state.owner_id,
            chat_id,
            request.idempotency_key,
            "pull_request",
        )
        .await?
        .is_some()
    {
        let replayed = claim_mutation(
            &state,
            chat_id,
            request.idempotency_key,
            "pull_request",
            &request,
        )
        .await?;
        if !replayed {
            return Err(ApiError::internal());
        }
        return Ok((
            StatusCode::OK,
            Json(replay_result(&state, chat_id, request.idempotency_key, "pull_request").await?),
        ));
    }
    let chat = state
        .storage
        .get_direct_chat(state.owner_id, chat_id)
        .await?;
    let capability = CapabilityRequest {
        context: OperationContext {
            operation_id: request.request_id,
            owner_id: state.owner_id,
            device_id: Uuid::nil(),
            bot_id: chat.bot_id,
            chat_id,
            workspace_id: workspace.workspace_id,
        },
        capability: CapabilityClass::ExternalMutation,
        action: "git.pull_request.create".to_owned(),
        canonical_resource: format!(
            "workspace:{}:remote:{}:head:{}:base:{}",
            workspace.workspace_id, request.remote, request.head_branch, request.base_branch
        ),
        summary: format!(
            "Create pull request from {} into {}",
            request.head_branch, request.base_branch
        ),
        destructive: false,
    };
    match state
        .policy_engine
        .authorize(&capability, request.approval_id)
        .await
    {
        Ok(_authorization) => {}
        Err(ToolError::ApprovalRequired(ticket)) => {
            let approval = persist_approval(
                &state,
                chat_id,
                &ticket,
                "homebot.git.pull_request",
                "Approve pull request creation",
            )
            .await?;
            return Ok((
                StatusCode::ACCEPTED,
                Json(PullRequestMutationResponse {
                    status: VcsMutationStatus::ApprovalRequired,
                    approval: Some(approval),
                    result: None,
                }),
            ));
        }
        Err(error) => return Err(policy_error(&error)),
    }
    if claim_mutation(
        &state,
        chat_id,
        request.idempotency_key,
        "pull_request",
        &request,
    )
    .await?
    {
        return Err(ApiError::internal());
    }
    let result = runtime(&state)?
        .create_pull_request(
            path.as_ref(),
            &request.remote,
            &request.head_branch,
            &request.base_branch,
            &request.title,
            &request.body,
            request.draft,
        )
        .await
        .map_err(|error| vcs_error(&error))?;
    let response = PullRequestMutationResponse {
        status: VcsMutationStatus::Completed,
        approval: None,
        result: Some(result),
    };
    record_result(
        &state,
        chat_id,
        request.idempotency_key,
        "pull_request",
        &response,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(response)))
}

async fn persist_approval(
    state: &AppState,
    chat_id: Uuid,
    ticket: &homebot_tools::ApprovalTicket,
    capability: &str,
    title: &str,
) -> Result<ApprovalSummary, ApiError> {
    match state
        .storage
        .chat_approval(state.owner_id, ticket.approval_id)
        .await
    {
        Ok(existing) => return Ok(super::chats::approval_summary(existing)),
        Err(StorageError::ApprovalNotFound) => {}
        Err(error) => return Err(error.into()),
    }
    let record = ChatApproval {
        id: ticket.approval_id,
        owner_id: state.owner_id,
        chat_id,
        message_id: None,
        operation_id: ticket.operation_id,
        capability: capability.to_owned(),
        title: title.to_owned(),
        detail: ticket.summary.clone(),
        status: DomainApprovalStatus::Pending,
        created_at_ms: unix_time_ms(),
        decided_at_ms: None,
    };
    state.storage.create_chat_approval(&record).await?;
    let approval = super::chats::approval_summary(record);
    super::chats::publish(
        state,
        "approval_changed",
        ServerEventBody::ApprovalChanged {
            approval: approval.clone(),
        },
    )
    .await?;
    Ok(approval)
}

async fn claim_mutation<T: Serialize>(
    state: &AppState,
    chat_id: Uuid,
    idempotency_key: Uuid,
    action: &str,
    request: &T,
) -> Result<bool, ApiError> {
    Ok(matches!(
        claim(
            state,
            idempotency_key,
            &format!("vcs:{action}:{chat_id}"),
            request
        )
        .await?,
        IdempotencyClaim::Replayed { .. }
    ))
}

async fn record_result<T: Serialize>(
    state: &AppState,
    chat_id: Uuid,
    idempotency_key: Uuid,
    action: &str,
    response: &T,
) -> Result<(), ApiError> {
    state
        .storage
        .record_vcs_operation_result(&VcsOperationResultRecord {
            idempotency_key,
            owner_id: state.owner_id,
            chat_id,
            action: action.to_owned(),
            response: serde_json::to_value(response).map_err(|_| ApiError::internal())?,
            created_at_ms: unix_time_ms(),
        })
        .await?;
    Ok(())
}

async fn replay_result<T: DeserializeOwned>(
    state: &AppState,
    chat_id: Uuid,
    idempotency_key: Uuid,
    action: &str,
) -> Result<T, ApiError> {
    let record = state
        .storage
        .vcs_operation_result(state.owner_id, chat_id, idempotency_key, action)
        .await?
        .ok_or_else(|| {
            ApiError::conflict(
                "The prior Git operation has an indeterminate result; refresh status",
            )
        })?;
    serde_json::from_value(record.response).map_err(|_| ApiError::internal())
}

async fn publish_status(
    state: &AppState,
    chat_id: Uuid,
    path: &std::path::Path,
) -> Result<(), ApiError> {
    let status = runtime(state)?
        .source_status(path)
        .await
        .map_err(|error| vcs_error(&error))?;
    publish(state, chat_id, status).await
}

async fn publish(state: &AppState, chat_id: Uuid, status: VcsStatus) -> Result<(), ApiError> {
    super::chats::publish(
        state,
        "vcs_status_changed",
        ServerEventBody::VcsStatusChanged { chat_id, status },
    )
    .await
}

fn runtime(state: &AppState) -> Result<std::sync::Arc<homebot_vcs::GitRuntime>, ApiError> {
    state
        .git_runtime
        .clone()
        .ok_or_else(|| ApiError::unavailable("Git is not installed"))
}

fn vcs_error(error: &homebot_vcs::VcsError) -> ApiError {
    super::workspaces::vcs_error(error)
}

fn policy_error(error: &ToolError) -> ApiError {
    match error {
        ToolError::Denied => ApiError::conflict("Git remote operation was denied"),
        ToolError::InvalidApproval => {
            ApiError::conflict("Git remote approval is invalid or no longer usable")
        }
        _ => ApiError::internal(),
    }
}
