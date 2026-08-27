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
use homebot_providers::{ProviderTool, ProviderToolCall, ProviderToolResult};
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
const PROVIDER_BROWSER_OPEN: &str = "homebot_browser_open";
const PROVIDER_BROWSER_NAVIGATE: &str = "homebot_browser_navigate";
const PROVIDER_BROWSER_CURRENT_URL: &str = "homebot_browser_current_url";
const PROVIDER_BROWSER_SCREENSHOT: &str = "homebot_browser_screenshot";
const PROVIDER_BROWSER_CLOSE: &str = "homebot_browser_close";

pub(super) enum BrowserProviderOutcome {
    Result(ProviderToolResult),
    Cancelled,
}

enum BrowserRuntimeFailure {
    Tool(ToolError),
    Stalled,
}

pub(super) fn provider_tools(state: &AppState) -> Vec<ProviderTool> {
    if state.browser_runtime.is_none() {
        return Vec::new();
    }
    let empty = serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {}
    });
    vec![
        ProviderTool {
            name: PROVIDER_BROWSER_OPEN.to_owned(),
            description: "Open or reuse HomeBot's shared local browser. Cookies stay in the server-owned profile and are never returned to the model.".to_owned(),
            input_schema: empty.clone(),
        },
        ProviderTool {
            name: PROVIDER_BROWSER_NAVIGATE.to_owned(),
            description: "Navigate HomeBot's shared local browser to an HTTPS URL. HomeBot opens the browser first when needed and may require owner approval.".to_owned(),
            input_schema: serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {"url": {"type": "string", "format": "uri", "maxLength": 2048}},
                "required": ["url"]
            }),
        },
        ProviderTool {
            name: PROVIDER_BROWSER_CURRENT_URL.to_owned(),
            description: "Read the current HTTPS page URL from HomeBot's shared local browser.".to_owned(),
            input_schema: empty.clone(),
        },
        ProviderTool {
            name: PROVIDER_BROWSER_SCREENSHOT.to_owned(),
            description: "Capture the current browser view as a server-owned PNG artifact. The result contains artifact metadata, never a local path.".to_owned(),
            input_schema: empty.clone(),
        },
        ProviderTool {
            name: PROVIDER_BROWSER_CLOSE.to_owned(),
            description: "Close this chat's active HomeBot browser session without deleting its persistent login profile.".to_owned(),
            input_schema: empty,
        },
    ]
}

pub(super) async fn handle_provider_tool(
    state: &AppState,
    operation_id: Uuid,
    chat_id: Uuid,
    bot_id: Uuid,
    message_id: Uuid,
    call: &ProviderToolCall,
) -> Option<BrowserProviderOutcome> {
    if !matches!(
        call.name.as_str(),
        PROVIDER_BROWSER_OPEN
            | PROVIDER_BROWSER_NAVIGATE
            | PROVIDER_BROWSER_CURRENT_URL
            | PROVIDER_BROWSER_SCREENSHOT
            | PROVIDER_BROWSER_CLOSE
    ) {
        return None;
    }
    Some(
        match call_provider_tool(state, operation_id, chat_id, bot_id, message_id, call).await {
            Ok(Some(content)) => BrowserProviderOutcome::Result(ProviderToolResult {
                success: true,
                content,
            }),
            Ok(None) => BrowserProviderOutcome::Cancelled,
            Err(content) => BrowserProviderOutcome::Result(ProviderToolResult {
                success: false,
                content,
            }),
        },
    )
}

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
#[serde(deny_unknown_fields)]
struct NoProviderArguments {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NavigateProviderArguments {
    url: String,
}

async fn call_provider_tool(
    state: &AppState,
    operation_id: Uuid,
    chat_id: Uuid,
    bot_id: Uuid,
    message_id: Uuid,
    call: &ProviderToolCall,
) -> Result<Option<String>, String> {
    if state.browser_runtime.is_none() {
        return Err("Local browser control is unavailable on this HomeBot server".to_owned());
    }
    if call.name == PROVIDER_BROWSER_CLOSE {
        parse_no_provider_arguments(&call.arguments)?;
        return close_for_provider(state, operation_id, chat_id, bot_id, message_id).await;
    }
    let action = match call.name.as_str() {
        PROVIDER_BROWSER_OPEN => {
            parse_no_provider_arguments(&call.arguments)?;
            None
        }
        PROVIDER_BROWSER_NAVIGATE => {
            let arguments: NavigateProviderArguments =
                serde_json::from_value(call.arguments.clone())
                    .map_err(|_| "Browser navigation requires only an HTTPS url".to_owned())?;
            let url = url::Url::parse(arguments.url.trim())
                .map_err(|_| "Browser navigation requires a valid HTTPS URL".to_owned())?;
            if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
                return Err(
                    "Browser navigation accepts only HTTPS URLs without embedded credentials"
                        .to_owned(),
                );
            }
            Some(BrowserAction::Navigate {
                url: url.to_string(),
            })
        }
        PROVIDER_BROWSER_CURRENT_URL => {
            parse_no_provider_arguments(&call.arguments)?;
            Some(BrowserAction::CurrentUrl)
        }
        PROVIDER_BROWSER_SCREENSHOT => {
            parse_no_provider_arguments(&call.arguments)?;
            Some(BrowserAction::CaptureScreenshot)
        }
        _ => return Err("Unknown HomeBot browser tool".to_owned()),
    };
    if state
        .storage
        .browser_takeover_active(state.owner_id, chat_id)
        .await
        .map_err(|_| "HomeBot could not verify shared-browser control".to_owned())?
    {
        return Err(
            "The shared browser is paused for human takeover; return it to the Bot before using browser tools"
                .to_owned(),
        );
    }
    let current = ensure_provider_session(state, operation_id, chat_id, bot_id, message_id).await?;
    let Some(current) = current else {
        return Ok(None);
    };
    let Some(action) = action else {
        let session =
            summary(current).map_err(|_| "HomeBot stored an invalid browser session".to_owned())?;
        return serialize_provider_response(&response(session, None, None)).map(Some);
    };
    execute_for_provider(
        state,
        operation_id,
        message_id,
        current,
        action,
        provider_tool_title(&call.name),
    )
    .await
}

