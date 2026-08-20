//! Authenticated, bounded attachment transport and content-addressed storage.

use super::{AppState, unix_time_ms};
use axum::{
    Json,
    body::Body,
    extract::{Path as AxumPath, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use futures_util::StreamExt;
use homebot_protocol::{
    Attachment, CreateAttachmentRequest, CreateAttachmentResponse, ErrorCode, ErrorEnvelope,
    FinalizeAttachmentRequest,
};
use homebot_storage::{AttachmentClaim, AttachmentRecord, IdempotencyClaim};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

const ATTACHMENT_TTL_MS: i64 = 15 * 60 * 1_000;
const MAX_ATTACHMENT_BYTES: u64 = 25 * 1024 * 1024;

pub(super) async fn create_attachment(
    State(state): State<AppState>,
    Json(mut request): Json<CreateAttachmentRequest>,
) -> Response {
    request.filename = request.filename.trim().to_owned();
    request.media_type = request.media_type.trim().to_ascii_lowercase();
    request.sha256 = request.sha256.to_ascii_lowercase();
    if let Err(message) = validate_attachment_request(&request) {
        return api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::ValidationFailed,
            message,
            Some(request.request_id),
        );
    }
    let hash_material = json!({
        "filename": request.filename,
        "media_type": request.media_type,
        "size_bytes": request.size_bytes,
        "sha256": request.sha256,
    });
    let Ok(encoded) = serde_json::to_vec(&hash_material) else {
        return internal_error(Some(request.request_id));
    };
    let request_hash = format!("{:x}", Sha256::digest(encoded));
    let now = unix_time_ms();
    let proposed = AttachmentRecord {
        id: Uuid::now_v7(),
        owner_id: state.owner_id,
        filename: request.filename,
        media_type: request.media_type,
        size_bytes: request.size_bytes,
        sha256: request.sha256,
        storage_path: None,
        status: "pending".to_owned(),
        expires_at_ms: now.saturating_add(ATTACHMENT_TTL_MS),
        created_at_ms: now,
        finalized_at_ms: None,
    };
    let claim = state
        .storage
        .claim_attachment_create(request.idempotency_key, &request_hash, &proposed)
        .await;
    let record = match claim {
        Ok(AttachmentClaim::Claimed(record) | AttachmentClaim::Replayed(record)) => record,
        Ok(AttachmentClaim::Conflict) => {
            return api_error(
                StatusCode::CONFLICT,
                ErrorCode::Conflict,
                "Idempotency key was already used for a different attachment".to_owned(),
                Some(request.request_id),
            );
        }
        Err(_) => return internal_error(Some(request.request_id)),
    };
    let expires_at_unix_ms = u64::try_from(record.expires_at_ms).unwrap_or(0);
    (
        StatusCode::CREATED,
        Json(CreateAttachmentResponse {
            attachment_id: record.id,
            upload_url: format!("/api/v1/attachments/{}/content", record.id),
            expires_at_unix_ms,
        }),
    )
        .into_response()
}

pub(super) async fn upload_attachment(
    State(state): State<AppState>,
    AxumPath(attachment_id): AxumPath<Uuid>,
    body: Body,
) -> Response {
    let record = match state
        .storage
        .attachment(state.owner_id, attachment_id)
        .await
    {
        Ok(Some(record)) => record,
        Ok(None) => {
            return api_error(
                StatusCode::NOT_FOUND,
                ErrorCode::NotFound,
                "Attachment was not found".to_owned(),
                None,
            );
        }
        Err(_) => return internal_error(None),
    };
    if record.status != "pending" {
        return api_error(
            StatusCode::CONFLICT,
            ErrorCode::Conflict,
            "Attachment is not pending upload".to_owned(),
            None,
        );
    }
    if record.expires_at_ms < unix_time_ms() {
        return api_error(
            StatusCode::GONE,
            ErrorCode::NotFound,
            "Attachment upload expired".to_owned(),
            None,
        );
    }

    let partials = state.artifact_root.join("partials");
    if tokio::fs::create_dir_all(&partials).await.is_err() {
        return internal_error(None);
    }
    let temporary = partials.join(format!("{}.{}.upload", attachment_id, Uuid::now_v7()));
    let partial = partial_path(&state.artifact_root, record.id);
    let Ok(mut file) = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .await
    else {
        return internal_error(None);
    };
    let mut stream = body.into_data_stream();
    let mut digest = Sha256::new();
    let mut received = 0_u64;
    while let Some(chunk) = stream.next().await {
        let Ok(chunk) = chunk else {
            remove_file_if_present(&temporary).await;
            return api_error(
                StatusCode::BAD_REQUEST,
                ErrorCode::ValidationFailed,
                "Attachment upload stream was interrupted".to_owned(),
                None,
            );
        };
        received = received.saturating_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX));
        if received > record.size_bytes || received > MAX_ATTACHMENT_BYTES {
            remove_file_if_present(&temporary).await;
            return api_error(
                StatusCode::PAYLOAD_TOO_LARGE,
                ErrorCode::ValidationFailed,
                "Attachment exceeds its declared or maximum size".to_owned(),
                None,
            );
        }
        digest.update(&chunk);
        if file.write_all(&chunk).await.is_err() {
            remove_file_if_present(&temporary).await;
            return internal_error(None);
        }
    }
    let actual_sha = format!("{:x}", digest.finalize());
    if received != record.size_bytes || actual_sha != record.sha256 {
        remove_file_if_present(&temporary).await;
        return api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::ValidationFailed,
            "Attachment size or SHA-256 digest does not match its declaration".to_owned(),
            None,
        );
    }
    if file.flush().await.is_err() || file.sync_all().await.is_err() {
        remove_file_if_present(&temporary).await;
        return internal_error(None);
    }
    drop(file);
    remove_file_if_present(&partial).await;
    if tokio::fs::rename(&temporary, &partial).await.is_err() {
        remove_file_if_present(&temporary).await;
        return internal_error(None);
    }
    StatusCode::NO_CONTENT.into_response()
}

