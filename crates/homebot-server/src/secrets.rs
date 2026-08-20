//! Authenticated secret metadata API backed by the operating-system credential store.

use super::{AppState, bots::ApiError, persist_event, unix_time_ms};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use homebot_protocol::{
    CreateSecretRequest, SecretStatus as ProtocolSecretStatus, SecretSummary, ServerEventBody,
    UpdateSecretRequest,
};
use homebot_secrets::{SecretInput, SecretStatus, SecretStoreError, locator_for};
use homebot_storage::SecretReferenceRecord;
use uuid::Uuid;

pub(super) async fn list(
    State(state): State<AppState>,
) -> Result<Json<Vec<SecretSummary>>, ApiError> {
    let records = state.storage.list_secret_references(state.owner_id).await?;
    let mut summaries = Vec::with_capacity(records.len());
    for record in records {
        summaries.push(summary(&state, &record).await);
    }
    Ok(Json(summaries))
}

pub(super) async fn create(
    State(state): State<AppState>,
    Json(request): Json<CreateSecretRequest>,
) -> Result<(StatusCode, Json<SecretSummary>), ApiError> {
    let label = validated_label(&request.label)?;
    if let Ok(existing) = state
        .storage
        .secret_reference(state.owner_id, request.idempotency_key)
        .await
    {
        return Ok((StatusCode::OK, Json(summary(&state, &existing).await)));
    }

    let now_ms = unix_time_ms();
    let record = SecretReferenceRecord {
        id: request.idempotency_key,
        owner_id: state.owner_id,
        locator: locator_for(request.idempotency_key),
        label,
        created_at_ms: now_ms,
        updated_at_ms: now_ms,
    };
    state
        .secret_vault
        .put(&record.locator, SecretInput::new(request.value))
        .await?;
    if let Err(error) = state.storage.create_secret_reference(&record).await {
        let _ = state.secret_vault.delete(&record.locator).await;
        return Err(error.into());
    }
    let secret = summary(&state, &record).await;
    publish_changed(&state, secret.clone()).await?;
    Ok((StatusCode::CREATED, Json(secret)))
}

pub(super) async fn update(
    State(state): State<AppState>,
    Path(secret_id): Path<Uuid>,
    Json(request): Json<UpdateSecretRequest>,
) -> Result<Json<SecretSummary>, ApiError> {
    if request.label.is_none() && request.value.is_none() {
        return Err(validation_error("Provide a new label, value, or both"));
    }
    let validated_label = request.label.as_deref().map(validated_label).transpose()?;
    let mut record = state
        .storage
        .secret_reference(state.owner_id, secret_id)
        .await?;
    if let Some(value) = request.value {
        state
            .secret_vault
            .put(&record.locator, SecretInput::new(value))
            .await?;
    }
    if let Some(label) = validated_label {
        record = state
            .storage
            .update_secret_reference(state.owner_id, secret_id, &label, unix_time_ms())
            .await?;
    }
    let secret = summary(&state, &record).await;
    publish_changed(&state, secret.clone()).await?;
    Ok(Json(secret))
}

pub(super) async fn delete(
    State(state): State<AppState>,
    Path(secret_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let record = state
        .storage
        .secret_reference(state.owner_id, secret_id)
        .await?;
    match state.secret_vault.delete(&record.locator).await {
        Ok(()) | Err(SecretStoreError::NotFound) => {}
        Err(error) => return Err(error.into()),
    }
    state
        .storage
        .delete_secret_reference(state.owner_id, secret_id)
        .await?;
    persist_event(
        &state,
        "secret_removed",
        ServerEventBody::SecretRemoved { secret_id },
    )
    .await
    .map_err(|()| ApiError::internal())?;
    Ok(StatusCode::NO_CONTENT)
}

async fn summary(state: &AppState, record: &SecretReferenceRecord) -> SecretSummary {
    let status = match state.secret_vault.status(&record.locator).await {
        SecretStatus::Ready => ProtocolSecretStatus::Ready,
        SecretStatus::Locked => ProtocolSecretStatus::Locked,
        SecretStatus::Unavailable => ProtocolSecretStatus::Unavailable,
        SecretStatus::Missing => ProtocolSecretStatus::Missing,
    };
    SecretSummary {
        id: record.id,
        label: record.label.clone(),
        status,
        created_at_unix_ms: u64::try_from(record.created_at_ms).unwrap_or_default(),
        updated_at_unix_ms: u64::try_from(record.updated_at_ms).unwrap_or_default(),
    }
}

async fn publish_changed(state: &AppState, secret: SecretSummary) -> Result<(), ApiError> {
    persist_event(
        state,
        "secret_changed",
        ServerEventBody::SecretChanged { secret },
    )
    .await
    .map(|_| ())
    .map_err(|()| ApiError::internal())
}

fn validated_label(value: &str) -> Result<String, ApiError> {
    let label = value.trim();
    if label.is_empty() || label.len() > 80 || label.chars().any(char::is_control) {
        return Err(validation_error(
            "Secret label must contain 1 to 80 visible characters",
        ));
    }
    Ok(label.to_owned())
}

fn validation_error(message: &str) -> ApiError {
    ApiError::validation(message)
}
