use async_trait::async_trait;
use axum::{
    Json,
    extract::{Extension, Path, Query, State},
    http::StatusCode,
};
use homebot_domain::chat::{ActivityStatus, ExecutionActivity};
use homebot_protocol::{
    ActivityDetail, ActivityPresentation, ApprovalSummary, BrowserActionRequest,
    BrowserActionResponse, BrowserCommand, BrowserMutationRequest, BrowserSessionSummary,
    CreateBrowserSessionRequest, ServerEventBody,
};
use homebot_storage::{BrowserProfileRecord, BrowserSessionRecord, IdempotencyClaim};
use homebot_tools::{
    BrowserAction, BrowserResult, BrowserService, BrowserSessionProfile, OperationContext,
    ToolError,
};
use serde::Deserialize;
use std::{path::PathBuf, sync::Arc};
use uuid::Uuid;

use crate::{
    AppState, AuthenticatedIdentity,
    artifacts::{GeneratedArtifact, persist_generated_artifact},
    bots::{ApiError, claim},
    chats::publish,
    source_control::persist_approval,
    unix_time_ms,
};

const TAKEOVER_LEASE_MS: i64 = 5 * 60 * 1_000;

#[async_trait]
pub trait BrowserRuntime: Send + Sync {
    async fn create(
        &self,
        context: OperationContext,
        profile: &BrowserSessionProfile,
        approval_id: Option<Uuid>,
    ) -> Result<Uuid, ToolError>;
    async fn execute(
        &self,
        context: OperationContext,
        session_id: Uuid,
        action: BrowserAction,
        approval_id: Option<Uuid>,
    ) -> Result<BrowserResult, ToolError>;
    async fn close(
        &self,
        context: OperationContext,
        session_id: Uuid,
        approval_id: Option<Uuid>,
    ) -> Result<(), ToolError>;
}

#[async_trait]
impl BrowserRuntime for BrowserService {
    async fn create(
        &self,
        context: OperationContext,
        profile: &BrowserSessionProfile,
        approval_id: Option<Uuid>,
    ) -> Result<Uuid, ToolError> {
        self.ensure_profile_directory(profile)?;
        match self.create_session(context, profile, approval_id).await? {
            BrowserResult::SessionCreated { session_id } => Ok(session_id),
            _ => Err(ToolError::BrowserProtocol),
        }
    }

    async fn execute(
        &self,
        context: OperationContext,
        session_id: Uuid,
        action: BrowserAction,
        approval_id: Option<Uuid>,
    ) -> Result<BrowserResult, ToolError> {
        BrowserService::execute(self, context, session_id, action, approval_id).await
    }

    async fn close(
        &self,
        context: OperationContext,
        session_id: Uuid,
        approval_id: Option<Uuid>,
    ) -> Result<(), ToolError> {
        match self.close_session(context, session_id, approval_id).await? {
            BrowserResult::SessionClosed => Ok(()),
            _ => Err(ToolError::BrowserProtocol),
        }
    }
}

#[derive(Deserialize)]
pub(super) struct BrowserListQuery {
    chat_id: Option<Uuid>,
}

pub(super) async fn list(
    State(state): State<AppState>,
    Query(query): Query<BrowserListQuery>,
) -> Result<Json<Vec<BrowserSessionSummary>>, ApiError> {
    Ok(Json(
        state
            .storage
            .browser_sessions(state.owner_id, query.chat_id)
            .await?
            .into_iter()
            .map(summary)
            .collect::<Result<_, _>>()?,
    ))
}

pub(super) async fn get(
    State(state): State<AppState>,
    Path(session_id): Path<Uuid>,
) -> Result<Json<BrowserSessionSummary>, ApiError> {
    Ok(Json(summary(
        state
            .storage
            .browser_session(state.owner_id, session_id)
            .await?,
    )?))
}

