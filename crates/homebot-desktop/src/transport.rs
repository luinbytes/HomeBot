//! Authenticated desktop transport and local authoritative-server supervision.

use std::{
    collections::VecDeque,
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    sync::mpsc::{Receiver, Sender, channel},
    thread::JoinHandle,
    time::Duration,
};

use futures_util::{SinkExt, StreamExt};
use homebot_protocol::{
    ApprovalDecisionRequest, AttachChatWorkspaceRequest, BotMutationRequest, BotResponse,
    ChatTimelineResponse, ChatWorkspaceSummary, CheckpointDiffResponse, ClientMessage,
    CompactWorkingContextRequest, CreateAttachmentRequest, CreateAttachmentResponse,
    CreateBotRequest, CreateDirectChatRequest, CreateDirectChatResponse, CreatePairingRequest,
    CreatePullRequestRequest, CreateRepositoryWorkspaceRequest, DeleteBotRequest,
    DetachChatWorkspaceRequest, DeviceSessionSummary, ErrorEnvelope, FinalizeAttachmentRequest,
    MIN_COMPATIBLE_PROTOCOL_VERSION, MessageMutationRequest, PROTOCOL_VERSION, PairingOffer,
    ProtocolRange, PullRequestMetadata, PullRequestMutationResponse, ReactionMutationRequest,
    RepositoryWorkspaceSummary, RestoreCheckpointRequest, RevokeDeviceSessionRequest,
    SendMessageRequest, ServerEvent, ServerEventBody, SetInteractionModeRequest, Snapshot,
    UpdateBotRequest, VcsCommitRequest, VcsCommitResult, VcsCreateBranchRequest, VcsPushRequest,
    VcsRemoteMutationResponse, VcsStatus, WorkingContextSummary, WorkingTreeDiffResponse,
    WorkspaceBranchesResponse,
};
use reqwest::{Client, Method, StatusCode};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::{net::TcpListener, sync::mpsc};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{
        Message,
        client::IntoClientRequest,
        http::{HeaderValue, header::AUTHORIZATION},
    },
};
use uuid::Uuid;

use crate::{
    bot_roster::{BotClientCommand, BotEditorDraft},
    timeline::{ComposerDraft, TimelineCommand},
    workspaces::WorkspaceCommand,
};

const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone)]
pub struct RuntimeConfig {
    pub endpoint: String,
    pub device_token: String,
    pub local_database: Option<PathBuf>,
    pub reconnect_delay: Duration,
}

impl std::fmt::Debug for RuntimeConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RuntimeConfig")
            .field("endpoint", &self.endpoint)
            .field("device_token", &"[REDACTED]")
            .field("local_database", &self.local_database)
            .field("reconnect_delay", &self.reconnect_delay)
            .finish()
    }
}