fn parse_no_provider_arguments(arguments: &serde_json::Value) -> Result<(), String> {
    serde_json::from_value::<NoProviderArguments>(arguments.clone())
        .map(|_| ())
        .map_err(|_| "This browser tool does not accept arguments".to_owned())
}

async fn ensure_provider_session(
    state: &AppState,
    operation_id: Uuid,
    chat_id: Uuid,
    bot_id: Uuid,
    message_id: Uuid,
) -> Result<Option<BrowserSessionRecord>, String> {
    if let Some(active) = state
        .storage
        .browser_sessions(state.owner_id, Some(chat_id))
        .await
        .map_err(|_| "HomeBot could not read browser sessions".to_owned())?
        .into_iter()
        .find(|session| {
            session.controller == "bot"
                && session.status == "active"
                && session.runtime_session_id.is_some()
        })
    {
        return Ok(Some(active));
    }
    state
        .ensure_policy_loaded()
        .await
        .map_err(|_| "HomeBot could not load browser policy".to_owned())?;
    let now = unix_time_ms();
    let profile_id = Uuid::new_v5(&state.owner_id, b"homebot-shared-browser-profile");
    let profile = state
        .storage
        .upsert_browser_profile(&BrowserProfileRecord {
            id: profile_id,
            owner_id: state.owner_id,
            display_name: "HomeBot shared browser".to_owned(),
            directory_ref: format!("profile-{profile_id}"),
            created_at_ms: now,
            updated_at_ms: now,
        })
        .await
        .map_err(|_| "HomeBot could not open the shared browser profile".to_owned())?;
    let context = provider_operation_context(state, operation_id, chat_id, bot_id).await?;
    let browser_profile = BrowserSessionProfile {
        profile_id,
        display_name: profile.display_name.clone(),
        profile_directory: PathBuf::from(&profile.directory_ref),
    };
    let Some(runtime) = state.browser_runtime.as_ref() else {
        return Err("Local browser control is unavailable on this HomeBot server".to_owned());
    };
    let runtime_session_id = match runtime
        .create(context.clone(), &browser_profile, None)
        .await
    {
        Ok(id) => id,
        Err(ToolError::ApprovalRequired(ticket)) => {
            let Some(approval_id) = crate::provider_turn::await_capability_approval(
                state,
                operation_id,
                chat_id,
                message_id,
                &ticket,
                "homebot.browser.session",
                "Open shared browser",
            )
            .await?
            else {
                return Ok(None);
            };
            runtime
                .create(context, &browser_profile, Some(approval_id))
                .await
                .map_err(|error| provider_browser_error(&error))?
        }
        Err(error) => return Err(provider_browser_error(&error)),
    };
    let request = CreateBrowserSessionRequest {
        request_id: operation_id,
        idempotency_key: Uuid::now_v7(),
        chat_id,
        bot_id,
        profile_id,
        profile_name: profile.display_name.clone(),
        approval_id: None,
    };
    activate_created_session(state, &request, profile, runtime_session_id, now)
        .await
        .map_err(|_| "HomeBot could not record the shared browser session".to_owned())?;
    state
        .storage
        .browser_session(state.owner_id, request.idempotency_key)
        .await
        .map(Some)
        .map_err(|_| "HomeBot could not read the shared browser session".to_owned())
}