pub(super) async fn create(
    State(state): State<AppState>,
    Extension(identity): Extension<AuthenticatedIdentity>,
    Json(request): Json<CreateBrowserSessionRequest>,
) -> Result<(StatusCode, Json<BrowserActionResponse>), ApiError> {
    let mut canonical = request.clone();
    canonical.approval_id = None;
    if matches!(
        claim(
            &state,
            request.idempotency_key,
            "create_browser_session",
            &canonical,
        )
        .await?,
        IdempotencyClaim::Replayed { .. }
    ) {
        let existing = state
            .storage
            .browser_sessions(state.owner_id, None)
            .await?
            .into_iter()
            .find(|session| session.id == request.idempotency_key)
            .ok_or_else(|| ApiError::conflict("Browser session creation is still in progress"))?;
        return Ok((
            StatusCode::OK,
            Json(response(summary(existing)?, None, None)),
        ));
    }
    let now = unix_time_ms();
    let profile = BrowserProfileRecord {
        id: request.profile_id,
        owner_id: state.owner_id,
        display_name: request.profile_name.trim().to_owned(),
        directory_ref: format!("profile-{}", request.profile_id),
        created_at_ms: now,
        updated_at_ms: now,
    };
    let profile = state.storage.upsert_browser_profile(&profile).await?;
    let context = operation_context(&state, &identity, &request, Uuid::nil());
    state.ensure_policy_loaded().await?;
    let runtime = runtime(&state)?;
    match runtime
        .create(
            context,
            &BrowserSessionProfile {
                profile_id: profile.id,
                display_name: profile.display_name.clone(),
                profile_directory: PathBuf::from(&profile.directory_ref),
            },
            request.approval_id,
        )
        .await
    {
        Ok(runtime_session_id) => Ok((
            StatusCode::CREATED,
            Json(
                activate_created_session(&state, &request, profile, runtime_session_id, now)
                    .await?,
            ),
        )),
        Err(ToolError::ApprovalRequired(ticket)) => {
            state
                .storage
                .release_idempotency(request.idempotency_key)
                .await?;
            let approval = persist_approval(
                &state,
                request.chat_id,
                &ticket,
                "homebot.browser.session",
                "Approve browser session",
            )
            .await?;
            let record = pending_session(&state, &request, &profile, approval.id, now).await?;
            let session = summary(record)?;
            publish_session(&state, session.clone()).await?;
            Ok((
                StatusCode::ACCEPTED,
                Json(response(session, Some(approval), None)),
            ))
        }
        Err(error) => Err(tool_error(&error)),
    }
}

pub(super) async fn takeover(
    State(state): State<AppState>,
    Extension(identity): Extension<AuthenticatedIdentity>,
    Path(session_id): Path<Uuid>,
    Json(request): Json<BrowserMutationRequest>,
) -> Result<(StatusCode, Json<BrowserActionResponse>), ApiError> {
    let current = state
        .storage
        .browser_session(state.owner_id, session_id)
        .await?;
    enforce_takeover_claimable(&identity, &current)?;
    let mut canonical = request.clone();
    canonical.approval_id = None;
    if matches!(
        claim(
            &state,
            request.idempotency_key,
            &format!("browser_takeover:{session_id}"),
            &canonical,
        )
        .await?,
        IdempotencyClaim::Replayed { .. }
    ) {
        return Ok((
            StatusCode::OK,
            Json(response(summary(current)?, None, None)),
        ));
    }
    state.ensure_policy_loaded().await?;
    let capability = homebot_tools::CapabilityRequest {
        context: context_for(&state, &identity, &current, request.request_id),
        capability: homebot_tools::CapabilityClass::BrowserAct,
        action: "browser.takeover".to_owned(),
        canonical_resource: format!("browser-session:{session_id}"),
        summary: "Take control of the live browser".to_owned(),
        destructive: true,
    };
    match state
        .policy_engine
        .authorize(&capability, request.approval_id)
        .await
    {
        Ok(_authorization) => {
            let now = unix_time_ms();
            let record = match state
                .storage
                .claim_browser_takeover(
                    state.owner_id,
                    session_id,
                    device_id(&identity),
                    now.saturating_add(TAKEOVER_LEASE_MS),
                    now,
                )
                .await
            {
                Ok(record) => record,
                Err(error) => {
                    state
                        .storage
                        .release_idempotency(request.idempotency_key)
                        .await?;
                    return Err(error.into());
                }
            };
            let session = summary(record)?;
            publish_session(&state, session.clone()).await?;
            Ok((StatusCode::OK, Json(response(session, None, None))))
        }
        Err(ToolError::ApprovalRequired(ticket)) => {
            state
                .storage
                .release_idempotency(request.idempotency_key)
                .await?;
            let approval = persist_approval(
                &state,
                current.chat_id,
                &ticket,
                "homebot.browser.takeover",
                "Approve browser takeover",
            )
            .await?;
            let record = state
                .storage
                .update_browser_session(
                    state.owner_id,
                    session_id,
                    &current.controller,
                    "awaiting_approval",
                    None,
                    Some(approval.id),
                    unix_time_ms(),
                )
                .await?;
            let session = summary(record)?;
            publish_session(&state, session.clone()).await?;
            Ok((
                StatusCode::ACCEPTED,
                Json(response(session, Some(approval), None)),
            ))
        }
        Err(error) => Err(tool_error(&error)),
    }
}

