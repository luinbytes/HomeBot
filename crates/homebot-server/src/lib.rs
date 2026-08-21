//! Authoritative authenticated HTTP and WebSocket transport.

pub mod artifacts;
mod attachments;
mod bots;
mod chats;
mod checkpoints;
mod groups;
mod plugins;
mod provider_turn;
mod routines;
mod scheduler;
mod secrets;
mod skills;
mod source_control;
mod working_context;
mod workspaces;

use axum::{
    Json, Router,
    extract::{
        Request, State, WebSocketUpgrade,
        ws::{CloseFrame, Message, WebSocket},
    },
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use futures_util::{SinkExt, StreamExt};
use homebot_protocol::{
    ClientMessage, ErrorCode, ErrorEnvelope, ProtocolRange, ResumeDisposition, ServerEvent,
    ServerEventBody, Snapshot,
};
use homebot_providers::{ProviderAdapterId, ProviderRuntime};
use homebot_secrets::{OsSecretVault, SecretVault};
use homebot_storage::{IdempotencyClaim, ReplayWindow, Storage};
use homebot_tools::{NoopActivitySink, PolicyEngine};
use homebot_vcs::GitRuntime;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use subtle::ConstantTimeEq;
use tokio::sync::{Mutex, Notify, broadcast, mpsc, watch};
use tokio::{net::TcpListener, sync::oneshot};
use uuid::Uuid;

const HEARTBEAT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(15);
const HEARTBEAT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(45);
const OUTBOUND_CAPACITY: usize = 256;
const LIVE_EVENT_CAPACITY: usize = 1_024;
const COMMAND_DELAY: std::time::Duration = std::time::Duration::from_millis(5);

#[derive(Debug)]
struct OperationControl {
    cancel: Notify,
}

#[derive(Clone, Debug)]
struct ChatOperation {
    operation: Uuid,
    adapter: ProviderAdapterId,
    profile: Uuid,
    bot: Uuid,
    message: Uuid,
}

#[derive(Clone)]
pub struct AppState {
    storage: Storage,
    bearer_digest: [u8; 32],
    owner_id: Uuid,
    heartbeat_interval: std::time::Duration,
    heartbeat_timeout: std::time::Duration,
    artifact_root: PathBuf,
    outbound_capacity: usize,
    writer_delay: std::time::Duration,
    command_delay: std::time::Duration,
    operations: Arc<Mutex<HashMap<Uuid, Arc<OperationControl>>>>,
    provider_runtime: Arc<ProviderRuntime>,
    secret_vault: Arc<dyn SecretVault>,
    chat_operations: Arc<Mutex<HashMap<Uuid, ChatOperation>>>,
    live_events: broadcast::Sender<ServerEvent>,
    server_shutdown: watch::Sender<bool>,
    scheduler_started: Arc<AtomicBool>,
    routine_cancellations: Arc<Mutex<HashMap<Uuid, Arc<Notify>>>>,
    trigger_events: broadcast::Sender<(String, Uuid)>,
    git_runtime: Option<Arc<GitRuntime>>,
    policy_engine: Arc<PolicyEngine>,
    worktree_root: PathBuf,
}

impl AppState {
    #[must_use]
    pub fn new(storage: Storage, bearer_token: &str) -> Self {
        let (live_events, _) = broadcast::channel(LIVE_EVENT_CAPACITY);
        let (server_shutdown, _) = watch::channel(false);
        let (trigger_events, _) = broadcast::channel(1_024);
        Self {
            storage,
            bearer_digest: Sha256::digest(bearer_token.as_bytes()).into(),
            owner_id: Uuid::nil(),
            heartbeat_interval: HEARTBEAT_INTERVAL,
            heartbeat_timeout: HEARTBEAT_TIMEOUT,
            artifact_root: std::env::temp_dir().join("homebot-artifacts"),
            outbound_capacity: OUTBOUND_CAPACITY,
            writer_delay: std::time::Duration::ZERO,
            command_delay: COMMAND_DELAY,
            operations: Arc::new(Mutex::new(HashMap::new())),
            provider_runtime: Arc::new(ProviderRuntime::new()),
            secret_vault: Arc::new(OsSecretVault::new()),
            chat_operations: Arc::new(Mutex::new(HashMap::new())),
            live_events,
            server_shutdown,
            scheduler_started: Arc::new(AtomicBool::new(false)),
            routine_cancellations: Arc::new(Mutex::new(HashMap::new())),
            trigger_events,
            git_runtime: GitRuntime::discover().ok().map(Arc::new),
            policy_engine: Arc::new(PolicyEngine::new(
                std::time::Duration::from_secs(300),
                Arc::new(NoopActivitySink),
            )),
            worktree_root: std::env::temp_dir().join("homebot-worktrees"),
        }
    }

    #[must_use]
    pub fn storage(&self) -> &Storage {
        &self.storage
    }

    #[must_use]
    pub fn with_provider_runtime(mut self, provider_runtime: Arc<ProviderRuntime>) -> Self {
        self.provider_runtime = provider_runtime;
        self
    }

    #[must_use]
    pub fn with_secret_vault(mut self, secret_vault: Arc<dyn SecretVault>) -> Self {
        self.secret_vault = secret_vault;
        self
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

    #[must_use]
    pub fn with_artifact_root(mut self, artifact_root: PathBuf) -> Self {
        self.artifact_root = artifact_root;
        self
    }

    #[must_use]
    pub fn with_transport_limits(
        mut self,
        outbound_capacity: usize,
        writer_delay: std::time::Duration,
        command_delay: std::time::Duration,
    ) -> Self {
        self.outbound_capacity = outbound_capacity.max(1);
        self.writer_delay = writer_delay;
        self.command_delay = command_delay;
        self
    }

    #[must_use]
    pub fn with_git_runtime(mut self, runtime: Arc<GitRuntime>, worktree_root: PathBuf) -> Self {
        self.git_runtime = Some(runtime);
        self.worktree_root = worktree_root;
        self
    }

    #[must_use]
    pub fn with_policy_engine(mut self, policy_engine: Arc<PolicyEngine>) -> Self {
        self.policy_engine = policy_engine;
        self
    }
}

#[allow(clippy::too_many_lines)]
pub fn router(state: AppState) -> Router {
    if !state.scheduler_started.swap(true, Ordering::AcqRel) {
        scheduler::start(state.clone());
    }
    let authenticated = Router::new()
        .route("/api/v1/version", get(version))
        .route("/api/v1/events", get(events_socket))
        .route("/api/v1/bots", get(bots::list).post(bots::create))
        .route("/api/v1/bots/{bot_id}", put(bots::update))
        .route("/api/v1/bots/{bot_id}/archive", post(bots::archive))
        .route("/api/v1/bots/{bot_id}/restore", post(bots::restore))
        .route("/api/v1/bots/{bot_id}/read", post(bots::mark_read))
        .route("/api/v1/chats/direct", post(chats::create_direct))
        .route("/api/v1/chats/{chat_id}/timeline", get(chats::timeline))
        .route(
            "/api/v1/chats/{chat_id}/messages",
            post(chats::send_message),
        )
        .route("/api/v1/chats/{chat_id}/steer", post(chats::steer))
        .route("/api/v1/chats/{chat_id}/stop", post(chats::stop))
        .route("/api/v1/chats/{chat_id}/read", post(chats::mark_read))
        .route(
            "/api/v1/chats/{chat_id}/working-context",
            get(working_context::get).post(working_context::compact),
        )
        .route(
            "/api/v1/chats/{chat_id}/interaction-mode",
            put(working_context::set_mode),
        )
        .route(
            "/api/v1/chats/{chat_id}/messages/{message_id}/retry",
            post(chats::retry),
        )
        .route(
            "/api/v1/approvals/{approval_id}/decision",
            post(chats::decide_approval),
        )
        .route("/api/v1/groups", post(groups::create))
        .route("/api/v1/groups/{chat_id}/timeline", get(groups::timeline))
        .route(
            "/api/v1/groups/{chat_id}/messages",
            post(groups::send_message),
        )
        .route("/api/v1/groups/{chat_id}/handoff", post(groups::handoff))
        .route(
            "/api/v1/groups/{chat_id}/participants/{bot_id}/status",
            put(groups::update_participant),
        )
        .route(
            "/api/v1/groups/{chat_id}/participants",
            post(groups::add_participant),
        )
        .route(
            "/api/v1/groups/{chat_id}/participants/{bot_id}/remove",
            post(groups::remove_participant),
        )
        .route(
            "/api/v1/groups/{chat_id}/coordination-turns",
            post(groups::record_turn),
        )
        .route("/api/v1/groups/{chat_id}/stop", post(groups::stop))
        .route("/api/v1/attachments", post(attachments::create_attachment))
        .route(
            "/api/v1/attachments/{attachment_id}/content",
            put(attachments::upload_attachment),
        )
        .route(
            "/api/v1/attachments/{attachment_id}/finalize",
            post(attachments::finalize_attachment),
        )
        .route("/api/v1/artifacts/{artifact_id}", get(artifacts::metadata))
        .route(
            "/api/v1/artifacts/{artifact_id}/content",
            get(artifacts::content),
        )
        .route("/api/v1/secrets", get(secrets::list).post(secrets::create))
        .route(
            "/api/v1/secrets/{secret_id}",
            put(secrets::update).delete(secrets::delete),
        )
        .route("/api/v1/plugins", get(plugins::list).post(plugins::create))
        .route(
            "/api/v1/plugins/{plugin_id}",
            axum::routing::delete(plugins::delete),
        )
        .route(
            "/api/v1/plugins/{plugin_id}/connect",
            post(plugins::connect),
        )
        .route("/api/v1/plugins/{plugin_id}/reopen", post(plugins::connect))
        .route("/api/v1/plugins/{plugin_id}/enable", post(plugins::enable))
        .route(
            "/api/v1/plugins/{plugin_id}/disable",
            post(plugins::disable),
        )
        .route("/api/v1/plugins/{plugin_id}/health", post(plugins::connect))
        .route(
            "/api/v1/plugins/{plugin_id}/assignment",
            put(plugins::assign),
        )
        .route("/api/v1/skills", get(skills::list).post(skills::create))
        .route(
            "/api/v1/skills/{skill_id}",
            put(skills::update).delete(skills::delete),
        )
        .route(
            "/api/v1/skills/{skill_id}/duplicate",
            post(skills::duplicate),
        )
        .route("/api/v1/skills/{skill_id}/export", get(skills::export))
        .route("/api/v1/skills/import", post(skills::import))
        .route("/api/v1/skills/{skill_id}/assignment", put(skills::assign))
        .route(
            "/api/v1/workspaces",
            get(workspaces::list).post(workspaces::create),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/branches",
            get(workspaces::branches),
        )
        .route(
            "/api/v1/chats/{chat_id}/vcs/status",
            get(source_control::status),
        )
        .route(
            "/api/v1/chats/{chat_id}/vcs/diff",
            get(source_control::diff),
        )
        .route(
            "/api/v1/chats/{chat_id}/vcs/commit",
            post(source_control::commit),
        )
        .route(
            "/api/v1/chats/{chat_id}/vcs/branches",
            post(source_control::create_branch),
        )
        .route(
            "/api/v1/chats/{chat_id}/vcs/push",
            post(source_control::push),
        )
        .route(
            "/api/v1/chats/{chat_id}/vcs/pull-request",
            get(source_control::pull_request_metadata).post(source_control::create_pull_request),
        )
        .route(
            "/api/v1/chats/{chat_id}/workspace",
            get(workspaces::chat).put(workspaces::attach),
        )
        .route(
            "/api/v1/chats/{chat_id}/workspace/detach",
            post(workspaces::detach),
        )
        .route(
            "/api/v1/chats/{chat_id}/checkpoints",
            get(checkpoints::list),
        )
        .route(
            "/api/v1/chats/{chat_id}/checkpoints/diff",
            get(checkpoints::diff),
        )
        .route(
            "/api/v1/chats/{chat_id}/checkpoints/diff/full",
            get(checkpoints::full_diff),
        )
        .route(
            "/api/v1/checkpoints/{checkpoint_id}/restore",
            post(checkpoints::restore),
        )
        .route(
            "/api/v1/routines",
            get(routines::list).post(routines::create),
        )
        .route(
            "/api/v1/routines/{routine_id}",
            put(routines::update).delete(routines::delete),
        )
        .route(
            "/api/v1/routines/{routine_id}/duplicate",
            post(routines::duplicate),
        )
        .route(
            "/api/v1/routines/{routine_id}/enable",
            post(routines::enable),
        )
        .route(
            "/api/v1/routines/{routine_id}/disable",
            post(routines::disable),
        )
        .route("/api/v1/routines/{routine_id}/run", post(routines::run_now))
        .route(
            "/api/v1/routines/{routine_id}/dry-run",
            post(routines::dry_run),
        )
        .route("/api/v1/routines/{routine_id}/runs", get(routines::runs))
        .route(
            "/api/v1/routines/{routine_id}/triggers",
            get(scheduler::list_triggers).post(scheduler::create_trigger),
        )
        .route(
            "/api/v1/routine-triggers/{trigger_id}",
            axum::routing::delete(scheduler::delete_trigger),
        )
        .route(
            "/api/v1/routine-triggers/{trigger_id}/deliver",
            post(scheduler::deliver_trigger),
        )
        .route(
            "/api/v1/routines/{routine_id}/jobs",
            get(scheduler::list_jobs),
        )
        .route(
            "/api/v1/routine-jobs/{job_id}/cancel",
            post(scheduler::cancel_job),
        )
        .route(
            "/api/v1/routine-recordings",
            post(routines::start_recording),
        )
        .route(
            "/api/v1/routine-recordings/{recording_id}/actions",
            post(routines::append_recording),
        )
        .route(
            "/api/v1/routine-recordings/{recording_id}/finish",
            post(routines::finish_recording),
        )
        .route(
            "/api/v1/routine-recordings/{recording_id}/cancel",
            post(routines::cancel_recording),
        )
        .route_layer(middleware::from_fn_with_state(state.clone(), authenticate));
    Router::new()
        .route("/health", get(health))
        .merge(authenticated)
        .with_state(state)
}

/// Serves the authoritative API on an already-bound listener until shutdown is requested.
///
/// Keeping listener ownership outside the server lets the desktop supervisor and the headless
/// binary share exactly the same transport implementation without duplicating product logic.
///
/// # Errors
///
/// Returns a listener/connection I/O error if Axum cannot continue serving requests.
pub async fn serve(
    listener: TcpListener,
    state: AppState,
    shutdown: oneshot::Receiver<()>,
) -> std::io::Result<()> {
    let shutdown_signal = state.server_shutdown.clone();
    axum::serve(listener, router(state))
        .with_graceful_shutdown(async move {
            let _ = shutdown.await;
            let _ = shutdown_signal.send(true);
        })
        .await
}

async fn events_socket(State(state): State<AppState>, upgrade: WebSocketUpgrade) -> Response {
    upgrade.on_upgrade(move |socket| serve_events(socket, state))
}

async fn serve_events(mut socket: WebSocket, state: AppState) {
    let mut live_events = state.live_events.subscribe();
    let mut server_shutdown = state.server_shutdown.subscribe();
    let Some(mut last_queued) = initial_sync(&mut socket, &state).await else {
        return;
    };
    let (sink, mut stream) = socket.split();
    let (outbound_tx, outbound_rx) = mpsc::channel::<ServerEvent>(state.outbound_capacity);
    let (disconnect_tx, mut disconnect_rx) = watch::channel(false);
    let writer = spawn_event_writer(
        sink,
        outbound_rx,
        disconnect_rx.clone(),
        disconnect_tx.clone(),
        state.writer_delay,
        last_queued,
    );

    let mut heartbeat = tokio::time::interval(state.heartbeat_interval);
    heartbeat.tick().await;
    let mut last_pong = tokio::time::Instant::now();
    let mut pending_nonce = None;
    loop {
        tokio::select! {
            changed = server_shutdown.changed() => {
                if changed.is_err() || *server_shutdown.borrow() { break; }
            }
            _ = heartbeat.tick() => {
                if last_pong.elapsed() >= state.heartbeat_timeout {
                    let _ = disconnect_tx.send(true);
                    break;
                }
                let sequence = state.storage.latest_sequence(state.owner_id).await.unwrap_or(last_queued);
                let nonce = Uuid::now_v7();
                pending_nonce = Some(nonce);
                let ping = ServerEvent {
                    protocol_version: homebot_protocol::PROTOCOL_VERSION,
                    sequence,
                    event_id: Uuid::now_v7(),
                    body: ServerEventBody::Ping { nonce },
                };
                if !queue_outbound(&outbound_tx, &disconnect_tx, ping) {
                    break;
                }
            }
            changed = disconnect_rx.changed() => {
                if changed.is_err() || *disconnect_rx.borrow() { break; }
            }
            event = live_events.recv() => {
                match event {
                    Ok(event) if event.sequence > last_queued => {
                        last_queued = event.sequence;
                        if !queue_outbound(&outbound_tx, &disconnect_tx, event) { break; }
                    }
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        let _ = disconnect_tx.send(true);
                        break;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            message = stream.next() => {
                let Some(Ok(Message::Text(text))) = message else { break; };
                if let Ok(message) = serde_json::from_str::<ClientMessage>(&text) {
                    match message {
                        ClientMessage::Pong { nonce } if pending_nonce == Some(nonce) => {
                            last_pong = tokio::time::Instant::now();
                            pending_nonce = None;
                        }
                        ClientMessage::Pong { .. } => {}
                        message => handle_client_message(&state, message).await,
                    }
                }
            }
        }
    }
    let _ = disconnect_tx.send(true);
    drop(outbound_tx);
    let _ = tokio::time::timeout(std::time::Duration::from_millis(250), writer).await;
}

fn spawn_event_writer(
    mut sink: futures_util::stream::SplitSink<WebSocket, Message>,
    mut outbound: mpsc::Receiver<ServerEvent>,
    mut disconnect: watch::Receiver<bool>,
    signal: watch::Sender<bool>,
    writer_delay: std::time::Duration,
    initial_cursor: u64,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut last_sent = initial_cursor;
        loop {
            tokio::select! {
                changed = disconnect.changed() => {
                    if changed.is_ok() && *disconnect.borrow() {
                        close_with_cursor(&mut sink, last_sent).await;
                    }
                    break;
                }
                event = outbound.recv() => {
                    let Some(event) = event else { break; };
                    if !writer_delay.is_zero() {
                        tokio::select! {
                            () = tokio::time::sleep(writer_delay) => {}
                            changed = disconnect.changed() => {
                                if changed.is_ok() && *disconnect.borrow() {
                                    close_with_cursor(&mut sink, last_sent).await;
                                }
                                break;
                            }
                        }
                    }
                    if send_json_sink(&mut sink, &event).await.is_err() {
                        let _ = signal.send(true);
                        break;
                    }
                    last_sent = event.sequence;
                }
            }
        }
    })
}

async fn close_with_cursor(
    sink: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    cursor: u64,
) {
    let reason = format!("resume_after={cursor}");
    let _ = sink
        .send(Message::Close(Some(CloseFrame {
            code: 1013,
            reason: reason.into(),
        })))
        .await;
}

fn queue_outbound(
    sender: &mpsc::Sender<ServerEvent>,
    disconnect: &watch::Sender<bool>,
    event: ServerEvent,
) -> bool {
    match sender.try_send(event) {
        Ok(()) => true,
        Err(mpsc::error::TrySendError::Full(_) | mpsc::error::TrySendError::Closed(_)) => {
            let _ = disconnect.send(true);
            false
        }
    }
}

async fn initial_sync(socket: &mut WebSocket, state: &AppState) -> Option<u64> {
    let Some(Ok(Message::Text(text))) = socket.next().await else {
        return None;
    };
    let Ok(ClientMessage::Hello {
        protocol_version,
        resume_after,
        ..
    }) = serde_json::from_str(&text)
    else {
        return None;
    };
    if let Err(error) = homebot_protocol::check_compatibility(protocol_version) {
        if let Ok(encoded) = serde_json::to_string(&error) {
            let _ = socket.send(Message::Text(encoded.into())).await;
        }
        return None;
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
        return None;
    }
    if let Some(ReplayWindow::Available(mut events)) = replay {
        let mut last_sent = cursor;
        loop {
            let batch_is_full = events.len() == 1_000;
            let mut next_cursor = last_sent;
            for event in events {
                next_cursor = event.sequence;
                if send_json(socket, &event.payload).await.is_err() {
                    return None;
                }
            }
            last_sent = next_cursor;
            if !batch_is_full {
                return Some(last_sent);
            }
            events = match state
                .storage
                .replay_after(state.owner_id, next_cursor, 1_000)
                .await
            {
                Ok(ReplayWindow::Available(events)) => events,
                Ok(ReplayWindow::Unavailable) | Err(_) => return None,
            };
        }
    }
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
            snapshot: current_snapshot(state).await,
        },
    };
    if send_json(socket, &snapshot).await.is_err() {
        return None;
    }
    Some(boundary)
}