async fn execute_for_provider(
    state: &AppState,
    operation_id: Uuid,
    message_id: Uuid,
    current: BrowserSessionRecord,
    action: BrowserAction,
    title: &str,
) -> Result<Option<String>, String> {
    state
        .ensure_policy_loaded()
        .await
        .map_err(|_| "HomeBot could not load browser policy".to_owned())?;
    let context =
        provider_operation_context(state, operation_id, current.chat_id, current.bot_id).await?;
    let Some(runtime) = state.browser_runtime.as_ref() else {
        return Err("Local browser control is unavailable on this HomeBot server".to_owned());
    };
    let runtime_session_id = current
        .runtime_session_id
        .ok_or_else(|| "The shared browser session is no longer active".to_owned())?;
    let result = match execute_runtime(
        state,
        runtime,
        context.clone(),
        runtime_session_id,
        action.clone(),
        None,
    )
    .await
    {
        Ok(result) => result,
        Err(BrowserRuntimeFailure::Tool(ToolError::ApprovalRequired(ticket))) => {
            let Some(approval_id) = crate::provider_turn::await_capability_approval(
                state,
                operation_id,
                current.chat_id,
                message_id,
                &ticket,
                "homebot.browser.action",
                title,
            )
            .await?
            else {
                return Ok(None);
            };
            match execute_runtime(
                state,
                runtime,
                context,
                runtime_session_id,
                action,
                Some(approval_id),
            )
            .await
            {
                Ok(result) => result,
                Err(BrowserRuntimeFailure::Tool(error)) => {
                    return Err(provider_browser_error(&error));
                }
                Err(BrowserRuntimeFailure::Stalled) => {
                    fail_stalled_session(state, &current)
                        .await
                        .map_err(|_| "HomeBot could not record the stalled browser".to_owned())?;
                    return Err("The local browser stopped responding and was detached; retry to open a fresh session".to_owned());
                }
            }
        }
        Err(BrowserRuntimeFailure::Tool(error)) => return Err(provider_browser_error(&error)),
        Err(BrowserRuntimeFailure::Stalled) => {
            fail_stalled_session(state, &current)
                .await
                .map_err(|_| "HomeBot could not record the stalled browser".to_owned())?;
            return Err("The local browser stopped responding and was detached; retry to open a fresh session".to_owned());
        }
    };
    let response = complete_action(state, current, result, Some(message_id))
        .await
        .map_err(|_| "HomeBot could not record the browser result".to_owned())?;
    serialize_provider_response(&response).map(Some)
}

async fn close_for_provider(
    state: &AppState,
    operation_id: Uuid,
    chat_id: Uuid,
    bot_id: Uuid,
    message_id: Uuid,
) -> Result<Option<String>, String> {
    let sessions = state
        .storage
        .browser_sessions(state.owner_id, Some(chat_id))
        .await
        .map_err(|_| "HomeBot could not read browser sessions".to_owned())?;
    if sessions
        .iter()
        .any(|session| session.controller == "user" && session.status == "active")
    {
        return Err(
            "The shared browser is controlled by a person and cannot be closed by a Bot".to_owned(),
        );
    }
    let Some(current) = sessions.into_iter().find(|session| {
        session.controller == "bot"
            && session.status == "active"
            && session.runtime_session_id.is_some()
    }) else {
        return Ok(Some(
            "{\"closed\":false,\"reason\":\"no_active_session\"}".to_owned(),
        ));
    };
    state
        .ensure_policy_loaded()
        .await
        .map_err(|_| "HomeBot could not load browser policy".to_owned())?;
    let context = provider_operation_context(state, operation_id, chat_id, bot_id).await?;
    let Some(runtime) = state.browser_runtime.as_ref() else {
        return Err("Local browser control is unavailable on this HomeBot server".to_owned());
    };
    let Some(runtime_session_id) = current.runtime_session_id else {
        return Err("The shared browser session is no longer active".to_owned());
    };
    match runtime
        .close(context.clone(), runtime_session_id, None)
        .await
    {
        Ok(()) => {}
        Err(ToolError::ApprovalRequired(ticket)) => {
            let Some(approval_id) = crate::provider_turn::await_capability_approval(
                state,
                operation_id,
                chat_id,
                message_id,
                &ticket,
                "homebot.browser.close",
                "Close shared browser",
            )
            .await?
            else {
                return Ok(None);
            };
            runtime
                .close(context, runtime_session_id, Some(approval_id))
                .await
                .map_err(|error| provider_browser_error(&error))?;
        }
        Err(error) => return Err(provider_browser_error(&error)),
    }
    let record = state
        .storage
        .update_browser_session(
            state.owner_id,
            current.id,
            "bot",
            "closed",
            None,
            None,
            unix_time_ms(),
        )
        .await
        .map_err(|_| "HomeBot could not record the closed browser session".to_owned())?;
    let session =
        summary(record).map_err(|_| "HomeBot stored an invalid browser session".to_owned())?;
    publish_session(state, session.clone())
        .await
        .map_err(|_| "HomeBot could not publish the closed browser session".to_owned())?;
    serialize_provider_response(&response(session, None, None)).map(Some)
}