impl RuntimeConfig {
    #[must_use]
    pub fn desktop_default(endpoint: impl Into<String>) -> Self {
        let endpoint = endpoint.into().trim_end_matches('/').to_owned();
        let device_token =
            std::env::var("HOMEBOT_DEVICE_TOKEN").unwrap_or_else(|_| Uuid::now_v7().to_string());
        let local_database = is_loopback_endpoint(&endpoint).then(default_database_path);
        Self {
            endpoint,
            device_token,
            local_database,
            reconnect_delay: Duration::from_millis(250),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TransportFailure {
    ServerUnavailable,
    AuthenticationFailed,
    VersionMismatch,
    InvalidEndpoint,
    Protocol(String),
    Request(String),
}

impl std::fmt::Display for TransportFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ServerUnavailable => formatter.write_str("HomeBot server is unavailable"),
            Self::AuthenticationFailed => formatter.write_str("HomeBot authentication failed"),
            Self::VersionMismatch => {
                formatter.write_str("HomeBot protocol versions are incompatible")
            }
            Self::InvalidEndpoint => formatter.write_str("HomeBot endpoint is invalid"),
            Self::Protocol(message) | Self::Request(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for TransportFailure {}

#[derive(Clone, Debug)]
pub enum DesktopEvent {
    Connecting,
    Connected,
    Disconnected(TransportFailure),
    Snapshot {
        boundary_sequence: u64,
        snapshot: Snapshot,
    },
    Server(ServerEvent),
    Timeline(ChatTimelineResponse),
    BotMutation(BotResponse),
    AttachmentUploaded(Uuid),
    RepositoryWorkspaceRegistered(RepositoryWorkspaceSummary),
    ChatWorkspaceAttached(ChatWorkspaceSummary),
    ChatWorkspaceDetached(Uuid),
    WorkspaceBranches {
        workspace_id: Uuid,
        branches: Vec<String>,
    },
    VcsStatus {
        chat_id: Uuid,
        status: VcsStatus,
    },
    VcsDiff {
        chat_id: Uuid,
        diff: WorkingTreeDiffResponse,
    },
    VcsCommit {
        chat_id: Uuid,
        result: VcsCommitResult,
    },
    VcsRemoteMutation {
        chat_id: Uuid,
        response: VcsRemoteMutationResponse,
    },
    PullRequestMetadata {
        chat_id: Uuid,
        metadata: PullRequestMetadata,
    },
    PullRequestMutation {
        chat_id: Uuid,
        response: PullRequestMutationResponse,
    },
    WorkingContext(WorkingContextSummary),
    Devices(Vec<DeviceSessionSummary>),
    PairingOffer(PairingOffer),
    DeviceRevoked(DeviceSessionSummary),
    CheckpointDiff(CheckpointDiffResponse),
    MutationFailed(TransportFailure),
}

#[derive(Clone, Debug)]
pub enum DesktopCommand {
    Bot(BotClientCommand),
    LoadTimeline(Uuid),
    Timeline {
        bot_id: Uuid,
        chat_id: Option<Uuid>,
        command: TimelineCommand,
    },
    UploadAttachment {
        filename: String,
        media_type: String,
        bytes: Vec<u8>,
    },
    Workspace(WorkspaceCommand),
    LoadDevices,
    CreatePairing {
        endpoint: String,
        allow_insecure_private_network: bool,
    },
    RevokeDevice(Uuid),
    Shutdown,
}

pub struct DesktopTransport {
    commands: mpsc::UnboundedSender<DesktopCommand>,
    events: Receiver<DesktopEvent>,
    thread: Option<JoinHandle<()>>,
}

impl DesktopTransport {
    #[must_use]
    pub fn start(config: RuntimeConfig) -> Self {
        let (commands, command_rx) = mpsc::unbounded_channel();
        let (event_tx, events) = channel();
        let thread = std::thread::Builder::new()
            .name("homebot-desktop-transport".to_owned())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build();
                match runtime {
                    Ok(runtime) => runtime.block_on(run(config, command_rx, event_tx)),
                    Err(error) => {
                        let _ =
                            event_tx.send(DesktopEvent::Disconnected(TransportFailure::Request(
                                format!("Could not start the HomeBot client runtime: {error}"),
                            )));
                    }
                }
            })
            .ok();
        Self {
            commands,
            events,
            thread,
        }
    }

    /// Queues a mutation for the authenticated server connection.
    ///
    /// # Errors
    ///
    /// Returns `ServerUnavailable` after the background runtime has stopped.
    pub fn send(&self, command: DesktopCommand) -> Result<(), TransportFailure> {
        self.commands
            .send(command)
            .map_err(|_| TransportFailure::ServerUnavailable)
    }

    pub fn try_events(&self) -> impl Iterator<Item = DesktopEvent> + '_ {
        self.events.try_iter()
    }
}

impl Drop for DesktopTransport {
    fn drop(&mut self) {
        let _ = self.commands.send(DesktopCommand::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct LocalServer {
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
}

impl LocalServer {
    async fn shutdown(mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        let _ = tokio::time::timeout(Duration::from_secs(2), self.task).await;
    }
}

async fn run(
    config: RuntimeConfig,
    mut commands: mpsc::UnboundedReceiver<DesktopCommand>,
    events: Sender<DesktopEvent>,
) {
    let client = match Client::builder().timeout(Duration::from_secs(15)).build() {
        Ok(client) => client,
        Err(error) => {
            let _ = events.send(DesktopEvent::Disconnected(TransportFailure::Request(
                error.to_string(),
            )));
            return;
        }
    };
    let mut local_server = None;
    let mut cursor = None;
    let mut pending_commands = VecDeque::new();
    loop {
        let _ = events.send(DesktopEvent::Connecting);
        if health(&client, &config.endpoint).await.is_err() && local_server.is_none() {
            match start_local_server(&config).await {
                Ok(server) => local_server = server,
                Err(error) => {
                    let _ = events.send(DesktopEvent::Disconnected(error));
                }
            }
        }

        match connect(&client, &config, cursor).await {
            Ok(mut socket) => {
                let _ = events.send(DesktopEvent::Connected);
                let disconnected = connected_loop(
                    &client,
                    &config,
                    &mut socket,
                    &mut commands,
                    &mut pending_commands,
                    &events,
                    &mut cursor,
                )
                .await;
                if matches!(disconnected, ConnectedExit::Shutdown) {
                    break;
                }
                if let ConnectedExit::Failed(error) = disconnected {
                    let _ = events.send(DesktopEvent::Disconnected(error));
                }
            }
            Err(error) => {
                let _ = events.send(DesktopEvent::Disconnected(error));
            }
        }

        tokio::select! {
            command = commands.recv() => {
                match command {
                    None | Some(DesktopCommand::Shutdown) => break,
                    Some(command) => pending_commands.push_back(command),
                }
            }
            () = tokio::time::sleep(config.reconnect_delay) => {}
        }
    }
    if let Some(server) = local_server {
        server.shutdown().await;
    }
}

async fn start_local_server(
    config: &RuntimeConfig,
) -> Result<Option<LocalServer>, TransportFailure> {
    let Some(database) = &config.local_database else {
        return Err(TransportFailure::ServerUnavailable);
    };
    let address = endpoint_socket(&config.endpoint)?;
    if let Some(parent) = database.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| TransportFailure::Request(error.to_string()))?;
    }
    let listener = TcpListener::bind(address)
        .await
        .map_err(|_| TransportFailure::ServerUnavailable)?;
    let storage = homebot_storage::Storage::open(database)
        .await
        .map_err(|error| TransportFailure::Request(error.to_string()))?;
    let artifact_root = database
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("artifacts");
    let state = homebot_server::AppState::new(storage, &config.device_token)
        .with_artifact_root(artifact_root);
    let (shutdown, shutdown_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let _ = homebot_server::serve(listener, state, shutdown_rx).await;
    });
    Ok(Some(LocalServer {
        shutdown: Some(shutdown),
        task,
    }))
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect(
    client: &Client,
    config: &RuntimeConfig,
    resume_after: Option<u64>,
) -> Result<Socket, TransportFailure> {
    negotiate(client, config).await?;
    let websocket_url = websocket_url(&config.endpoint)?;
    let mut request = websocket_url
        .into_client_request()
        .map_err(|_| TransportFailure::InvalidEndpoint)?;
    request.headers_mut().insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {}", config.device_token))
            .map_err(|_| TransportFailure::AuthenticationFailed)?,
    );
    let (mut socket, _) = connect_async(request)
        .await
        .map_err(classify_websocket_error)?;
    send_socket(
        &mut socket,
        &ClientMessage::Hello {
            protocol_version: PROTOCOL_VERSION,
            client_version: CLIENT_VERSION.to_owned(),
            device_session: "desktop".to_owned(),
            resume_after,
        },
    )
    .await?;
    Ok(socket)
}

#[derive(Deserialize)]
struct VersionResponse {
    protocol: ProtocolRange,
}

async fn negotiate(client: &Client, config: &RuntimeConfig) -> Result<(), TransportFailure> {
    let response = authenticated(client, config, Method::GET, "/api/v1/version")
        .header("x-homebot-protocol", PROTOCOL_VERSION)
        .send()
        .await
        .map_err(|_| TransportFailure::ServerUnavailable)?;
    if response.status() == StatusCode::UNAUTHORIZED {
        return Err(TransportFailure::AuthenticationFailed);
    }
    if response.status() == StatusCode::UPGRADE_REQUIRED {
        return Err(TransportFailure::VersionMismatch);
    }
    let response: VersionResponse = response
        .error_for_status()
        .map_err(|error| TransportFailure::Request(error.to_string()))?
        .json()
        .await
        .map_err(|error| TransportFailure::Protocol(error.to_string()))?;
    if response.protocol.minimum > PROTOCOL_VERSION
        || response.protocol.maximum < MIN_COMPATIBLE_PROTOCOL_VERSION
    {
        return Err(TransportFailure::VersionMismatch);
    }
    Ok(())
}

enum ConnectedExit {
    Shutdown,
    Failed(TransportFailure),
}

async fn connected_loop(
    client: &Client,
    config: &RuntimeConfig,
    socket: &mut Socket,
    commands: &mut mpsc::UnboundedReceiver<DesktopCommand>,
    pending_commands: &mut VecDeque<DesktopCommand>,
    events: &Sender<DesktopEvent>,
    cursor: &mut Option<u64>,
) -> ConnectedExit {
    loop {
        while let Some(command) = pending_commands.pop_front() {
            if let Err(error) = execute_command(client, config, command, events).await {
                let _ = events.send(DesktopEvent::MutationFailed(error));
            }
        }
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else { return ConnectedExit::Shutdown; };
                if matches!(command, DesktopCommand::Shutdown) {
                    let _ = socket.close(None).await;
                    return ConnectedExit::Shutdown;
                }
                if let Err(error) = execute_command(client, config, command, events).await {
                    let _ = events.send(DesktopEvent::MutationFailed(error));
                }
            }
            message = socket.next() => {
                let Some(message) = message else {
                    return ConnectedExit::Failed(TransportFailure::ServerUnavailable);
                };
                match handle_socket(message, socket, events, cursor).await {
                    Ok(()) => {}
                    Err(error) => return ConnectedExit::Failed(error),
                }
            }
        }
    }
}

