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
    BotProviderStatus, BotResponse, BotShape, BotSummary, CreateBotRequest, DeleteBotRequest,
    ErrorCode, ErrorEnvelope, ServerEventBody, UpdateBotRequest,
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

async fn set_flag(
    state: &AppState,
    bot_id: Uuid,
    request: &BotMutationRequest,
    operation: &str,
    value: bool,
) -> Result<Json<BotResponse>, ApiError> {
    let replayed = matches!(
        claim(
            state,
            request.idempotency_key,
            &format!("{operation}:{bot_id}"),
            request,
        )
        .await?,
        IdempotencyClaim::Replayed { .. }
    );
    let bot = if replayed {
        let bot = state.storage.get_bot(state.owner_id, bot_id).await?;
        let matches = if operation == "pin_bot" {
            bot.pinned_at_ms.is_some() == value
        } else {
            bot.hidden_at_ms.is_some() == value
        };
        if !matches {
            return Err(ApiError::conflict(
                "The original Bot mutation is not reflected in current state",
            ));
        }
        bot
    } else if operation == "pin_bot" {
        state
            .storage
            .set_bot_pinned(state.owner_id, bot_id, value, unix_time_ms())
            .await?
    } else {
        state
            .storage
            .set_bot_hidden(state.owner_id, bot_id, value, unix_time_ms())
            .await?
    };
    let bot = summary(state, bot).await;
    if !replayed {
        publish(state, bot.clone()).await?;
    }
    Ok(Json(BotResponse { bot }))
}

pub(super) async fn pin(
    State(state): State<AppState>,
    Path(bot_id): Path<Uuid>,
    Json(request): Json<BotMutationRequest>,
) -> Result<Json<BotResponse>, ApiError> {
    set_flag(&state, bot_id, &request, "pin_bot", true).await
}

pub(super) async fn unpin(
    State(state): State<AppState>,
    Path(bot_id): Path<Uuid>,
    Json(request): Json<BotMutationRequest>,
) -> Result<Json<BotResponse>, ApiError> {
    set_flag(&state, bot_id, &request, "pin_bot", false).await
}

pub(super) async fn hide(
    State(state): State<AppState>,
    Path(bot_id): Path<Uuid>,
    Json(request): Json<BotMutationRequest>,
) -> Result<Json<BotResponse>, ApiError> {
    set_flag(&state, bot_id, &request, "hide_bot", true).await
}

pub(super) async fn unhide(
    State(state): State<AppState>,
    Path(bot_id): Path<Uuid>,
    Json(request): Json<BotMutationRequest>,
) -> Result<Json<BotResponse>, ApiError> {
    set_flag(&state, bot_id, &request, "hide_bot", false).await
}

