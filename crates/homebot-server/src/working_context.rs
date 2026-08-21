//! Server-owned provider interaction mode and working-context lifecycle.

use axum::{
    Json,
    extract::{Path, State},
};
use homebot_protocol::{
    CompactWorkingContextRequest, ContextCompactionStatus, ContextCompactionStrategy,
    InteractionMode, ServerEventBody, SetInteractionModeRequest, WorkingContextSummary,
};
use homebot_providers::{CompactRequest, ProviderAdapterId, ProviderCapability};
use homebot_storage::{IdempotencyClaim, WorkingContextRecord};
use uuid::Uuid;

use crate::{
    AppState,
    bots::{ApiError, claim},
    chats::publish,
    unix_time_ms,
};

pub(super) async fn get(
    State(state): State<AppState>,
    Path(chat_id): Path<Uuid>,
) -> Result<Json<WorkingContextSummary>, ApiError> {
    Ok(Json(summary(&state, chat_id).await?.ok_or_else(|| {
        ApiError::validation("This Bot does not have a provider profile")
    })?))
}

pub(super) async fn set_mode(
    State(state): State<AppState>,
    Path(chat_id): Path<Uuid>,
    Json(request): Json<SetInteractionModeRequest>,
) -> Result<Json<WorkingContextSummary>, ApiError> {
    let replayed = matches!(
        claim(
            &state,
            request.idempotency_key,
            &format!("set_interaction_mode:{chat_id}"),
            &request,
        )
        .await?,
        IdempotencyClaim::Replayed { .. }
    );
    let current = summary(&state, chat_id)
        .await?
        .ok_or_else(|| ApiError::validation("This Bot does not have a provider profile"))?;
    if replayed {
        return Ok(Json(current));
    }
    if request.mode == InteractionMode::Plan && !current.plan_mode_available {
        return Err(ApiError::validation(
            "The configured provider does not support plan mode",
        ));
    }
    let record = state
        .storage
        .set_working_context_mode(
            state.owner_id,
            chat_id,
            mode_name(request.mode),
            unix_time_ms(),
        )
        .await?;
    let result = hydrate(&state, record).await?;
    publish_context(&state, &result).await?;
    Ok(Json(result))
}

#[allow(clippy::too_many_lines)]
pub(super) async fn compact(
    State(state): State<AppState>,
    Path(chat_id): Path<Uuid>,
    Json(request): Json<CompactWorkingContextRequest>,
) -> Result<Json<WorkingContextSummary>, ApiError> {
    if request.target_tokens == Some(0) {
        return Err(ApiError::validation(
            "Target tokens must be greater than zero",
        ));
    }
    let replayed = matches!(
        claim(
            &state,
            request.idempotency_key,
            &format!("compact_working_context:{chat_id}"),
            &request,
        )
        .await?,
        IdempotencyClaim::Replayed { .. }
    );
    let chat = state
        .storage
        .get_direct_chat(state.owner_id, chat_id)
        .await?;
    if chat.running {
        return Err(ApiError::conflict(
            "Working context can be compacted only while the Bot is idle",
        ));
    }
    let current = summary(&state, chat_id)
        .await?
        .ok_or_else(|| ApiError::validation("This Bot does not have a provider profile"))?;
    if replayed {
        return Ok(Json(current));
    }
    if request.strategy == ContextCompactionStrategy::Compact && !current.compaction_available {
        return Err(ApiError::validation(
            "The configured provider does not support manual compaction; reset is available",
        ));
    }
    let route = state
        .storage
        .provider_route_for_bot(state.owner_id, chat.bot_id)
        .await?
        .ok_or_else(|| ApiError::validation("This Bot does not have a provider profile"))?;
    let running = state
        .storage
        .begin_working_context_compaction(state.owner_id, chat_id, unix_time_ms())
        .await?;
    publish_context(&state, &hydrate(&state, running).await?).await?;
    let result = match request.strategy {
        ContextCompactionStrategy::Reset => state
            .storage
            .reset_provider_conversation(chat.bot_id, chat_id, route.profile_id)
            .await
            .map_err(ApiError::from),
        ContextCompactionStrategy::Compact => {
            compact_provider(
                &state,
                chat.bot_id,
                chat_id,
                &route.adapter_kind,
                route.profile_id,
                request.target_tokens,
            )
            .await
        }
    };
    if result.is_err() {
        let failed = state
            .storage
            .set_working_context_compaction(
                state.owner_id,
                chat_id,
                "failed",
                false,
                false,
                Some("The provider could not compact its working context"),
                unix_time_ms(),
            )
            .await?;
        publish_context(&state, &hydrate(&state, failed).await?).await?;
        return Err(ApiError::unavailable(
            "The provider could not compact its working context",
        ));
    }
    let completed = state
        .storage
        .set_working_context_compaction(
            state.owner_id,
            chat_id,
            "completed",
            true,
            true,
            None,
            unix_time_ms(),
        )
        .await?;
    let completed = hydrate(&state, completed).await?;
    publish_context(&state, &completed).await?;
    Ok(Json(completed))
}