async fn handle_socket(
    message: Result<Message, tokio_tungstenite::tungstenite::Error>,
    socket: &mut Socket,
    events: &Sender<DesktopEvent>,
    cursor: &mut Option<u64>,
) -> Result<(), TransportFailure> {
    let Message::Text(text) = message.map_err(classify_websocket_error)? else {
        return Ok(());
    };
    let event: ServerEvent = serde_json::from_str(&text)
        .map_err(|error| TransportFailure::Protocol(error.to_string()))?;
    if event.protocol_version < MIN_COMPATIBLE_PROTOCOL_VERSION
        || event.protocol_version > PROTOCOL_VERSION
    {
        return Err(TransportFailure::VersionMismatch);
    }
    match &event.body {
        ServerEventBody::Hello {
            supported_protocols,
            ..
        } => {
            if supported_protocols.minimum > PROTOCOL_VERSION
                || supported_protocols.maximum < MIN_COMPATIBLE_PROTOCOL_VERSION
            {
                return Err(TransportFailure::VersionMismatch);
            }
        }
        ServerEventBody::Snapshot {
            boundary_sequence,
            snapshot,
        } => {
            *cursor = Some(*boundary_sequence);
            let _ = events.send(DesktopEvent::Snapshot {
                boundary_sequence: *boundary_sequence,
                snapshot: snapshot.clone(),
            });
        }
        ServerEventBody::Ping { nonce } => {
            send_socket(socket, &ClientMessage::Pong { nonce: *nonce }).await?;
        }
        _ => {
            if cursor.is_some_and(|current| event.sequence <= current) {
                return Ok(());
            }
            if cursor.is_some_and(|current| event.sequence != current.saturating_add(1)) {
                return Err(TransportFailure::Protocol(
                    "HomeBot event sequence has a gap; reconnecting for replay".to_owned(),
                ));
            }
            *cursor = Some(event.sequence);
            let _ = events.send(DesktopEvent::Server(event));
        }
    }
    Ok(())
}