async fn current_snapshot(state: &AppState) -> Snapshot {
    let bots = state
        .storage
        .list_bots(state.owner_id, true)
        .await
        .unwrap_or_default();
    let mut summaries = Vec::with_capacity(bots.len());
    for bot in bots {
        summaries.push(bots::summary(state, bot).await);
    }
    let chats = state
        .storage
        .list_direct_chats(state.owner_id)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(chats::chat_summary)
        .collect();
    let group_chats = state
        .storage
        .list_group_chats(state.owner_id)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(groups::group_summary)
        .collect();
    let skills = state
        .storage
        .list_skills(state.owner_id)
        .await
        .unwrap_or_default()
        .iter()
        .map(skills::summary)
        .collect();
    let repository_workspaces = workspaces::repository_summaries(state)
        .await
        .unwrap_or_default();
    let chat_workspaces = workspaces::chat_summaries(state).await.unwrap_or_default();
    Snapshot {
        bots: summaries,
        chats,
        group_chats,
        skills,
        repository_workspaces,
        chat_workspaces,
    }
}

async fn send_json_sink<S>(sink: &mut S, value: &impl serde::Serialize) -> Result<(), ()>
where
    S: futures_util::Sink<Message> + Unpin,
{
    let encoded = serde_json::to_string(value).map_err(|_| ())?;
    sink.send(Message::Text(encoded.into()))
        .await
        .map_err(|_| ())
}

