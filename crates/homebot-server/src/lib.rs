//! Authoritative authenticated HTTP and WebSocket transport.

use axum::{
    Json, Router,
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use homebot_storage::Storage;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

#[derive(Clone, Debug)]
pub struct AppState {
    storage: Storage,
    bearer_digest: [u8; 32],
}

impl AppState {
    #[must_use]
    pub fn new(storage: Storage, bearer_token: &str) -> Self {
        Self {
            storage,
            bearer_digest: Sha256::digest(bearer_token.as_bytes()).into(),
        }
    }

    #[must_use]
    pub fn storage(&self) -> &Storage {
        &self.storage
    }
}

pub fn router(state: AppState) -> Router {
    let authenticated = Router::new()
        .route("/api/v1/version", get(version))
        .route_layer(middleware::from_fn_with_state(state.clone(), authenticate));
    Router::new()
        .route("/health", get(health))
        .merge(authenticated)
        .with_state(state)
}

async fn health() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "service": "homebot-server",
        "protocol_version": homebot_protocol::PROTOCOL_VERSION
    }))
}

async fn version(headers: HeaderMap) -> Response {
    if let Some(protocol) = headers
        .get("x-homebot-protocol")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u16>().ok())
        && let Err(error) = homebot_protocol::check_compatibility(protocol)
    {
        return (StatusCode::UPGRADE_REQUIRED, Json(error)).into_response();
    }
    Json(json!({
        "protocol": {
            "minimum": homebot_protocol::MIN_COMPATIBLE_PROTOCOL_VERSION,
            "maximum": homebot_protocol::PROTOCOL_VERSION
        },
        "server_version": env!("CARGO_PKG_VERSION")
    }))
    .into_response()
}

async fn authenticate(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: Request,
    next: Next,
) -> Response {
    let supplied = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    let valid = supplied.is_some_and(|token| {
        let candidate: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        bool::from(candidate.ct_eq(&state.bearer_digest))
    });
    if !valid {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "code": "unauthenticated",
                "message": "A valid HomeBot device session is required",
                "retryable": false,
                "request_id": null
            })),
        )
            .into_response();
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    async fn test_app() -> Result<Router, homebot_storage::StorageError> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("homebot.db");
        let storage = Storage::open(&path).await?;
        Ok(router(AppState::new(storage, "correct-token")))
    }

    #[tokio::test]
    async fn health_is_available_without_auth_but_reveals_no_secrets()
    -> Result<(), Box<dyn std::error::Error>> {
        let response = test_app()
            .await?
            .oneshot(Request::get("/health").body(Body::empty())?)
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        Ok(())
    }

    #[tokio::test]
    async fn protected_routes_deny_missing_and_invalid_tokens_server_side()
    -> Result<(), Box<dyn std::error::Error>> {
        for authorization in [None, Some("Bearer wrong-token")] {
            let mut request = Request::get("/api/v1/version");
            if let Some(value) = authorization {
                request = request.header("authorization", value);
            }
            let response = test_app()
                .await?
                .oneshot(request.body(Body::empty())?)
                .await?;
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }
        Ok(())
    }

    #[tokio::test]
    async fn valid_device_session_can_negotiate_version() -> Result<(), Box<dyn std::error::Error>>
    {
        let response = test_app()
            .await?
            .oneshot(
                Request::get("/api/v1/version")
                    .header("authorization", "Bearer correct-token")
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        Ok(())
    }

    #[tokio::test]
    async fn stale_protocol_is_rejected_with_upgrade_required()
    -> Result<(), Box<dyn std::error::Error>> {
        let response = test_app()
            .await?
            .oneshot(
                Request::get("/api/v1/version")
                    .header("authorization", "Bearer correct-token")
                    .header("x-homebot-protocol", "0")
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(response.status(), StatusCode::UPGRADE_REQUIRED);
        Ok(())
    }
}