async fn provider_operation_context(
    state: &AppState,
    operation_id: Uuid,
    chat_id: Uuid,
    bot_id: Uuid,
) -> Result<OperationContext, String> {
    let workspace_id = state
        .storage
        .chat_workspace(state.owner_id, chat_id)
        .await
        .map_err(|_| "HomeBot could not read the chat workspace".to_owned())?
        .map_or(Uuid::nil(), |workspace| workspace.workspace_id);
    Ok(OperationContext {
        operation_id,
        owner_id: state.owner_id,
        device_id: Uuid::nil(),
        bot_id,
        chat_id,
        workspace_id,
    })
}

fn provider_tool_title(name: &str) -> &str {
    match name {
        PROVIDER_BROWSER_NAVIGATE => "Navigate shared browser",
        PROVIDER_BROWSER_CURRENT_URL => "Read shared browser URL",
        PROVIDER_BROWSER_SCREENSHOT => "Capture shared browser screenshot",
        _ => "Use shared browser",
    }
}

fn provider_browser_error(error: &ToolError) -> String {
    match error {
        ToolError::Denied => "The owner denied this browser action".to_owned(),
        ToolError::InvalidApproval => "The browser approval is no longer valid".to_owned(),
        ToolError::InvalidRequest(_) => "The browser request is invalid".to_owned(),
        _ => "HomeBot could not complete the browser action safely".to_owned(),
    }
}

fn serialize_provider_response(response: &BrowserActionResponse) -> Result<String, String> {
    serde_json::to_string(response)
        .map_err(|_| "HomeBot could not encode the browser result".to_owned())
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
                None,
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
                None,
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
    let runtime = runtime(&state)?;
    let result = execute_runtime(
        &state,
        &runtime,
        context_for(&state, &identity, &current, request.request_id),
        current.runtime_session_id.unwrap_or_default(),
        action,
        request.approval_id,
    )
    .await;
    match result {
        Ok(result) => Ok((
            StatusCode::OK,
            Json(complete_action(&state, current, result, None).await?),
        )),
        Err(BrowserRuntimeFailure::Tool(ToolError::ApprovalRequired(ticket))) => {
            state
                .storage
                .release_idempotency(request.idempotency_key)
                .await?;
            let approval = persist_approval(
                &state,
                current.chat_id,
                None,
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
        Err(BrowserRuntimeFailure::Tool(error)) => Err(tool_error(&error)),
        Err(BrowserRuntimeFailure::Stalled) => {
            state
                .storage
                .release_idempotency(request.idempotency_key)
                .await?;
            fail_stalled_session(&state, &current).await?;
            Err(ApiError::conflict(
                "The local browser stopped responding and was detached; retry to open a fresh session",
            ))
        }
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
                    None,
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
    message_id: Option<Uuid>,
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
                        message_id,
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

async fn execute_runtime(
    state: &AppState,
    runtime: &Arc<dyn BrowserRuntime>,
    context: OperationContext,
    session_id: Uuid,
    action: BrowserAction,
    approval_id: Option<Uuid>,
) -> Result<BrowserResult, BrowserRuntimeFailure> {
    tokio::time::timeout(
        state.browser_runtime_timeout,
        runtime.execute(context, session_id, action, approval_id),
    )
    .await
    .map_err(|_| BrowserRuntimeFailure::Stalled)?
    .map_err(BrowserRuntimeFailure::Tool)
}

async fn fail_stalled_session(
    state: &AppState,
    current: &BrowserSessionRecord,
) -> Result<(), ApiError> {
    let record = state
        .storage
        .fail_stalled_browser_session(state.owner_id, current.id, unix_time_ms())
        .await?;
    publish_session(state, summary(record)?).await
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