pub(super) async fn delete(
    State(state): State<AppState>,
    Path(bot_id): Path<Uuid>,
    Json(request): Json<DeleteBotRequest>,
) -> Result<StatusCode, ApiError> {
    let existing = state.storage.get_bot(state.owner_id, bot_id).await;
    if let Ok(bot) = &existing
        && request.confirm_name != bot.name
    {
        return Err(ApiError::validation("Bot name confirmation did not match"));
    }
    if matches!(existing, Err(StorageError::BotNotFound)) {
        let prior: i64 =
            sqlx::query_scalar("SELECT count(*) FROM idempotency_records WHERE key = ?")
                .bind(request.idempotency_key.to_string())
                .fetch_one(state.storage.pool())
                .await
                .map_err(|_| ApiError::internal())?;
        if prior == 0 {
            return Err(StorageError::BotNotFound.into());
        }
    }
    let replayed = matches!(
        claim(
            &state,
            request.idempotency_key,
            &format!("delete_bot:{bot_id}"),
            &request
        )
        .await?,
        IdempotencyClaim::Replayed { .. }
    );
    if replayed {
        return Ok(StatusCode::NO_CONTENT);
    }
    existing?;
    state.storage.delete_bot(state.owner_id, bot_id).await?;
    persist_event(
        &state,
        "bot_deleted",
        ServerEventBody::BotDeleted { bot_id },
    )
    .await
    .map_err(|()| ApiError::internal())?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn duplicate(
    State(state): State<AppState>,
    Path(bot_id): Path<Uuid>,
    Json(request): Json<BotMutationRequest>,
) -> Result<(StatusCode, Json<BotResponse>), ApiError> {
    let replayed = matches!(
        claim(
            &state,
            request.idempotency_key,
            &format!("duplicate_bot:{bot_id}"),
            &request
        )
        .await?,
        IdempotencyClaim::Replayed { .. }
    );
    let bot = if replayed {
        state
            .storage
            .get_bot(state.owner_id, request.idempotency_key)
            .await?
    } else {
        state
            .storage
            .duplicate_bot_configuration(
                state.owner_id,
                bot_id,
                request.idempotency_key,
                unix_time_ms(),
            )
            .await?
    };
    let bot = summary(&state, bot).await;
    if !replayed {
        publish(&state, bot.clone()).await?;
    }
    Ok((
        if replayed {
            StatusCode::OK
        } else {
            StatusCode::CREATED
        },
        Json(BotResponse { bot }),
    ))
}

pub(super) async fn claim<T: Serialize>(
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
        pinned: bot.pinned_at_ms.is_some(),
        hidden: bot.hidden_at_ms.is_some(),
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
    pub(super) fn conflict(message: &str) -> Self {
        Self::new(StatusCode::CONFLICT, ErrorCode::Conflict, message)
    }

    pub(super) fn forbidden(message: &str) -> Self {
        Self::new(StatusCode::FORBIDDEN, ErrorCode::Forbidden, message)
    }

    pub(super) fn rate_limited(message: &str, retry_after_ms: u64) -> Self {
        let mut error = Self::new(
            StatusCode::TOO_MANY_REQUESTS,
            ErrorCode::RateLimited,
            message,
        );
        error.envelope.retry_after_ms = Some(retry_after_ms);
        error
    }

    pub(super) fn internal() -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::Internal,
            "HomeBot could not complete the request",
        )
    }

    pub(super) fn validation(message: &str) -> Self {
        Self::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::ValidationFailed,
            message,
        )
    }

    pub(super) fn unavailable(message: &str) -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::ProviderUnavailable,
            message,
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
            StorageError::BotNotFound => not_found("Bot"),
            StorageError::DuplicateBotName => Self::conflict("A Bot with that name already exists"),
            StorageError::ChatNotFound => not_found("Chat"),
            StorageError::MessageNotFound => not_found("Message"),
            StorageError::ApprovalNotFound => Self::new(
                StatusCode::CONFLICT,
                ErrorCode::Conflict,
                "Approval was not found or is no longer pending",
            ),
            StorageError::InvalidGroupParticipants => Self::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                ErrorCode::ValidationFailed,
                "Group chat requires at least three distinct active Bots",
            ),
            StorageError::CoordinationLimitReached => {
                Self::conflict("Group coordination limit was reached or the group was stopped")
            }
            StorageError::InvalidOwnershipHandoff => {
                Self::conflict("Group ownership handoff is no longer valid")
            }
            StorageError::AttachmentUnavailable => Self::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                ErrorCode::ValidationFailed,
                "An attachment is unavailable",
            ),
            StorageError::SecretNotFound => Self::new(
                StatusCode::NOT_FOUND,
                ErrorCode::NotFound,
                "Secret was not found",
            ),
            StorageError::DuplicateSecretLabel => {
                Self::conflict("A secret with that label already exists")
            }
            StorageError::PluginNotFound => Self::new(
                StatusCode::NOT_FOUND,
                ErrorCode::NotFound,
                "Plugin was not found",
            ),
            StorageError::DuplicatePluginName => {
                Self::conflict("A plugin with that name already exists")
            }
            StorageError::SkillNotFound => Self::new(
                StatusCode::NOT_FOUND,
                ErrorCode::NotFound,
                "Skill was not found",
            ),
            StorageError::DuplicateSkillName => {
                Self::conflict("A Skill with that name already exists")
            }
            StorageError::WorkspaceNotFound => Self::new(
                StatusCode::NOT_FOUND,
                ErrorCode::NotFound,
                "Workspace was not found",
            ),
            StorageError::DuplicateWorkspacePath => {
                Self::conflict("This repository is already registered")
            }
            StorageError::DuplicateChatWorkspace => {
                Self::conflict("This chat already has a workspace")
            }
            StorageError::CheckpointNotFound => Self::new(
                StatusCode::NOT_FOUND,
                ErrorCode::NotFound,
                "Checkpoint was not found",
            ),
            StorageError::WorkingContextBusy => working_context_busy(),
            error @ (StorageError::PairingNotFound
            | StorageError::PairingExpired
            | StorageError::PairingConsumed
            | StorageError::PairingOriginMismatch
            | StorageError::PairingRateLimited
            | StorageError::DeviceSessionNotFound) => pairing_storage_error(&error),
            StorageError::RoutineNotFound => Self::new(
                StatusCode::NOT_FOUND,
                ErrorCode::NotFound,
                "Routine was not found",
            ),
            StorageError::DuplicateRoutineName => {
                Self::conflict("A routine with that name already exists")
            }
            StorageError::RoutineRecordingNotFound => Self::new(
                StatusCode::CONFLICT,
                ErrorCode::Conflict,
                "Routine recording was not found or is no longer active",
            ),
            StorageError::ChatDomain(error) => Self::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                ErrorCode::ValidationFailed,
                &error.to_string(),
            ),
            StorageError::Domain(error) => error.into(),
            _ => Self::internal(),
        }
    }
}