mod api;
use api::{authenticated, execute_command};

async fn health(client: &Client, endpoint: &str) -> Result<(), TransportFailure> {
    client
        .get(format!("{endpoint}/health"))
        .send()
        .await
        .map_err(|_| TransportFailure::ServerUnavailable)?
        .error_for_status()
        .map(|_| ())
        .map_err(|_| TransportFailure::ServerUnavailable)
}

async fn send_socket(socket: &mut Socket, message: &ClientMessage) -> Result<(), TransportFailure> {
    let encoded = serde_json::to_string(message).map_err(protocol_error)?;
    socket
        .send(Message::Text(encoded.into()))
        .await
        .map_err(classify_websocket_error)
}

fn classify_websocket_error(error: tokio_tungstenite::tungstenite::Error) -> TransportFailure {
    match error {
        tokio_tungstenite::tungstenite::Error::Http(response)
            if response.status()
                == tokio_tungstenite::tungstenite::http::StatusCode::UNAUTHORIZED =>
        {
            TransportFailure::AuthenticationFailed
        }
        _ => TransportFailure::ServerUnavailable,
    }
}

fn websocket_url(endpoint: &str) -> Result<String, TransportFailure> {
    if let Some(rest) = endpoint.strip_prefix("http://") {
        Ok(format!("ws://{rest}/api/v1/events"))
    } else if let Some(rest) = endpoint.strip_prefix("https://") {
        Ok(format!("wss://{rest}/api/v1/events"))
    } else {
        Err(TransportFailure::InvalidEndpoint)
    }
}

