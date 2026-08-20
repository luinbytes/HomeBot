use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use homebot_domain::{
    Bot, BotColor as DomainColor, BotPermissionProfile as DomainPermission,
    BotShape as DomainShape, DomainError,
};
use homebot_protocol::{
    BotAdvancedSettings, BotAttention, BotColor, BotMutationRequest, BotPermissionProfile,
    BotProviderStatus, BotResponse, BotShape, BotSummary, CreateBotRequest, ErrorCode,
    ErrorEnvelope, ServerEventBody, UpdateBotRequest,
};
use homebot_storage::{IdempotencyClaim, StorageError};
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{AppState, persist_event, unix_time_ms};

pub(super) async fn list(State(state): State<AppState>) -> Result<Json<Vec<BotSummary>>, ApiError> {
    let bots = state.storage.list_bots(state.owner_id, true).await?;
    let mut summaries = Vec::with_capacity(bots.len());
    for bot in bots {
        summaries.push(summary(&state, bot).await);
    }
    Ok(Json(summaries))
}

pub(super) async fn create(
    State(state): State<AppState>,
    Json(request): Json<CreateBotRequest>,
) -> Result<(StatusCode, Json<BotResponse>), ApiError> {
    let claim = claim(&state, request.idempotency_key, "create_bot", &request).await?;
    if matches!(claim, IdempotencyClaim::Replayed { .. }) {
        let bot = state
            .storage
            .get_bot(state.owner_id, request.idempotency_key)
            .await?;
        return Ok((
            StatusCode::OK,
            Json(BotResponse {
                bot: summary(&state, bot).await,
            }),
        ));
    }
    let mut bot = Bot::create(&request.name, &request.title)?;
    bot.id.0 = request.idempotency_key;
    bot.update_identity(
        request.name,
        request.title,
        request.description,
        to_domain_shape(request.shape),
        to_domain_color(request.color),
    )?;
    bot.provider_profile_id = request.provider_profile_id;
    bot.permission_profile = to_domain_permission(request.permission_profile);
    let bot = state
        .storage
        .create_bot(state.owner_id, bot, unix_time_ms())
        .await?;
    let bot = summary(&state, bot).await;
    publish(&state, bot.clone()).await?;
    Ok((StatusCode::CREATED, Json(BotResponse { bot })))
}

pub(super) async fn update(
    State(state): State<AppState>,
    Path(bot_id): Path<Uuid>,
    Json(request): Json<UpdateBotRequest>,
) -> Result<Json<BotResponse>, ApiError> {
    let _ = claim(
        &state,
        request.idempotency_key,
        &format!("update_bot:{bot_id}"),
        &request,
    )
    .await?;
    let mut bot = state.storage.get_bot(state.owner_id, bot_id).await?;
    bot.update_identity(
        request.name,
        request.title,
        request.description,
        to_domain_shape(request.shape),
        to_domain_color(request.color),
    )?;
    bot.provider_profile_id = request.provider_profile_id;
    bot.permission_profile = to_domain_permission(request.permission_profile);
    let bot = state
        .storage
        .update_bot(state.owner_id, bot, unix_time_ms())
        .await?;
    let bot = summary(&state, bot).await;
    publish(&state, bot.clone()).await?;
    Ok(Json(BotResponse { bot }))
}

pub(super) async fn archive(
    State(state): State<AppState>,
    Path(bot_id): Path<Uuid>,
    Json(request): Json<BotMutationRequest>,
) -> Result<Json<BotResponse>, ApiError> {
    let replayed = matches!(
        claim(
            &state,
            request.idempotency_key,
            &format!("archive_bot:{bot_id}"),
            &request,
        )
        .await?,
        IdempotencyClaim::Replayed { .. }
    );
    let bot = if replayed {
        let bot = state.storage.get_bot(state.owner_id, bot_id).await?;
        if bot.archived_at_ms.is_none() {
            return Err(ApiError::conflict(
                "The original archive operation is not reflected in current state",
            ));
        }
        bot
    } else {
        state
            .storage
            .set_bot_archived(state.owner_id, bot_id, true, unix_time_ms())
            .await?
    };
    let bot = summary(&state, bot).await;
    if !replayed {
        publish(&state, bot.clone()).await?;
    }
    Ok(Json(BotResponse { bot }))
}

pub(super) async fn restore(
    State(state): State<AppState>,
    Path(bot_id): Path<Uuid>,
    Json(request): Json<BotMutationRequest>,
) -> Result<Json<BotResponse>, ApiError> {
    let replayed = matches!(
        claim(
            &state,
            request.idempotency_key,
            &format!("restore_bot:{bot_id}"),
            &request,
        )
        .await?,
        IdempotencyClaim::Replayed { .. }
    );
    let bot = if replayed {
        let bot = state.storage.get_bot(state.owner_id, bot_id).await?;
        if bot.archived_at_ms.is_some() {
            return Err(ApiError::conflict(
                "The original restore operation is not reflected in current state",
            ));
        }
        bot
    } else {
        state
            .storage
            .set_bot_archived(state.owner_id, bot_id, false, unix_time_ms())
            .await?
    };
    let bot = summary(&state, bot).await;
    if !replayed {
        publish(&state, bot.clone()).await?;
    }
    Ok(Json(BotResponse { bot }))
}

pub(super) async fn mark_read(
    State(state): State<AppState>,
    Path(bot_id): Path<Uuid>,
    Json(request): Json<BotMutationRequest>,
) -> Result<Json<BotResponse>, ApiError> {
    let _ = claim(
        &state,
        request.idempotency_key,
        &format!("mark_bot_read:{bot_id}"),
        &request,
    )
    .await?;
    let bot = state
        .storage
        .mark_bot_read(state.owner_id, bot_id, unix_time_ms())
        .await?;
    let bot = summary(&state, bot).await;
    publish(&state, bot.clone()).await?;
    Ok(Json(BotResponse { bot }))
}