fn not_found(entity: &str) -> ApiError {
    ApiError::new(
        StatusCode::NOT_FOUND,
        ErrorCode::NotFound,
        &format!("{entity} was not found"),
    )
}

fn pairing_storage_error(error: &StorageError) -> ApiError {
    match error {
        StorageError::PairingNotFound => ApiError::new(
            StatusCode::UNAUTHORIZED,
            ErrorCode::Unauthenticated,
            "The pairing credential is invalid",
        ),
        StorageError::PairingExpired => ApiError::new(
            StatusCode::GONE,
            ErrorCode::Conflict,
            "The pairing credential expired",
        ),
        StorageError::PairingConsumed => {
            ApiError::conflict("The pairing credential was already used")
        }
        StorageError::PairingOriginMismatch => {
            ApiError::forbidden("The request origin does not match the pairing endpoint")
        }
        StorageError::PairingRateLimited => {
            ApiError::rate_limited("Too many pairing attempts", 60_000)
        }
        StorageError::DeviceSessionNotFound => ApiError::new(
            StatusCode::NOT_FOUND,
            ErrorCode::NotFound,
            "Device session was not found",
        ),
        _ => ApiError::internal(),
    }
}

fn working_context_busy() -> ApiError {
    ApiError::conflict("A working-context operation is already running")
}

impl From<homebot_secrets::SecretStoreError> for ApiError {
    fn from(error: homebot_secrets::SecretStoreError) -> Self {
        match error {
            homebot_secrets::SecretStoreError::Locked => Self::new(
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorCode::SecretStoreLocked,
                "Unlock the operating-system credential store and try again",
            ),
            homebot_secrets::SecretStoreError::Unavailable => Self::new(
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorCode::SecretStoreUnavailable,
                "The operating-system credential store is unavailable",
            ),
            homebot_secrets::SecretStoreError::NotFound => Self::new(
                StatusCode::NOT_FOUND,
                ErrorCode::NotFound,
                "Secret value was not found in the operating-system credential store",
            ),
            homebot_secrets::SecretStoreError::InvalidReference => Self::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                ErrorCode::ValidationFailed,
                "Secret reference is invalid",
            ),
            homebot_secrets::SecretStoreError::OperationFailed => Self::internal(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(self.envelope)).into_response()
    }
}