pub(super) async fn return_to_bot(
    State(state): State<AppState>,
    Extension(identity): Extension<AuthenticatedIdentity>,
    Path(session_id): Path<Uuid>,
    Json(request): Json<BrowserMutationRequest>,
) -> Result<Json<BrowserActionResponse>, ApiError> {
    let current = state
        .storage
        .browser_session(state.owner_id, session_id)
        .await?;
    enforce_takeover_owner(&identity, &current)?;
    let replayed = matches!(
        claim(
            &state,
            request.idempotency_key,
            &format!("browser_return:{session_id}"),
            &request,
        )
        .await?,
        IdempotencyClaim::Replayed { .. }
    );
    if replayed {
        return Ok(Json(response(
            summary(
                state
                    .storage
                    .browser_session(state.owner_id, session_id)
                    .await?,
            )?,
            None,
            None,
        )));
    }
    let record = state
        .storage
        .release_browser_takeover(
            state.owner_id,
            session_id,
            device_id(&identity),
            unix_time_ms(),
        )
        .await?;
    let session = summary(record)?;
    publish_session(&state, session.clone()).await?;
    Ok(Json(response(session, None, None)))
}

pub(super) async fn execute(
    State(state): State<AppState>,
    Extension(identity): Extension<AuthenticatedIdentity>,
    Path(session_id): Path<Uuid>,
    Json(request): Json<BrowserActionRequest>,
) -> Result<(StatusCode, Json<BrowserActionResponse>), ApiError> {
    let current = state
        .storage
        .browser_session(state.owner_id, session_id)
        .await?;
    enforce_action_controller(&identity, &current)?;
    if current.status == "closed" || current.runtime_session_id.is_none() {
        return Err(ApiError::conflict("The browser session is not active"));
    }
    let mut canonical = request.clone();
    canonical.approval_id = None;
    if matches!(
        claim(
            &state,
            request.idempotency_key,
            &format!("browser_action:{session_id}"),
            &canonical,
        )
        .await?,
        IdempotencyClaim::Replayed { .. }
    ) {
        return Ok((
            StatusCode::OK,
            Json(response(summary(current)?, None, None)),
        ));
    }
    let action = protocol_action(&request.command);
    state.ensure_policy_loaded().await?;
    let result = runtime(&state)?
        .execute(
            context_for(&state, &identity, &current, request.request_id),
            current.runtime_session_id.unwrap_or_default(),
            action,
            request.approval_id,
        )
        .await;
    match result {
        Ok(result) => Ok((
            StatusCode::OK,
            Json(complete_action(&state, current, result).await?),
        )),
        Err(ToolError::ApprovalRequired(ticket)) => {
            state
                .storage
                .release_idempotency(request.idempotency_key)
                .await?;
            let approval = persist_approval(
                &state,
                current.chat_id,
                &ticket,
                "homebot.browser.action",
                "Approve browser action",
            )
            .await?;
            let record = state
                .storage
                .update_browser_session(
                    state.owner_id,
                    session_id,
                    &current.controller,
                    "awaiting_approval",
                    None,
                    Some(approval.id),
                    unix_time_ms(),
                )
                .await?;
            let session = summary(record)?;
            publish_session(&state, session.clone()).await?;
            Ok((
                StatusCode::ACCEPTED,
                Json(response(session, Some(approval), None)),
            ))
        }
        Err(error) => Err(tool_error(&error)),
    }
}

