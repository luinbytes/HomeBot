//! Authoritative authenticated HTTP and WebSocket transport.

use axum::{
    Json, Router,
    extract::{
        Request, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use futures_util::StreamExt;
use homebot_protocol::{
    ClientMessage, ProtocolRange, ResumeDisposition, ServerEvent, ServerEventBody, Snapshot,
};
use homebot_storage::Storage;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct AppState {
    storage: Storage,
    bearer_digest: [u8; 32],
    owner_id: Uuid,
}

impl AppState {
    #[must_use]
    pub fn new(storage: Storage, bearer_token: &str) -> Self {
        Self {
            storage,
            bearer_digest: Sha256::digest(bearer_token.as_bytes()).into(),
            owner_id: Uuid::nil(),
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
        .route("/api/v1/events", get(events_socket))
        .route_layer(middleware::from_fn_with_state(state.clone(), authenticate));
    Router::new()
        .route("/health", get(health))
        .merge(authenticated)
        .with_state(state)
}

async fn events_socket(State(state): State<AppState>, upgrade: WebSocketUpgrade) -> Response {
    upgrade.on_upgrade(move |socket| serve_events(socket, state))
}

async fn serve_events(mut socket: WebSocket, state: AppState) {
    let Some(Ok(Message::Text(text))) = socket.next().await else {
        return;
    };
    let Ok(ClientMessage::Hello {
        protocol_version,
        resume_after,
        ..
    }) = serde_json::from_str(&text)
    else {
        return;
    };
    if let Err(error) = homebot_protocol::check_compatibility(protocol_version) {
        if let Ok(encoded) = serde_json::to_string(&error) {
            let _ = socket.send(Message::Text(encoded.into())).await;
        }
        return;
    }

    let cursor = resume_after.unwrap_or(0);
    let retained = if resume_after.is_some() {
        state
            .storage
            .events_after(state.owner_id, cursor, 1_000)
            .await
            .ok()
    } else {
        None
    };
    let disposition = if retained.is_some() {
        ResumeDisposition::Replayed
    } else {
        ResumeDisposition::SnapshotRequired
    };
    let hello = ServerEvent {
        protocol_version: homebot_protocol::PROTOCOL_VERSION,
        sequence: cursor,
        event_id: Uuid::now_v7(),
        body: ServerEventBody::Hello {
            server_version: env!("CARGO_PKG_VERSION").to_owned(),
            supported_protocols: ProtocolRange {
                minimum: homebot_protocol::MIN_COMPATIBLE_PROTOCOL_VERSION,
                maximum: homebot_protocol::PROTOCOL_VERSION,
            },
            resume: disposition,
            heartbeat_interval_ms: 15_000,
            heartbeat_timeout_ms: 45_000,
        },
    };
    if send_json(&mut socket, &hello).await.is_err() {
        return;
    }
    if let Some(events) = retained {
        for event in events {
            if send_json(&mut socket, &event.payload).await.is_err() {
                return;
            }
        }
    } else {
        let boundary = state
            .storage
            .latest_sequence(state.owner_id)
            .await
            .unwrap_or(0);
        let snapshot = ServerEvent {
            protocol_version: homebot_protocol::PROTOCOL_VERSION,
            sequence: boundary,
            event_id: Uuid::now_v7(),
            body: ServerEventBody::Snapshot {
                boundary_sequence: boundary,
                snapshot: Snapshot::default(),
            },
        };
        let _ = send_json(&mut socket, &snapshot).await;
    }
}

async fn send_json(socket: &mut WebSocket, value: &impl serde::Serialize) -> Result<(), ()> {
    let encoded = serde_json::to_string(value).map_err(|_| ())?;
    socket
        .send(Message::Text(encoded.into()))
        .await
        .map_err(|_| ())
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
    use futures_util::{SinkExt, StreamExt};
    use tokio::task::JoinHandle;
    use tokio_tungstenite::{connect_async, tungstenite::client::IntoClientRequest};
    use tower::ServiceExt;

    async fn test_app() -> Result<Router, homebot_storage::StorageError> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("homebot.db");
        let storage = Storage::open(&path).await?;
        Ok(router(AppState::new(storage, "correct-token")))
    }

    async fn spawn_app(
        storage: Storage,
    ) -> Result<(std::net::SocketAddr, JoinHandle<()>, tempfile::TempDir), Box<dyn std::error::Error>>
    {
        let directory = tempfile::tempdir()?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router(AppState::new(storage, "correct-token"))).await;
        });
        Ok((address, task, directory))
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

    #[tokio::test]
    async fn websocket_requires_auth_and_sends_snapshot_after_hello()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let storage = Storage::open(&directory.path().join("homebot.db")).await?;
        let (address, task, _guard) = spawn_app(storage).await?;
        let url = format!("ws://{address}/api/v1/events");
        let unauthorised = connect_async(&url).await;
        assert!(
            matches!(unauthorised, Err(tokio_tungstenite::tungstenite::Error::Http(ref response)) if response.status() == StatusCode::UNAUTHORIZED)
        );

        let mut request = url.into_client_request()?;
        request
            .headers_mut()
            .insert("authorization", "Bearer correct-token".parse()?);
        let (mut socket, _) = connect_async(request).await?;
        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                serde_json::to_string(&ClientMessage::Hello {
                    protocol_version: homebot_protocol::PROTOCOL_VERSION,
                    client_version: "test".to_owned(),
                    device_session: "test-device".to_owned(),
                    resume_after: None,
                })?
                .into(),
            ))
            .await?;
        let hello: ServerEvent =
            serde_json::from_str(socket.next().await.ok_or("missing hello")??.to_text()?)?;
        let snapshot: ServerEvent =
            serde_json::from_str(socket.next().await.ok_or("missing snapshot")??.to_text()?)?;
        assert!(matches!(
            hello.body,
            ServerEventBody::Hello {
                resume: ResumeDisposition::SnapshotRequired,
                ..
            }
        ));
        assert!(matches!(
            snapshot.body,
            ServerEventBody::Snapshot {
                boundary_sequence: 0,
                ..
            }
        ));
        task.abort();
        Ok(())
    }

    #[tokio::test]
    async fn reconnect_replays_events_strictly_after_cursor()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let storage = Storage::open(&directory.path().join("homebot.db")).await?;
        let replay = ServerEvent {
            protocol_version: homebot_protocol::PROTOCOL_VERSION,
            sequence: 1,
            event_id: Uuid::now_v7(),
            body: ServerEventBody::Ping {
                nonce: Uuid::now_v7(),
            },
        };
        storage
            .append_event(Uuid::nil(), "ping", &serde_json::to_value(&replay)?, 1)
            .await?;
        let (address, task, _guard) = spawn_app(storage).await?;
        let mut request = format!("ws://{address}/api/v1/events").into_client_request()?;
        request
            .headers_mut()
            .insert("authorization", "Bearer correct-token".parse()?);
        let (mut socket, _) = connect_async(request).await?;
        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                serde_json::to_string(&ClientMessage::Hello {
                    protocol_version: homebot_protocol::PROTOCOL_VERSION,
                    client_version: "test".to_owned(),
                    device_session: "test-device".to_owned(),
                    resume_after: Some(0),
                })?
                .into(),
            ))
            .await?;
        let hello: ServerEvent =
            serde_json::from_str(socket.next().await.ok_or("missing hello")??.to_text()?)?;
        let replayed: ServerEvent =
            serde_json::from_str(socket.next().await.ok_or("missing replay")??.to_text()?)?;
        assert!(matches!(
            hello.body,
            ServerEventBody::Hello {
                resume: ResumeDisposition::Replayed,
                ..
            }
        ));
        assert_eq!(replayed, replay);
        task.abort();
        Ok(())
    }
}