pub(super) async fn finalize_attachment(
    State(state): State<AppState>,
    AxumPath(attachment_id): AxumPath<Uuid>,
    Json(mut request): Json<FinalizeAttachmentRequest>,
) -> Response {
    request.sha256 = request.sha256.to_ascii_lowercase();
    if !is_sha256(&request.sha256) {
        return api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::ValidationFailed,
            "SHA-256 must be 64 hexadecimal characters".to_owned(),
            Some(request.request_id),
        );
    }
    let request_hash = format!(
        "{:x}",
        Sha256::digest(format!("finalize:{attachment_id}:{}", request.sha256).as_bytes())
    );
    match state
        .storage
        .claim_idempotency(
            request.idempotency_key,
            &request_hash,
            attachment_id,
            unix_time_ms(),
        )
        .await
    {
        Ok(IdempotencyClaim::Claimed { .. } | IdempotencyClaim::Replayed { .. }) => {}
        Ok(IdempotencyClaim::Conflict) => {
            return api_error(
                StatusCode::CONFLICT,
                ErrorCode::Conflict,
                "Idempotency key was already used for a different finalization".to_owned(),
                Some(request.request_id),
            );
        }
        Err(_) => return internal_error(Some(request.request_id)),
    }
    let record = match state
        .storage
        .attachment(state.owner_id, attachment_id)
        .await
    {
        Ok(Some(record)) => record,
        Ok(None) => {
            return api_error(
                StatusCode::NOT_FOUND,
                ErrorCode::NotFound,
                "Attachment was not found".to_owned(),
                Some(request.request_id),
            );
        }
        Err(_) => return internal_error(Some(request.request_id)),
    };
    if request.sha256 != record.sha256 {
        return api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::ValidationFailed,
            "Finalization digest does not match attachment metadata".to_owned(),
            Some(request.request_id),
        );
    }
    if record.status == "ready" {
        return Json(protocol_attachment(&record)).into_response();
    }
    match complete_pending_attachment(&state, record, request.request_id).await {
        Ok(record) => Json(protocol_attachment(&record)).into_response(),
        Err(response) => response,
    }
}