pub(super) async fn close(
    State(state): State<AppState>,
    Extension(identity): Extension<AuthenticatedIdentity>,
    Path(session_id): Path<Uuid>,
    Json(request): Json<BrowserMutationRequest>,
) -> Result<(StatusCode, Json<BrowserActionResponse>), ApiError> {
    let current = state
        .storage
        .browser_session(state.owner_id, session_id)
        .await?;
    enforce_takeover_owner(&identity, &current)?;
    let mut canonical = request.clone();
    canonical.approval_id = None;
    if matches!(
        claim(
            &state,
            request.idempotency_key,
            &format!("browser_close:{session_id}"),
            &canonical,
        )
        .await?,
        IdempotencyClaim::Replayed { .. }
    ) {
        return Ok((
            StatusCode::OK,
            Json(response(summary(current)?, None, None)),
        ));
    }
    if let Some(runtime_session_id) = current.runtime_session_id {
        state.ensure_policy_loaded().await?;
        match runtime(&state)?
            .close(
                context_for(&state, &identity, &current, request.request_id),
                runtime_session_id,
                request.approval_id,
            )
            .await
        {
            Ok(()) => {}
            Err(ToolError::ApprovalRequired(ticket)) => {
                state
                    .storage
                    .release_idempotency(request.idempotency_key)
                    .await?;
                let approval = persist_approval(
                    &state,
                    current.chat_id,
                    &ticket,
                    "homebot.browser.close",
                    "Approve closing browser",
                )
                .await?;
                let record = state
                    .storage
                    .update_browser_session(
                        state.owner_id,
                        session_id,
                        &current.controller,
                        "awaiting_approval",
                        None,
                        Some(approval.id),
                        unix_time_ms(),
                    )
                    .await?;
                let session = summary(record)?;
                publish_session(&state, session.clone()).await?;
                return Ok((
                    StatusCode::ACCEPTED,
                    Json(response(session, Some(approval), None)),
                ));
            }
            Err(error) => return Err(tool_error(&error)),
        }
    }
    let record = state
        .storage
        .update_browser_session(
            state.owner_id,
            session_id,
            &current.controller,
            "closed",
            None,
            None,
            unix_time_ms(),
        )
        .await?;
    let session = summary(record)?;
    publish_session(&state, session.clone()).await?;
    Ok((StatusCode::OK, Json(response(session, None, None))))
}

async fn activate_created_session(
    state: &AppState,
    request: &CreateBrowserSessionRequest,
    profile: BrowserProfileRecord,
    runtime_session_id: Uuid,
    now: i64,
) -> Result<BrowserActionResponse, ApiError> {
    let record = BrowserSessionRecord {
        id: request.idempotency_key,
        owner_id: state.owner_id,
        chat_id: request.chat_id,
        bot_id: request.bot_id,
        profile_id: profile.id,
        runtime_session_id: Some(runtime_session_id),
        profile_name: profile.display_name,
        directory_ref: profile.directory_ref,
        current_url: None,
        controller: "bot".to_owned(),
        status: "active".to_owned(),
        pending_approval_id: None,
        controlling_device_id: None,
        takeover_expires_at_ms: None,
        created_at_ms: now,
        updated_at_ms: now,
    };
    let record = if state
        .storage
        .browser_sessions(state.owner_id, Some(record.chat_id))
        .await?
        .into_iter()
        .any(|existing| existing.id == record.id)
    {
        state
            .storage
            .activate_browser_session(state.owner_id, record.id, runtime_session_id, now)
            .await?
    } else {
        state.storage.create_browser_session(&record).await?
    };
    let session = summary(record)?;
    publish_session(state, session.clone()).await?;
    Ok(response(session, None, None))
}