async fn compact_provider(
    state: &AppState,
    bot_id: Uuid,
    chat_id: Uuid,
    adapter_kind: &str,
    profile_id: Uuid,
    target_tokens: Option<u64>,
) -> Result<(), ApiError> {
    let conversation_id = state
        .storage
        .provider_conversation(bot_id, chat_id, profile_id)
        .await?
        .ok_or_else(|| ApiError::conflict("There is no provider working context to compact"))?;
    let adapter_id =
        ProviderAdapterId::new(adapter_kind.to_owned()).map_err(|_| ApiError::internal())?;
    state
        .provider_runtime
        .compact(
            &adapter_id,
            CompactRequest {
                conversation_id,
                target_tokens,
            },
        )
        .await
        .map_err(|_| ApiError::unavailable("The provider could not compact its working context"))
}

pub(super) async fn summary(
    state: &AppState,
    chat_id: Uuid,
) -> Result<Option<WorkingContextSummary>, ApiError> {
    let chat = state
        .storage
        .get_direct_chat(state.owner_id, chat_id)
        .await?;
    let Some(route) = state
        .storage
        .provider_route_for_bot(state.owner_id, chat.bot_id)
        .await?
    else {
        return Ok(None);
    };
    let record = state
        .storage
        .working_context(state.owner_id, chat_id, route.profile_id, unix_time_ms())
        .await?;
    Ok(Some(hydrate(state, record).await?))
}

async fn hydrate(
    state: &AppState,
    record: WorkingContextRecord,
) -> Result<WorkingContextSummary, ApiError> {
    let route = state
        .storage
        .provider_route_for_bot(
            state.owner_id,
            state
                .storage
                .get_direct_chat(state.owner_id, record.chat_id)
                .await?
                .bot_id,
        )
        .await?
        .ok_or_else(|| ApiError::validation("This Bot does not have a provider profile"))?;
    let adapter_id =
        ProviderAdapterId::new(route.adapter_kind.clone()).map_err(|_| ApiError::internal())?;
    let descriptor = state.provider_runtime.descriptor(&adapter_id).await.ok();
    let plan_mode_available = descriptor
        .as_ref()
        .is_some_and(|value| value.capabilities.supports(ProviderCapability::PlanMode));
    let compaction_available = descriptor
        .as_ref()
        .is_some_and(|value| value.capabilities.supports(ProviderCapability::Compaction));
    let context_window_tokens = if record.context_window_tokens.is_some() {
        record.context_window_tokens
    } else if let Some(model) = route.model.as_deref() {
        state
            .provider_runtime
            .models(&adapter_id)
            .await
            .ok()
            .and_then(|models| {
                models
                    .into_iter()
                    .find(|candidate| candidate.id == model)
                    .and_then(|candidate| candidate.context_window_tokens)
            })
    } else {
        None
    };
    Ok(WorkingContextSummary {
        chat_id: record.chat_id,
        provider_profile_id: record.provider_profile_id,
        interaction_mode: parse_mode(&record.interaction_mode)?,
        plan_mode_available,
        compaction_available,
        reset_available: true,
        used_tokens: record.used_tokens,
        context_window_tokens,
        compaction_status: parse_status(&record.compaction_status)?,
        generation: record.generation,
        compacted_at_ms: record.compacted_at_ms,
        error_message: record.last_error,
        updated_at_ms: record.updated_at_ms,
    })
}

async fn publish_context(
    state: &AppState,
    context: &WorkingContextSummary,
) -> Result<(), ApiError> {
    publish(
        state,
        "working_context_changed",
        ServerEventBody::WorkingContextChanged {
            context: context.clone(),
        },
    )
    .await
}

pub(super) fn execution_mode(mode: InteractionMode) -> homebot_providers::ExecutionMode {
    match mode {
        InteractionMode::Default => homebot_providers::ExecutionMode::Normal,
        InteractionMode::Plan => homebot_providers::ExecutionMode::Plan,
    }
}

fn mode_name(mode: InteractionMode) -> &'static str {
    match mode {
        InteractionMode::Default => "default",
        InteractionMode::Plan => "plan",
    }
}

fn parse_mode(value: &str) -> Result<InteractionMode, ApiError> {
    match value {
        "default" => Ok(InteractionMode::Default),
        "plan" => Ok(InteractionMode::Plan),
        _ => Err(ApiError::internal()),
    }
}

fn parse_status(value: &str) -> Result<ContextCompactionStatus, ApiError> {
    match value {
        "idle" => Ok(ContextCompactionStatus::Idle),
        "running" => Ok(ContextCompactionStatus::Running),
        "completed" => Ok(ContextCompactionStatus::Completed),
        "failed" => Ok(ContextCompactionStatus::Failed),
        _ => Err(ApiError::internal()),
    }
}
