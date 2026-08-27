//! Server-owned structured input requested by an active Bot operation.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use homebot_domain::chat::{ActivityStatus as DomainStatus, ExecutionActivity};
use homebot_protocol::{
    ActivityDetail, ActivityPresentation, InteractionRequestKind, InteractionResponseRequest,
    RiskLevel, ServerEventBody,
};
use homebot_providers::{ProviderTool, ProviderToolCall, ProviderToolResult};
use homebot_secrets::SecretInput;
use serde::{Deserialize, Serialize};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::{Mutex, Notify};
use uuid::Uuid;

use crate::{
    AppState,
    bots::{ApiError, claim},
    chats::{activity_summary, publish},
    unix_time_ms,
};

const DECISION: &str = "homebot_request_decision";
const SECRET: &str = "homebot_request_secret";
const MAX_TEXT: usize = 1_000;
const MAX_CHOICES: usize = 8;
const TTL: std::time::Duration = std::time::Duration::from_secs(600);

#[derive(Debug)]
pub(super) struct PendingInteraction {
    pub operation_id: Uuid,
    kind: InteractionRequestKind,
    choices: Vec<String>,
    activity: Mutex<ExecutionActivity>,
    result: Mutex<Option<String>>,
    responded: AtomicBool,
    cancelled: AtomicBool,
    ready: Notify,
}

pub(super) fn provider_tools() -> Vec<ProviderTool> {
    vec![
        ProviderTool {
            name: DECISION.to_owned(),
            description: "Ask the user for an explicit confirmation or one choice. HomeBot renders and owns the response card; do not restate the choices as a prose menu.".to_owned(),
            input_schema: serde_json::json!({"type":"object","additionalProperties":false,"properties":{"prompt":{"type":"string","minLength":1,"maxLength":MAX_TEXT},"choices":{"type":"array","maxItems":MAX_CHOICES,"uniqueItems":true,"items":{"type":"string","minLength":1,"maxLength":120}}},"required":["prompt"]}),
        },
        ProviderTool {
            name: SECRET.to_owned(),
            description: "Request one secret through HomeBot's secure native card. The value is stored in the server vault and is never returned to the model; the tool returns only an opaque reference.".to_owned(),
            input_schema: serde_json::json!({"type":"object","additionalProperties":false,"properties":{"label":{"type":"string","minLength":1,"maxLength":120},"description":{"type":"string","minLength":1,"maxLength":MAX_TEXT}},"required":["label","description"]}),
        },
    ]
}

pub(super) async fn handle_provider_tool(
    state: &AppState,
    operation_id: Uuid,
    chat_id: Uuid,
    message_id: Uuid,
    call: &ProviderToolCall,
) -> Option<ProviderToolResult> {
    let request = match call.name.as_str() {
        DECISION => decode_decision(&call.arguments),
        SECRET => decode_secret(&call.arguments),
        _ => return None,
    };
    Some(match request {
        Ok(request) => {
            match await_response(state, operation_id, chat_id, message_id, request).await {
                Ok(content) => ProviderToolResult {
                    success: true,
                    content,
                },
                Err(content) => ProviderToolResult {
                    success: false,
                    content,
                },
            }
        }
        Err(content) => ProviderToolResult {
            success: false,
            content,
        },
    })
}