async fn complete_pending_attachment(
    state: &AppState,
    record: AttachmentRecord,
    request_id: Uuid,
) -> Result<AttachmentRecord, Response> {
    if record.status != "pending" || record.expires_at_ms < unix_time_ms() {
        return Err(api_error(
            StatusCode::GONE,
            ErrorCode::NotFound,
            "Attachment upload expired".to_owned(),
            Some(request_id),
        ));
    }
    let partial = partial_path(&state.artifact_root, record.id);
    let Ok((actual_size, actual_sha)) = digest_file(&partial).await else {
        return Err(api_error(
            StatusCode::CONFLICT,
            ErrorCode::Conflict,
            "Attachment content has not been uploaded".to_owned(),
            Some(request_id),
        ));
    };
    if actual_size != record.size_bytes || actual_sha != record.sha256 {
        return Err(api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::ValidationFailed,
            "Stored attachment failed final integrity verification".to_owned(),
            Some(request_id),
        ));
    }
    let relative = PathBuf::from("objects")
        .join(&record.sha256[..2])
        .join(&record.sha256);
    let target = state.artifact_root.join(&relative);
    if let Some(parent) = target.parent()
        && tokio::fs::create_dir_all(parent).await.is_err()
    {
        return Err(internal_error(Some(request_id)));
    }
    if tokio::fs::try_exists(&target).await.unwrap_or(false) {
        let Ok((existing_size, existing_sha)) = digest_file(&target).await else {
            return Err(internal_error(Some(request_id)));
        };
        if existing_size != record.size_bytes || existing_sha != record.sha256 {
            return Err(internal_error(Some(request_id)));
        }
        remove_file_if_present(&partial).await;
    } else if tokio::fs::rename(&partial, &target).await.is_err() {
        return Err(internal_error(Some(request_id)));
    }
    let Some(storage_path) = relative.to_str() else {
        return Err(internal_error(Some(request_id)));
    };
    match state
        .storage
        .mark_attachment_ready(state.owner_id, record.id, storage_path, unix_time_ms())
        .await
    {
        Ok(true) => {}
        Ok(false) | Err(_) => return Err(internal_error(Some(request_id))),
    }
    Ok(AttachmentRecord {
        status: "ready".to_owned(),
        storage_path: Some(storage_path.to_owned()),
        ..record
    })
}

fn validate_attachment_request(request: &CreateAttachmentRequest) -> Result<(), String> {
    if request.filename.is_empty()
        || request.filename.len() > 255
        || request.filename.contains(['/', '\\', '\0'])
    {
        return Err("Filename must be a safe, non-empty basename".to_owned());
    }
    if request.media_type.len() > 127
        || !request.media_type.is_ascii()
        || !request.media_type.contains('/')
    {
        return Err("Media type must be a valid ASCII type/subtype".to_owned());
    }
    if request.size_bytes > MAX_ATTACHMENT_BYTES {
        return Err(format!(
            "Attachment exceeds the {MAX_ATTACHMENT_BYTES}-byte limit"
        ));
    }
    if !is_sha256(&request.sha256) {
        return Err("SHA-256 must be 64 hexadecimal characters".to_owned());
    }
    Ok(())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn partial_path(root: &Path, attachment_id: Uuid) -> PathBuf {
    root.join("partials")
        .join(format!("{attachment_id}.partial"))
}

async fn digest_file(path: &Path) -> Result<(u64, String), std::io::Error> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut buffer = vec![0_u8; 16 * 1024];
    let mut digest = Sha256::new();
    let mut size = 0_u64;
    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        size = size.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
        digest.update(&buffer[..read]);
    }
    Ok((size, format!("{:x}", digest.finalize())))
}

async fn remove_file_if_present(path: &Path) {
    if let Err(error) = tokio::fs::remove_file(path).await
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(path = %path.display(), "failed to remove owned attachment staging file");
    }
}

fn protocol_attachment(record: &AttachmentRecord) -> Attachment {
    Attachment {
        id: record.id,
        filename: record.filename.clone(),
        media_type: record.media_type.clone(),
        size_bytes: record.size_bytes,
        sha256: record.sha256.clone(),
    }
}

fn api_error(
    status: StatusCode,
    code: ErrorCode,
    message: String,
    request_id: Option<Uuid>,
) -> Response {
    (
        status,
        Json(ErrorEnvelope {
            code,
            message,
            retryable: false,
            request_id,
            retry_after_ms: None,
            details: None,
        }),
    )
        .into_response()
}

fn internal_error(request_id: Option<Uuid>) -> Response {
    api_error(
        StatusCode::INTERNAL_SERVER_ERROR,
        ErrorCode::Internal,
        "HomeBot could not complete the request".to_owned(),
        request_id,
    )
}
