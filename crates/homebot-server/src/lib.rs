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
    ClientMessage, ErrorCode, ErrorEnvelope, ProtocolRange, ResumeDisposition, ServerEvent,
    ServerEventBody, Snapshot,
};
use homebot_storage::{IdempotencyClaim, ReplayWindow, Storage};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use uuid::Uuid;

const HEARTBEAT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(15);
const HEARTBEAT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(45);

#[derive(Clone, Debug)]
pub struct AppState {
    storage: Storage,
    bearer_digest: [u8; 32],
    owner_id: Uuid,
    heartbeat_interval: std::time::Duration,
    heartbeat_timeout: std::time::Duration,
}

impl AppState {
    #[must_use]
    pub fn new(storage: Storage, bearer_token: &str) -> Self {
        Self {
            storage,
            bearer_digest: Sha256::digest(bearer_token.as_bytes()).into(),
            owner_id: Uuid::nil(),
            heartbeat_interval: HEARTBEAT_INTERVAL,
            heartbeat_timeout: HEARTBEAT_TIMEOUT,
        }
    }

    #[must_use]
    pub fn storage(&self) -> &Storage {
        &self.storage
    }

    #[must_use]
    pub fn with_heartbeat(
        mut self,
        interval: std::time::Duration,
        timeout: std::time::Duration,
    ) -> Self {
        self.heartbeat_interval = interval;
        self.heartbeat_timeout = timeout;
        self
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
    if !initial_sync(&mut socket, &state).await {
        return;
    }
    let mut heartbeat = tokio::time::interval(state.heartbeat_interval);
    heartbeat.tick().await;
    let mut last_pong = tokio::time::Instant::now();
    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                if last_pong.elapsed() >= state.heartbeat_timeout { return; }
                let sequence = state.storage.latest_sequence(state.owner_id).await.unwrap_or(0);
                let ping = ServerEvent {
                    protocol_version: homebot_protocol::PROTOCOL_VERSION,
                    sequence,
                    event_id: Uuid::now_v7(),
                    body: ServerEventBody::Ping { nonce: Uuid::now_v7() },
                };
                if send_json(&mut socket, &ping).await.is_err() { return; }
            }
            message = socket.next() => {
                let Some(Ok(Message::Text(text))) = message else { return; };
                if let Ok(message) = serde_json::from_str::<ClientMessage>(&text) {
                    if matches!(message, ClientMessage::Pong { .. }) { last_pong = tokio::time::Instant::now(); }
                    if !handle_client_message(&mut socket, &state, message).await { return; }
                }
            }
        }
    }
}

async fn initial_sync(socket: &mut WebSocket, state: &AppState) -> bool {
    let Some(Ok(Message::Text(text))) = socket.next().await else {
        return false;
    };
    let Ok(ClientMessage::Hello {
        protocol_version,
        resume_after,
        ..
    }) = serde_json::from_str(&text)
    else {
        return false;
    };
    if let Err(error) = homebot_protocol::check_compatibility(protocol_version) {
        if let Ok(encoded) = serde_json::to_string(&error) {
            let _ = socket.send(Message::Text(encoded.into())).await;
        }
        return false;
    }

    let cursor = resume_after.unwrap_or(0);
    let replay = if resume_after.is_some() {
        match state
            .storage
            .replay_after(state.owner_id, cursor, 1_000)
            .await
        {
            Ok(window @ ReplayWindow::Available(_)) => Some(window),
            Ok(ReplayWindow::Unavailable) | Err(_) => None,
        }
    } else {
        None
    };
    let disposition = if replay.is_some() {
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
            heartbeat_interval_ms: u32::try_from(state.heartbeat_interval.as_millis())
                .unwrap_or(u32::MAX),
            heartbeat_timeout_ms: u32::try_from(state.heartbeat_timeout.as_millis())
                .unwrap_or(u32::MAX),
        },
    };
    if send_json(socket, &hello).await.is_err() {
        return false;
    }
    if let Some(ReplayWindow::Available(mut events)) = replay {
        loop {
            let batch_is_full = events.len() == 1_000;
            let mut next_cursor = cursor;
            for event in events {
                next_cursor = event.sequence;
                if send_json(socket, &event.payload).await.is_err() {
                    return false;
                }
            }
            if !batch_is_full {
                break;
            }
            events = match state
                .storage
                .replay_after(state.owner_id, next_cursor, 1_000)
                .await
            {
                Ok(ReplayWindow::Available(events)) => events,
                Ok(ReplayWindow::Unavailable) | Err(_) => return false,
            };
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
        if send_json(socket, &snapshot).await.is_err() {
            return false;
        }
    }
    true
}