fn endpoint_socket(endpoint: &str) -> Result<SocketAddr, TransportFailure> {
    let url = reqwest::Url::parse(endpoint).map_err(|_| TransportFailure::InvalidEndpoint)?;
    let host = url.host_str().ok_or(TransportFailure::InvalidEndpoint)?;
    let ip: IpAddr = host
        .parse()
        .map_err(|_| TransportFailure::InvalidEndpoint)?;
    if !ip.is_loopback() {
        return Err(TransportFailure::InvalidEndpoint);
    }
    let port = url
        .port_or_known_default()
        .ok_or(TransportFailure::InvalidEndpoint)?;
    Ok(SocketAddr::new(ip, port))
}

fn is_loopback_endpoint(endpoint: &str) -> bool {
    endpoint_socket(endpoint).is_ok()
}

fn default_database_path() -> PathBuf {
    if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map_or_else(
            || PathBuf::from("homebot.db"),
            |home| PathBuf::from(home).join("Library/Application Support/HomeBot/homebot.db"),
        )
    } else if let Some(data) = std::env::var_os("XDG_DATA_HOME") {
        PathBuf::from(data).join("homebot/homebot.db")
    } else {
        std::env::var_os("HOME").map_or_else(
            || PathBuf::from("homebot.db"),
            |home| PathBuf::from(home).join(".local/share/homebot/homebot.db"),
        )
    }
}

fn request_error(error: reqwest::Error) -> TransportFailure {
    let is_connect = error.is_connect();
    let message = error.to_string();
    drop(error);
    if is_connect {
        TransportFailure::ServerUnavailable
    } else {
        TransportFailure::Request(message)
    }
}

fn protocol_error(error: serde_json::Error) -> TransportFailure {
    let message = error.to_string();
    drop(error);
    TransportFailure::Protocol(message)
}

#[cfg(test)]
#[path = "transport/tests.rs"]
mod tests;