struct NewInteraction {
    title: String,
    prompt: String,
    kind: InteractionRequestKind,
    choices: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DecisionArguments {
    prompt: String,
    #[serde(default)]
    choices: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SecretArguments {
    label: String,
    description: String,
}

fn decode_decision(value: &serde_json::Value) -> Result<NewInteraction, String> {
    let value: DecisionArguments =
        serde_json::from_value(value.clone()).map_err(|_| "Invalid decision request".to_owned())?;
    let prompt = bounded(&value.prompt, MAX_TEXT, "Decision prompt")?;
    if value.choices.len() > MAX_CHOICES {
        return Err("A decision may contain at most eight choices".to_owned());
    }
    let mut choices = Vec::with_capacity(value.choices.len());
    for choice in value.choices {
        let choice = bounded(&choice, 120, "Decision choice")?;
        if choices.contains(&choice) {
            return Err("Decision choices must be unique".to_owned());
        }
        choices.push(choice);
    }
    Ok(NewInteraction {
        title: if choices.is_empty() {
            "Confirmation needed"
        } else {
            "Choose one"
        }
        .to_owned(),
        prompt,
        kind: if choices.is_empty() {
            InteractionRequestKind::Confirm
        } else {
            InteractionRequestKind::PickOne
        },
        choices,
    })
}

fn decode_secret(value: &serde_json::Value) -> Result<NewInteraction, String> {
    let value: SecretArguments =
        serde_json::from_value(value.clone()).map_err(|_| "Invalid secret request".to_owned())?;
    Ok(NewInteraction {
        title: bounded(&value.label, 120, "Secret label")?,
        prompt: bounded(&value.description, MAX_TEXT, "Secret description")?,
        kind: InteractionRequestKind::Secret,
        choices: Vec::new(),
    })
}

fn bounded(value: &str, max: usize, label: &str) -> Result<String, String> {
    let value = value.trim().to_owned();
    if value.is_empty() || value.chars().count() > max {
        Err(format!("{label} is empty or exceeds its limit"))
    } else {
        Ok(value)
    }
}

async fn await_response(
    state: &AppState,
    operation_id: Uuid,
    chat_id: Uuid,
    message_id: Uuid,
    request: NewInteraction,
) -> Result<String, String> {
    if state
        .pending_interactions
        .lock()
        .await
        .values()
        .filter(|pending| pending.operation_id == operation_id)
        .count()
        >= 4
    {
        return Err("This Bot already has four pending requests".to_owned());
    }
    let id = Uuid::now_v7();
    let presentation = ActivityPresentation {
        risk: if request.kind == InteractionRequestKind::Secret {
            RiskLevel::Elevated
        } else {
            RiskLevel::Low
        },
        detail: ActivityDetail::Interaction {
            request_kind: request.kind,
            prompt: request.prompt.clone(),
            choices: request.choices.clone(),
        },
        copy_text: None,
        open_artifact_id: None,
    };
    let activity = ExecutionActivity {
        id,
        chat_id,
        message_id: Some(message_id),
        kind: "interaction".to_owned(),
        title: request.title,
        detail: request.prompt,
        presentation_json: serde_json::to_value(presentation)
            .map_err(|_| "HomeBot could not create the request card".to_owned())?,
        status: DomainStatus::Pending,
        requires_attention: true,
        started_at_ms: unix_time_ms(),
        finished_at_ms: None,
    };
    let pending = Arc::new(PendingInteraction {
        operation_id,
        kind: request.kind,
        choices: request.choices.clone(),
        activity: Mutex::new(activity.clone()),
        result: Mutex::new(None),
        responded: AtomicBool::new(false),
        cancelled: AtomicBool::new(false),
        ready: Notify::new(),
    });
    state
        .pending_interactions
        .lock()
        .await
        .insert(id, Arc::clone(&pending));
    if state
        .storage
        .upsert_activity(state.owner_id, &activity)
        .await
        .is_err()
        || publish(
            state,
            "activity_changed",
            ServerEventBody::ActivityChanged {
                activity: activity_summary(activity),
            },
        )
        .await
        .is_err()
    {
        state.pending_interactions.lock().await.remove(&id);
        return Err("HomeBot could not persist the request card".to_owned());
    }
    let notified = tokio::time::timeout(TTL, pending.ready.notified())
        .await
        .is_ok();
    state.pending_interactions.lock().await.remove(&id);
    if !notified {
        return Err("The interaction request expired".to_owned());
    }
    if pending.cancelled.load(Ordering::Acquire) {
        return Err("The interaction request was cancelled".to_owned());
    }
    pending
        .result
        .lock()
        .await
        .take()
        .ok_or_else(|| "The interaction request is no longer active".to_owned())
}

#[derive(Serialize)]
struct ResponseClaim {
    request_id: Uuid,
    interaction_id: Uuid,
}

fn validate_response(
    pending: &PendingInteraction,
    interaction_id: Uuid,
    request: InteractionResponseRequest,
) -> Result<(String, Option<(String, String)>), ApiError> {
    match pending.kind {
        InteractionRequestKind::Confirm => {
            let confirmed = request.confirmed.ok_or_else(|| {
                ApiError::validation("This request needs a confirmation response")
            })?;
            if request.choice.is_some() || request.secret.is_some() {
                return Err(ApiError::validation(
                    "This request accepts only a confirmation",
                ));
            }
            Ok((serde_json::json!({"confirmed":confirmed}).to_string(), None))
        }
        InteractionRequestKind::PickOne => {
            let choice = request
                .choice
                .ok_or_else(|| ApiError::validation("This request needs one choice"))?;
            if request.confirmed.is_some()
                || request.secret.is_some()
                || !pending.choices.contains(&choice)
            {
                return Err(ApiError::validation(
                    "The selected choice is not valid for this request",
                ));
            }
            Ok((serde_json::json!({"choice":choice}).to_string(), None))
        }
        InteractionRequestKind::Secret => {
            let secret = request
                .secret
                .ok_or_else(|| ApiError::validation("This request needs a secret value"))?;
            if request.confirmed.is_some()
                || request.choice.is_some()
                || secret.is_empty()
                || secret.len() > 16_384
            {
                return Err(ApiError::validation("The submitted secret is invalid"));
            }
            let locator = format!("homebot:interaction:{interaction_id}");
            Ok((
                serde_json::json!({"stored":true,"secret_reference":locator,"acknowledgement":"Secret stored securely; its value is not visible in chat."}).to_string(),
                Some((locator, secret)),
            ))
        }
    }
}

pub(super) async fn respond(
    State(state): State<AppState>,
    Path(interaction_id): Path<Uuid>,
    Json(request): Json<InteractionResponseRequest>,
) -> Result<StatusCode, ApiError> {
    let replayed = matches!(
        claim(
            &state,
            request.idempotency_key,
            &format!("respond_interaction:{interaction_id}"),
            &ResponseClaim {
                request_id: request.request_id,
                interaction_id
            }
        )
        .await?,
        homebot_storage::IdempotencyClaim::Replayed { .. }
    );
    if replayed {
        return Ok(StatusCode::NO_CONTENT);
    }
    let pending = state
        .pending_interactions
        .lock()
        .await
        .get(&interaction_id)
        .cloned()
        .ok_or_else(|| ApiError::conflict("The interaction request is no longer active"))?;
    let (result, secret) = validate_response(&pending, interaction_id, request)?;
    if pending
        .responded
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err(ApiError::conflict(
            "The interaction request already has a response",
        ));
    }
    if let Some((locator, secret)) = secret
        && state
            .secret_vault
            .put(&locator, SecretInput::new(secret))
            .await
            .is_err()
    {
        pending.responded.store(false, Ordering::Release);
        return Err(ApiError::conflict(
            "The secure credential store is unavailable",
        ));
    }
    let mut activity = pending.activity.lock().await;
    activity.status = DomainStatus::Succeeded;
    activity.detail = if pending.kind == InteractionRequestKind::Secret {
        "Secret stored securely; its value is not visible in chat.".to_owned()
    } else {
        "Response submitted".to_owned()
    };
    activity.requires_attention = false;
    activity.finished_at_ms = Some(unix_time_ms());
    state
        .storage
        .upsert_activity(state.owner_id, &activity)
        .await?;
    publish(
        &state,
        "activity_changed",
        ServerEventBody::ActivityChanged {
            activity: activity_summary(activity.clone()),
        },
    )
    .await?;
    drop(activity);
    *pending.result.lock().await = Some(result);
    pending.ready.notify_one();
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn cancel_operation(state: &AppState, operation_id: Uuid) {
    for pending in state.pending_interactions.lock().await.values() {
        if pending.operation_id == operation_id {
            pending.cancelled.store(true, Ordering::Release);
            pending.ready.notify_one();
        }
    }
}