#[allow(clippy::too_many_lines)] // Command lifecycle stays together until operation executors land.
async fn handle_client_message(
    socket: &mut WebSocket,
    state: &AppState,
    message: ClientMessage,
) -> bool {
    match message {
        ClientMessage::Command {
            request_id,
            idempotency_key,
            command,
        } => {
            let Ok(encoded) = serde_json::to_vec(&command) else {
                return true;
            };
            let request_hash = format!("{:x}", Sha256::digest(encoded));
            let proposed = Uuid::now_v7();
            let claim = state
                .storage
                .claim_idempotency(idempotency_key, &request_hash, proposed, unix_time_ms())
                .await;
            let Ok(claim) = claim else {
                return true;
            };
            match claim {
                IdempotencyClaim::Claimed { operation_id }
                | IdempotencyClaim::Replayed { operation_id } => {
                    let accepted = persist_event(
                        state,
                        "command_accepted",
                        ServerEventBody::CommandAccepted {
                            request_id,
                            operation_id,
                        },
                    )
                    .await;
                    if let Ok(event) = accepted {
                        if send_json(socket, &event).await.is_err() {
                            return false;
                        }
                    }
                    if matches!(claim, IdempotencyClaim::Claimed { .. }) {
                        let completed = persist_event(
                            state,
                            "command_completed",
                            ServerEventBody::CommandCompleted {
                                request_id,
                                operation_id,
                                result: json!({"status":"accepted"}),
                            },
                        )
                        .await;
                        if let Ok(event) = completed {
                            if send_json(socket, &event).await.is_err() {
                                return false;
                            }
                        }
                    }
                }
                IdempotencyClaim::Conflict => {
                    let failed = persist_event(
                        state,
                        "command_failed",
                        ServerEventBody::CommandFailed {
                            request_id,
                            operation_id: proposed,
                            error: ErrorEnvelope {
                                code: ErrorCode::Conflict,
                                message: "Idempotency key was already used for a different command"
                                    .to_owned(),
                                retryable: false,
                                request_id: Some(request_id),
                                retry_after_ms: None,
                                details: None,
                            },
                        },
                    )
                    .await;
                    if let Ok(event) = failed {
                        if send_json(socket, &event).await.is_err() {
                            return false;
                        }
                    }
                }
            }
        }
        ClientMessage::Cancel {
            request_id,
            operation_id,
        } => {
            let cancelled = persist_event(
                state,
                "command_cancelled",
                ServerEventBody::CommandCancelled {
                    request_id,
                    operation_id,
                },
            )
            .await;
            if let Ok(event) = cancelled {
                if send_json(socket, &event).await.is_err() {
                    return false;
                }
            }
        }
        ClientMessage::Hello { .. } | ClientMessage::Pong { .. } => {}
    }
    true
}

async fn persist_event(
    state: &AppState,
    kind: &str,
    body: ServerEventBody,
) -> Result<ServerEvent, ()> {
    let event = ServerEvent {
        protocol_version: homebot_protocol::PROTOCOL_VERSION,
        sequence: 0,
        event_id: Uuid::now_v7(),
        body,
    };
    let payload = serde_json::to_value(&event).map_err(|_| ())?;
    let stored = state
        .storage
        .append_event(state.owner_id, kind, &payload, unix_time_ms())
        .await
        .map_err(|_| ())?;
    serde_json::from_value(stored.payload).map_err(|_| ())
}