async fn claim<T: Serialize>(
    state: &AppState,
    key: Uuid,
    operation: &str,
    request: &T,
) -> Result<IdempotencyClaim, ApiError> {
    let encoded = serde_json::to_vec(request).map_err(|_| ApiError::internal())?;
    let mut digest = Sha256::new();
    digest.update(operation.as_bytes());
    digest.update([0]);
    digest.update(encoded);
    let hash = format!("{:x}", digest.finalize());
    let claim = state
        .storage
        .claim_idempotency(key, &hash, Uuid::now_v7(), unix_time_ms())
        .await?;
    if claim == IdempotencyClaim::Conflict {
        return Err(ApiError::conflict(
            "Idempotency key was already used for a different request",
        ));
    }
    Ok(claim)
}

async fn publish(state: &AppState, bot: BotSummary) -> Result<(), ApiError> {
    persist_event(state, "bot_changed", ServerEventBody::BotChanged { bot })
        .await
        .map(|_| ())
        .map_err(|()| ApiError::internal())
}

pub(super) async fn summary(state: &AppState, bot: Bot) -> BotSummary {
    let provider = if let Some(profile_id) = bot.provider_profile_id {
        let exists =
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM provider_profiles WHERE id = ?")
                .bind(profile_id.to_string())
                .fetch_one(state.storage.pool())
                .await
                .unwrap_or(0)
                > 0;
        if exists {
            BotProviderStatus::Ready
        } else {
            BotProviderStatus::Unavailable
        }
    } else {
        BotProviderStatus::NotConfigured
    };
    BotSummary {
        id: bot.id.0,
        name: bot.name,
        title: bot.title,
        description: bot.description,
        shape: match bot.shape {
            DomainShape::Circle => BotShape::Circle,
            DomainShape::RoundedSquare => BotShape::RoundedSquare,
            DomainShape::Hexagon => BotShape::Hexagon,
        },
        color: match bot.color {
            DomainColor::Violet => BotColor::Violet,
            DomainColor::Blue => BotColor::Blue,
            DomainColor::Green => BotColor::Green,
            DomainColor::Orange => BotColor::Orange,
            DomainColor::Rose => BotColor::Rose,
            DomainColor::Slate => BotColor::Slate,
        },
        archived: bot.archived_at_ms.is_some(),
        unread_count: bot.unread_count,
        attention: match bot.attention {
            homebot_domain::BotAttention::None => BotAttention::None,
            homebot_domain::BotAttention::Working => BotAttention::Working,
            homebot_domain::BotAttention::NeedsApproval => BotAttention::NeedsApproval,
            homebot_domain::BotAttention::Failed => BotAttention::Failed,
        },
        provider,
        advanced: BotAdvancedSettings {
            provider_profile_id: bot.provider_profile_id,
            permission_profile: match bot.permission_profile {
                DomainPermission::ReadOnly => BotPermissionProfile::ReadOnly,
                DomainPermission::AskBeforeChanges => BotPermissionProfile::AskBeforeChanges,
                DomainPermission::Trusted => BotPermissionProfile::Trusted,
            },
        },
    }
}

fn to_domain_shape(value: BotShape) -> DomainShape {
    match value {
        BotShape::Circle => DomainShape::Circle,
        BotShape::RoundedSquare => DomainShape::RoundedSquare,
        BotShape::Hexagon => DomainShape::Hexagon,
    }
}

fn to_domain_color(value: BotColor) -> DomainColor {
    match value {
        BotColor::Violet => DomainColor::Violet,
        BotColor::Blue => DomainColor::Blue,
        BotColor::Green => DomainColor::Green,
        BotColor::Orange => DomainColor::Orange,
        BotColor::Rose => DomainColor::Rose,
        BotColor::Slate => DomainColor::Slate,
    }
}

fn to_domain_permission(value: BotPermissionProfile) -> DomainPermission {
    match value {
        BotPermissionProfile::ReadOnly => DomainPermission::ReadOnly,
        BotPermissionProfile::AskBeforeChanges => DomainPermission::AskBeforeChanges,
        BotPermissionProfile::Trusted => DomainPermission::Trusted,
    }
}

#[derive(Debug)]
pub(super) struct ApiError {
    status: StatusCode,
    envelope: ErrorEnvelope,
}

impl ApiError {
    fn conflict(message: &str) -> Self {
        Self::new(StatusCode::CONFLICT, ErrorCode::Conflict, message)
    }

    fn internal() -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::Internal,
            "HomeBot could not complete the request",
        )
    }

    fn new(status: StatusCode, code: ErrorCode, message: &str) -> Self {
        Self {
            status,
            envelope: ErrorEnvelope {
                code,
                message: message.to_owned(),
                retryable: false,
                request_id: None,
                retry_after_ms: None,
                details: None,
            },
        }
    }
}

impl From<DomainError> for ApiError {
    fn from(error: DomainError) -> Self {
        Self::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::ValidationFailed,
            &error.to_string(),
        )
    }
}

impl From<StorageError> for ApiError {
    fn from(error: StorageError) -> Self {
        match error {
            StorageError::BotNotFound => Self::new(
                StatusCode::NOT_FOUND,
                ErrorCode::NotFound,
                "Bot was not found",
            ),
            StorageError::DuplicateBotName => Self::conflict("A Bot with that name already exists"),
            StorageError::Domain(error) => error.into(),
            _ => Self::internal(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(self.envelope)).into_response()
    }
}