async fn complete_action(
    state: &AppState,
    current: BrowserSessionRecord,
    result: BrowserResult,
) -> Result<BrowserActionResponse, ApiError> {
    let (url, artifact) = match result {
        BrowserResult::Url { url } => (Some(url), None),
        BrowserResult::ScreenshotPng { bytes } => (
            None,
            Some(
                persist_generated_artifact(
                    state,
                    GeneratedArtifact {
                        chat_id: current.chat_id,
                        message_id: None,
                        activity_id: None,
                        name: "browser.png",
                        kind: "browser_screenshot",
                        media_type: "image/png",
                        bytes: &bytes,
                    },
                )
                .await
                .map_err(|_| ApiError::internal())?,
            ),
        ),
        BrowserResult::NavigationAccepted
        | BrowserResult::Evaluation { .. }
        | BrowserResult::SessionCreated { .. }
        | BrowserResult::SessionClosed => (None, None),
    };
    let record = state
        .storage
        .update_browser_session(
            state.owner_id,
            current.id,
            &current.controller,
            "active",
            url.as_deref(),
            None,
            unix_time_ms(),
        )
        .await?;
    let session = summary(record)?;
    publish_session(state, session.clone()).await?;
    Ok(response(session, None, artifact))
}

async fn pending_session(
    state: &AppState,
    request: &CreateBrowserSessionRequest,
    profile: &BrowserProfileRecord,
    approval_id: Uuid,
    now: i64,
) -> Result<BrowserSessionRecord, ApiError> {
    if let Some(existing) = state
        .storage
        .browser_sessions(state.owner_id, Some(request.chat_id))
        .await?
        .into_iter()
        .find(|session| session.id == request.idempotency_key)
    {
        return Ok(existing);
    }
    Ok(state
        .storage
        .create_browser_session(&BrowserSessionRecord {
            id: request.idempotency_key,
            owner_id: state.owner_id,
            chat_id: request.chat_id,
            bot_id: request.bot_id,
            profile_id: profile.id,
            runtime_session_id: None,
            profile_name: profile.display_name.clone(),
            directory_ref: profile.directory_ref.clone(),
            current_url: None,
            controller: "bot".to_owned(),
            status: "awaiting_approval".to_owned(),
            pending_approval_id: Some(approval_id),
            controlling_device_id: None,
            takeover_expires_at_ms: None,
            created_at_ms: now,
            updated_at_ms: now,
        })
        .await?)
}

fn operation_context(
    state: &AppState,
    identity: &AuthenticatedIdentity,
    request: &CreateBrowserSessionRequest,
    workspace_id: Uuid,
) -> OperationContext {
    OperationContext {
        operation_id: request.request_id,
        owner_id: state.owner_id,
        device_id: device_id(identity),
        bot_id: request.bot_id,
        chat_id: request.chat_id,
        workspace_id,
    }
}

fn context_for(
    state: &AppState,
    identity: &AuthenticatedIdentity,
    session: &BrowserSessionRecord,
    operation_id: Uuid,
) -> OperationContext {
    OperationContext {
        operation_id,
        owner_id: state.owner_id,
        device_id: device_id(identity),
        bot_id: session.bot_id,
        chat_id: session.chat_id,
        workspace_id: Uuid::nil(),
    }
}

const fn device_id(identity: &AuthenticatedIdentity) -> Uuid {
    match identity {
        AuthenticatedIdentity::Owner => Uuid::nil(),
        AuthenticatedIdentity::Device { id } => *id,
    }
}

fn enforce_takeover_owner(
    identity: &AuthenticatedIdentity,
    session: &BrowserSessionRecord,
) -> Result<(), ApiError> {
    if session.controller != "user" {
        return Ok(());
    }
    if session.controlling_device_id == Some(device_id(identity)) {
        return Ok(());
    }
    Err(ApiError::forbidden(
        "Another device controls this browser session",
    ))
}

fn enforce_takeover_claimable(
    identity: &AuthenticatedIdentity,
    session: &BrowserSessionRecord,
) -> Result<(), ApiError> {
    if session.controller != "user"
        || session.controlling_device_id == Some(device_id(identity))
        || session
            .takeover_expires_at_ms
            .is_none_or(|expires| expires <= unix_time_ms())
    {
        return Ok(());
    }
    Err(ApiError::forbidden(
        "Another device controls this browser session",
    ))
}

fn enforce_action_controller(
    identity: &AuthenticatedIdentity,
    session: &BrowserSessionRecord,
) -> Result<(), ApiError> {
    enforce_takeover_owner(identity, session)?;
    if session.controller == "user"
        && session
            .takeover_expires_at_ms
            .is_some_and(|expires| expires <= unix_time_ms())
    {
        return Err(ApiError::conflict(
            "Browser takeover expired; return control before continuing",
        ));
    }
    Ok(())
}