fn unix_time_ms() -> i64 {
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
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
        let (address, task) = spawn_state(AppState::new(storage, "correct-token")).await?;
        Ok((address, task, directory))
    }

    async fn spawn_state(
        state: AppState,
    ) -> Result<(std::net::SocketAddr, JoinHandle<()>), Box<dyn std::error::Error>> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router(state)).await;
        });
        Ok((address, task))
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

    #[tokio::test]
    async fn reconnect_uses_snapshot_when_cursor_falls_outside_retention()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let storage = Storage::open(&directory.path().join("homebot.db")).await?;
        let replay = ServerEvent {
            protocol_version: homebot_protocol::PROTOCOL_VERSION,
            sequence: 0,
            event_id: Uuid::now_v7(),
            body: ServerEventBody::Ping {
                nonce: Uuid::now_v7(),
            },
        };
        storage
            .append_event(Uuid::nil(), "ping", &serde_json::to_value(&replay)?, 1)
            .await?;
        storage.prune_events_through(Uuid::nil(), 1, 2).await?;
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
        let snapshot: ServerEvent =
            serde_json::from_str(socket.next().await.ok_or("missing snapshot")??.to_text()?)?;
        assert!(matches!(
            hello.body,
            ServerEventBody::Hello {
                resume: ResumeDisposition::SnapshotRequired,
                ..
            }
        ));
        assert!(matches!(snapshot.body, ServerEventBody::Snapshot { .. }));
        task.abort();
        Ok(())
    }

    #[tokio::test]
    async fn duplicate_command_reuses_operation_and_conflicting_key_fails()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let storage = Storage::open(&directory.path().join("homebot.db")).await?;
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
                    resume_after: None,
                })?
                .into(),
            ))
            .await?;
        let _hello = socket.next().await.ok_or("missing hello")??;
        let _snapshot = socket.next().await.ok_or("missing snapshot")??;
        let request_id = Uuid::now_v7();
        let key = Uuid::now_v7();
        let command = ClientMessage::Command {
            request_id,
            idempotency_key: key,
            command: homebot_protocol::Command::CreateBot {
                name: "Ada".to_owned(),
                title: "Engineer".to_owned(),
            },
        };
        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                serde_json::to_string(&command)?.into(),
            ))
            .await?;
        let accepted: ServerEvent =
            serde_json::from_str(socket.next().await.ok_or("missing accepted")??.to_text()?)?;
        let _completed: ServerEvent =
            serde_json::from_str(socket.next().await.ok_or("missing completed")??.to_text()?)?;
        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                serde_json::to_string(&command)?.into(),
            ))
            .await?;
        let replayed: ServerEvent =
            serde_json::from_str(socket.next().await.ok_or("missing replay")??.to_text()?)?;
        let ServerEventBody::CommandAccepted {
            operation_id: operation,
            ..
        } = accepted.body
        else {
            return Err("expected accepted event".into());
        };
        assert!(
            matches!(replayed.body, ServerEventBody::CommandAccepted { operation_id, .. } if operation_id == operation)
        );

        let conflict = ClientMessage::Command {
            request_id: Uuid::now_v7(),
            idempotency_key: key,
            command: homebot_protocol::Command::CreateBot {
                name: "Different".to_owned(),
                title: "Engineer".to_owned(),
            },
        };
        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                serde_json::to_string(&conflict)?.into(),
            ))
            .await?;
        let failed: ServerEvent =
            serde_json::from_str(socket.next().await.ok_or("missing conflict")??.to_text()?)?;
        assert!(matches!(
            failed.body,
            ServerEventBody::CommandFailed {
                error: ErrorEnvelope {
                    code: ErrorCode::Conflict,
                    ..
                },
                ..
            }
        ));
        task.abort();
        Ok(())
    }

    #[tokio::test]
    async fn heartbeat_closes_clients_that_never_pong() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let storage = Storage::open(&directory.path().join("homebot.db")).await?;
        let state = AppState::new(storage, "correct-token").with_heartbeat(
            std::time::Duration::from_millis(15),
            std::time::Duration::from_millis(45),
        );
        let (address, task) = spawn_state(state).await?;
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
                    resume_after: None,
                })?
                .into(),
            ))
            .await?;
        let _hello = socket.next().await.ok_or("missing hello")??;
        let _snapshot = socket.next().await.ok_or("missing snapshot")??;
        let closed = tokio::time::timeout(std::time::Duration::from_millis(150), async {
            while let Some(message) = socket.next().await {
                if message.is_err()
                    || matches!(
                        message,
                        Ok(tokio_tungstenite::tungstenite::Message::Close(_))
                    )
                {
                    break;
                }
            }
        })
        .await;
        assert!(closed.is_ok());
        task.abort();
        Ok(())
    }
}