async fn handle_client_message(state: &AppState, message: ClientMessage) {
    match message {
        ClientMessage::Command {
            request_id,
            idempotency_key,
            command,
        } => {
            let Ok(encoded) = serde_json::to_vec(&command) else {
                return;
            };
            let request_hash = format!("{:x}", Sha256::digest(encoded));
            let proposed = Uuid::now_v7();
            let Ok(claim) = state
                .storage
                .claim_idempotency(idempotency_key, &request_hash, proposed, unix_time_ms())
                .await
            else {
                return;
            };
            match claim {
                IdempotencyClaim::Claimed { operation_id } => {
                    let control = Arc::new(OperationControl {
                        cancel: Notify::new(),
                    });
                    state
                        .operations
                        .lock()
                        .await
                        .insert(operation_id, Arc::clone(&control));
                    let accepted = persist_event(
                        state,
                        "command_accepted",
                        ServerEventBody::CommandAccepted {
                            request_id,
                            operation_id,
                        },
                    )
                    .await;
                    if accepted.is_ok() {
                        spawn_operation(state.clone(), control, request_id, operation_id, command);
                    } else {
                        state.operations.lock().await.remove(&operation_id);
                    }
                }
                IdempotencyClaim::Replayed { operation_id } => {
                    let _ = persist_event(
                        state,
                        "command_accepted",
                        ServerEventBody::CommandAccepted {
                            request_id,
                            operation_id,
                        },
                    )
                    .await;
                }
                IdempotencyClaim::Conflict => {
                    let _ = persist_event(
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
                }
            }
        }
        ClientMessage::Cancel { operation_id, .. } => {
            if let Some(control) = state.operations.lock().await.get(&operation_id).cloned() {
                control.cancel.notify_one();
            }
        }
        ClientMessage::Hello { .. } | ClientMessage::Pong { .. } => {}
    }
}

fn spawn_operation(
    state: AppState,
    control: Arc<OperationControl>,
    request_id: Uuid,
    operation_id: Uuid,
    command: homebot_protocol::Command,
) {
    tokio::spawn(async move {
        let cancelled = tokio::select! {
            () = tokio::time::sleep(state.command_delay) => false,
            () = control.cancel.notified() => true,
        };
        let (kind, body) = if cancelled {
            (
                "command_cancelled",
                ServerEventBody::CommandCancelled {
                    request_id,
                    operation_id,
                },
            )
        } else {
            let command_kind = match command {
                homebot_protocol::Command::CreateBot { .. } => "create_bot",
                homebot_protocol::Command::SendMessage { .. } => "send_message",
            };
            (
                "command_completed",
                ServerEventBody::CommandCompleted {
                    request_id,
                    operation_id,
                    result: json!({"status":"completed","command":command_kind}),
                },
            )
        };
        let _ = persist_event(&state, kind, body).await;
        state.operations.lock().await.remove(&operation_id);
    });
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
    let event: ServerEvent = serde_json::from_value(stored.payload).map_err(|_| ())?;
    let _ = state.live_events.send(event.clone());
    if !kind.starts_with("routine_") {
        let _ = state.trigger_events.send((kind.to_owned(), event.event_id));
    }
    Ok(event)
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
mod tests;