fn protocol_action(command: &BrowserCommand) -> BrowserAction {
    match command {
        BrowserCommand::Navigate { url } => BrowserAction::Navigate { url: url.clone() },
        BrowserCommand::CurrentUrl => BrowserAction::CurrentUrl,
        BrowserCommand::CaptureScreenshot => BrowserAction::CaptureScreenshot,
    }
}

pub(super) fn summary(record: BrowserSessionRecord) -> Result<BrowserSessionSummary, ApiError> {
    Ok(BrowserSessionSummary {
        id: record.id,
        chat_id: record.chat_id,
        bot_id: record.bot_id,
        profile_id: record.profile_id,
        profile_name: record.profile_name,
        current_url: record.current_url,
        controller: parse_enum(&record.controller)?,
        status: parse_enum(&record.status)?,
        pending_approval_id: record.pending_approval_id,
        created_at_ms: record.created_at_ms,
        updated_at_ms: record.updated_at_ms,
    })
}

fn response(
    session: BrowserSessionSummary,
    approval: Option<ApprovalSummary>,
    artifact: Option<homebot_protocol::ArtifactSummary>,
) -> BrowserActionResponse {
    BrowserActionResponse {
        session,
        approval,
        artifact,
    }
}

async fn publish_session(state: &AppState, session: BrowserSessionSummary) -> Result<(), ApiError> {
    let activity = ExecutionActivity {
        id: session.id,
        chat_id: session.chat_id,
        message_id: None,
        kind: "browser".to_owned(),
        title: format!("Shared browser: {}", session.profile_name),
        detail: format!("{:?} control • {:?}", session.controller, session.status),
        presentation_json: serde_json::to_value(ActivityPresentation {
            risk: homebot_protocol::RiskLevel::Elevated,
            detail: ActivityDetail::Browser {
                action: format!("{:?}", session.controller).to_ascii_lowercase(),
                url: session
                    .current_url
                    .clone()
                    .unwrap_or_else(|| "about:blank".to_owned()),
                page_title: None,
                screenshot_artifact_id: None,
            },
            copy_text: None,
            open_artifact_id: None,
        })
        .map_err(|_| ApiError::internal())?,
        status: match session.status {
            homebot_protocol::BrowserSessionStatus::Active => ActivityStatus::Running,
            homebot_protocol::BrowserSessionStatus::AwaitingApproval => ActivityStatus::Pending,
            homebot_protocol::BrowserSessionStatus::Closed => ActivityStatus::Succeeded,
            homebot_protocol::BrowserSessionStatus::Failed => ActivityStatus::Failed,
        },
        requires_attention: session.status
            == homebot_protocol::BrowserSessionStatus::AwaitingApproval,
        started_at_ms: session.created_at_ms,
        finished_at_ms: (session.status == homebot_protocol::BrowserSessionStatus::Closed)
            .then_some(session.updated_at_ms),
    };
    state
        .storage
        .upsert_activity(state.owner_id, &activity)
        .await?;
    publish(
        state,
        "activity_changed",
        ServerEventBody::ActivityChanged {
            activity: crate::chats::activity_summary(activity),
        },
    )
    .await?;
    publish(
        state,
        "browser_session_changed",
        ServerEventBody::BrowserSessionChanged { session },
    )
    .await
}

fn runtime(state: &AppState) -> Result<Arc<dyn BrowserRuntime>, ApiError> {
    state.browser_runtime.clone().ok_or_else(|| {
        ApiError::conflict(
            "Local browser control is unavailable; configure a loopback CDP endpoint",
        )
    })
}

fn tool_error(error: &ToolError) -> ApiError {
    match error {
        ToolError::Denied => ApiError::forbidden("Browser capability denied by server policy"),
        ToolError::InvalidRequest(_) | ToolError::PathOutsideWorkspace => {
            ApiError::validation("Browser request is invalid")
        }
        ToolError::ApprovalRequired(_) => ApiError::internal(),
        _ => ApiError::conflict("Browser operation could not be completed safely"),
    }
}

fn parse_enum<T: serde::de::DeserializeOwned>(value: &str) -> Result<T, ApiError> {
    serde_json::from_value(serde_json::Value::String(value.to_owned()))
        .map_err(|_| ApiError::internal())
}
