//! Codex CLI adapter using the structured App Server JSONL protocol.

#[path = "codex/protocol.rs"]
mod protocol;

use crate::{
    ApprovalDecision, CompactRequest, ExecutionMode, ProcessSpec, ProviderAdapter,
    ProviderAdapterId, ProviderApproval, ProviderAvailability, ProviderCapabilities,
    ProviderCapability, ProviderDescriptor, ProviderError, ProviderErrorCode, ProviderEvent,
    ProviderHealth, ProviderModel, ProviderRun, ResumeRequest, StartRequest, SupervisedProcess,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    ffi::OsString,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{ChildStdin, ChildStdout},
    sync::{Mutex, mpsc, oneshot},
};
use uuid::Uuid;

use protocol::{normalize_codex_error, notification_events, rpc_error};

const RPC_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_PROTOCOL_LINE_BYTES: usize = 4 * 1024 * 1024;
const EVENT_BUFFER: usize = 128;

/// One independently configurable Codex provider profile.
#[derive(Clone, Debug)]
pub struct CodexProfile {
    pub adapter_id: ProviderAdapterId,
    pub binary_path: PathBuf,
    pub working_directory: Option<PathBuf>,
    pub environment: BTreeMap<OsString, OsString>,
}

impl CodexProfile {
    #[must_use]
    pub fn new(adapter_id: ProviderAdapterId, binary_path: impl Into<PathBuf>) -> Self {
        Self {
            adapter_id,
            binary_path: binary_path.into(),
            working_directory: None,
            environment: safe_codex_environment(),
        }
    }

    #[must_use]
    pub fn working_directory(mut self, path: impl Into<PathBuf>) -> Self {
        self.working_directory = Some(path.into());
        self
    }

    #[must_use]
    pub fn environment(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.environment.insert(key.into(), value.into());
        self
    }
}

/// A profile-scoped App Server adapter. Register multiple instances for multiple accounts.
pub struct CodexAdapter {
    profile: CodexProfile,
    client: Mutex<Option<Arc<CodexClient>>>,
}

impl std::fmt::Debug for CodexAdapter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CodexAdapter")
            .field("adapter_id", &self.profile.adapter_id)
            .field("binary", &self.profile.binary_path.file_name())
            .field("working_directory", &self.profile.working_directory)
            .field("environment_keys", &self.profile.environment.keys())
            .finish_non_exhaustive()
    }
}

impl CodexAdapter {
    #[must_use]
    pub fn new(profile: CodexProfile) -> Self {
        Self {
            profile,
            client: Mutex::new(None),
        }
    }

    async fn client(&self) -> Result<Arc<CodexClient>, ProviderError> {
        let mut slot = self.client.lock().await;
        if let Some(client) = slot.as_ref().filter(|client| client.is_alive()) {
            return Ok(Arc::clone(client));
        }
        let client = CodexClient::spawn(&self.profile).await?;
        *slot = Some(Arc::clone(&client));
        Ok(client)
    }

    async fn begin_turn(
        &self,
        operation_id: Uuid,
        conversation_id: String,
        prompt: String,
        model: Option<String>,
        mode: ExecutionMode,
    ) -> Result<ProviderRun, ProviderError> {
        let client = self.client().await?;
        let (events_tx, events_rx) = mpsc::channel(EVENT_BUFFER);
        client
            .register_route(operation_id, conversation_id.clone(), events_tx)
            .await?;
        let mut params = json!({
            "threadId": conversation_id,
            "input": [{"type": "text", "text": prompt}],
        });
        if let Some(model) = model {
            params["model"] = Value::String(model);
        }
        if let Some(cwd) = &self.profile.working_directory {
            params["cwd"] = Value::String(cwd.to_string_lossy().into_owned());
        }
        if mode == ExecutionMode::Plan {
            params["collaborationMode"] = json!({
                "mode": "plan",
                "settings": {"developer_instructions": null}
            });
        }
        let result = match client.request("turn/start", params).await {
            Ok(result) => result,
            Err(error) => {
                client.remove_route(operation_id).await;
                return Err(error);
            }
        };
        let turn_id = string_at(&result, &["turn", "id"])
            .ok_or_else(|| protocol_error("turn/start omitted turn.id"))?;
        client.set_turn(operation_id, turn_id).await?;
        Ok(ProviderRun {
            operation_id,
            events: events_rx,
        })
    }
}

#[async_trait::async_trait]
impl ProviderAdapter for CodexAdapter {
    fn id(&self) -> &ProviderAdapterId {
        &self.profile.adapter_id
    }

    async fn discover(&self) -> Result<ProviderDescriptor, ProviderError> {
        let executable = resolve_binary(&self.profile).map(|path| path.display().to_string());
        Ok(ProviderDescriptor {
            adapter_id: self.profile.adapter_id.clone(),
            display_name: "Codex CLI".to_owned(),
            executable,
            capabilities: ProviderCapabilities {
                supported: [
                    ProviderCapability::ConversationResume,
                    ProviderCapability::Streaming,
                    ProviderCapability::Activities,
                    ProviderCapability::Approvals,
                    ProviderCapability::Cancellation,
                    ProviderCapability::Usage,
                    ProviderCapability::Compaction,
                    ProviderCapability::PlanMode,
                ]
                .into_iter()
                .collect(),
            },
        })
    }

    async fn health(&self) -> ProviderHealth {
        let checked_at_unix_ms = unix_ms();
        if resolve_binary(&self.profile).is_none() {
            return ProviderHealth {
                availability: ProviderAvailability::NotInstalled,
                message: "Codex CLI was not found at the configured path".to_owned(),
                checked_at_unix_ms,
            };
        }
        match self.client().await {
            Ok(client) => match client
                .request("account/read", json!({"refreshToken": false}))
                .await
            {
                Ok(result) => {
                    let requires_auth = result
                        .get("requiresOpenaiAuth")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    let has_account = result.get("account").is_some_and(|value| !value.is_null());
                    let availability = if requires_auth && !has_account {
                        ProviderAvailability::AuthenticationRequired
                    } else {
                        ProviderAvailability::Available
                    };
                    ProviderHealth {
                        availability,
                        message: if availability == ProviderAvailability::Available {
                            "Codex App Server is ready".to_owned()
                        } else {
                            "Codex CLI requires authentication".to_owned()
                        },
                        checked_at_unix_ms,
                    }
                }
                Err(error) => ProviderHealth {
                    availability: ProviderAvailability::Degraded,
                    message: error.message,
                    checked_at_unix_ms,
                },
            },
            Err(error) => ProviderHealth {
                availability: ProviderAvailability::Unavailable,
                message: error.message,
                checked_at_unix_ms,
            },
        }
    }

    async fn models(&self) -> Result<Vec<ProviderModel>, ProviderError> {
        let result = self
            .client()
            .await?
            .request("model/list", json!({"limit": 100, "includeHidden": false}))
            .await?;
        let entries = result
            .get("data")
            .and_then(Value::as_array)
            .ok_or_else(|| protocol_error("model/list omitted data"))?;
        Ok(entries
            .iter()
            .filter_map(|entry| {
                let id = entry
                    .get("id")
                    .or_else(|| entry.get("model"))?
                    .as_str()?
                    .to_owned();
                Some(ProviderModel {
                    display_name: entry
                        .get("displayName")
                        .and_then(Value::as_str)
                        .unwrap_or(&id)
                        .to_owned(),
                    id,
                    context_window_tokens: None,
                    supports_reasoning: entry
                        .get("supportedReasoningEfforts")
                        .and_then(Value::as_array)
                        .is_some_and(|values| !values.is_empty()),
                })
            })
            .collect())
    }

    async fn start(&self, request: StartRequest) -> Result<ProviderRun, ProviderError> {
        reject_attachments(&request.attachments)?;
        let client = self.client().await?;
        let mut params = json!({"serviceName": "homebot"});
        if let Some(model) = &request.model {
            params["model"] = Value::String(model.clone());
        }
        if let Some(cwd) = &self.profile.working_directory {
            params["cwd"] = Value::String(cwd.to_string_lossy().into_owned());
        }
        let result = client.request("thread/start", params).await?;
        let conversation_id = string_at(&result, &["thread", "id"])
            .ok_or_else(|| protocol_error("thread/start omitted thread.id"))?;
        self.begin_turn(
            request.operation_id,
            conversation_id,
            request.prompt,
            request.model,
            request.mode,
        )
        .await
    }

    async fn resume(&self, request: ResumeRequest) -> Result<ProviderRun, ProviderError> {
        reject_attachments(&request.attachments)?;
        self.client()
            .await?
            .request(
                "thread/resume",
                json!({"threadId": request.conversation_id}),
            )
            .await?;
        self.begin_turn(
            request.operation_id,
            request.conversation_id,
            request.prompt,
            request.model,
            request.mode,
        )
        .await
    }

    async fn cancel(&self, operation_id: Uuid) -> Result<(), ProviderError> {
        let client = self.client().await?;
        let (thread_id, turn_id) = client.operation_scope(operation_id).await?;
        client
            .request(
                "turn/interrupt",
                json!({"threadId": thread_id, "turnId": turn_id}),
            )
            .await?;
        Ok(())
    }

    async fn resolve_approval(
        &self,
        approval_id: Uuid,
        decision: ApprovalDecision,
    ) -> Result<(), ProviderError> {
        self.client()
            .await?
            .resolve_approval(approval_id, decision)
            .await
    }

    async fn compact(&self, request: CompactRequest) -> Result<(), ProviderError> {
        self.client()
            .await?
            .request(
                "thread/compact/start",
                json!({"threadId": request.conversation_id}),
            )
            .await?;
        Ok(())
    }

    async fn recover(&self) -> Result<Vec<ProviderRun>, ProviderError> {
        Ok(Vec::new())
    }
}

struct Route {
    operation_id: Uuid,
    thread_id: String,
    turn_id: Option<String>,
    events: mpsc::Sender<ProviderEvent>,
    item_ids: HashMap<String, Uuid>,
    last_error: Option<ProviderError>,
}

struct PendingApproval {
    request_id: Value,
    thread_id: String,
}

struct CodexClient {
    writer: Mutex<ChildStdin>,
    next_request_id: AtomicU64,
    pending: Mutex<HashMap<u64, oneshot::Sender<Result<Value, ProviderError>>>>,
    routes: Mutex<HashMap<Uuid, Route>>,
    approvals: Mutex<HashMap<Uuid, PendingApproval>>,
    alive: AtomicBool,
}

impl CodexClient {
    async fn spawn(profile: &CodexProfile) -> Result<Arc<Self>, ProviderError> {
        let binary = resolve_binary(profile).ok_or_else(|| ProviderError {
            code: ProviderErrorCode::NotInstalled,
            message: "Codex CLI was not found at the configured path".to_owned(),
            retryable: false,
            diagnostic_id: None,
        })?;
        let mut spec = ProcessSpec::new(binary)
            .arg("app-server")
            .arg("--listen")
            .arg("stdio://");
        for (key, value) in &profile.environment {
            spec = spec.environment(key, value);
        }
        if let Some(cwd) = &profile.working_directory {
            spec = spec.current_dir(cwd);
        }
        let mut process = SupervisedProcess::spawn(spec).map_err(process_error)?;
        let stdin = process
            .take_stdin()
            .ok_or_else(|| protocol_error("Codex App Server stdin is unavailable"))?;
        let stdout = process
            .take_stdout()
            .ok_or_else(|| protocol_error("Codex App Server stdout is unavailable"))?;
        let client = Arc::new(Self {
            writer: Mutex::new(stdin),
            next_request_id: AtomicU64::new(1),
            pending: Mutex::new(HashMap::new()),
            routes: Mutex::new(HashMap::new()),
            approvals: Mutex::new(HashMap::new()),
            alive: AtomicBool::new(true),
        });
        let reader_client = Arc::clone(&client);
        tokio::spawn(async move {
            if let Err(error) = reader_client.read_loop(stdout).await {
                reader_client.fail_all(error).await;
            }
        });
        let monitor_client = Arc::clone(&client);
        tokio::spawn(async move {
            let error = match process.wait().await {
                Ok(report) => ProviderError {
                    code: ProviderErrorCode::ProcessCrashed,
                    message: format!(
                        "Codex App Server exited (diagnostic {})",
                        report.diagnostic_id
                    ),
                    retryable: true,
                    diagnostic_id: Some(report.diagnostic_id),
                },
                Err(error) => process_error(error),
            };
            monitor_client.fail_all(error).await;
        });
        client
            .request(
                "initialize",
                json!({
                    "clientInfo": {
                        "name": "homebot",
                        "title": "HomeBot",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                }),
            )
            .await?;
        client.notify("initialized", json!({})).await?;
        Ok(client)
    }

    fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, ProviderError> {
        if !self.is_alive() {
            return Err(unavailable("Codex App Server is not running"));
        }
        let id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);
        if let Err(error) = self
            .write_message(&json!({"method": method, "id": id, "params": params}))
            .await
        {
            self.pending.lock().await.remove(&id);
            return Err(error);
        }
        match tokio::time::timeout(RPC_TIMEOUT, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(unavailable("Codex App Server closed the request")),
            Err(_) => {
                self.pending.lock().await.remove(&id);
                Err(ProviderError {
                    code: ProviderErrorCode::TimedOut,
                    message: format!("Codex App Server did not answer {method}"),
                    retryable: true,
                    diagnostic_id: Some(Uuid::now_v7()),
                })
            }
        }
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), ProviderError> {
        self.write_message(&json!({"method": method, "params": params}))
            .await
    }

    async fn write_message(&self, message: &Value) -> Result<(), ProviderError> {
        let mut bytes = serde_json::to_vec(message)
            .map_err(|_| protocol_error("Could not encode Codex App Server message"))?;
        bytes.push(b'\n');
        let mut writer = self.writer.lock().await;
        writer.write_all(&bytes).await.map_err(io_error)?;
        writer.flush().await.map_err(io_error)
    }

    async fn read_loop(self: &Arc<Self>, stdout: ChildStdout) -> Result<(), ProviderError> {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        loop {
            line.clear();
            let read = reader.read_line(&mut line).await.map_err(io_error)?;
            if read == 0 {
                return Err(unavailable("Codex App Server closed its event stream"));
            }
            if line.len() > MAX_PROTOCOL_LINE_BYTES {
                return Err(protocol_error(
                    "Codex App Server message exceeded the limit",
                ));
            }
            let message: Value = serde_json::from_str(&line)
                .map_err(|_| protocol_error("Codex App Server emitted invalid JSON"))?;
            self.dispatch(message).await?;
        }
    }

    async fn dispatch(&self, message: Value) -> Result<(), ProviderError> {
        if message.get("method").is_none() {
            let id = message
                .get("id")
                .and_then(Value::as_u64)
                .ok_or_else(|| protocol_error("Codex response omitted numeric id"))?;
            if let Some(sender) = self.pending.lock().await.remove(&id) {
                let result = if let Some(error) = message.get("error") {
                    Err(rpc_error(error))
                } else {
                    Ok(message.get("result").cloned().unwrap_or(Value::Null))
                };
                let _ = sender.send(result);
            }
            return Ok(());
        }
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .ok_or_else(|| protocol_error("Codex message method was not a string"))?;
        let params = message.get("params").cloned().unwrap_or_else(|| json!({}));
        if let Some(request_id) = message.get("id") {
            return self
                .handle_server_request(method, request_id.clone(), &params)
                .await;
        }
        self.handle_notification(method, &params).await
    }

    async fn handle_notification(&self, method: &str, params: &Value) -> Result<(), ProviderError> {
        let thread_id =
            string_at(params, &["threadId"]).or_else(|| string_at(params, &["thread", "id"]));
        let (operation_id, events, normalized) = {
            let mut routes = self.routes.lock().await;
            let route = thread_id
                .as_deref()
                .and_then(|id| routes.values_mut().find(|route| route.thread_id == id));
            let Some(route) = route else {
                return Ok(());
            };
            if method == "error" {
                route.last_error = Some(normalize_codex_error(params));
                return Ok(());
            }
            let item_key =
                string_at(params, &["itemId"]).or_else(|| string_at(params, &["item", "id"]));
            let activity_id = item_key.as_ref().map(|key| {
                *route
                    .item_ids
                    .entry(key.clone())
                    .or_insert_with(Uuid::now_v7)
            });
            (
                route.operation_id,
                route.events.clone(),
                notification_events(method, params, activity_id, route.last_error.clone()),
            )
        };
        let terminal = normalized.iter().any(|event| {
            matches!(
                event,
                ProviderEvent::Completed | ProviderEvent::Cancelled | ProviderEvent::Failed { .. }
            )
        });
        for event in normalized {
            events
                .send(event)
                .await
                .map_err(|_| unavailable("HomeBot stopped receiving Codex events"))?;
        }
        if method == "serverRequest/resolved" {
            let request_id = params.get("requestId");
            self.approvals
                .lock()
                .await
                .retain(|_, approval| request_id != Some(&approval.request_id));
            return Ok(());
        }
        if terminal {
            self.routes.lock().await.remove(&operation_id);
            if let Some(thread_id) = thread_id {
                self.approvals
                    .lock()
                    .await
                    .retain(|_, approval| approval.thread_id != thread_id);
            }
        }
        Ok(())
    }

    async fn handle_server_request(
        &self,
        method: &str,
        request_id: Value,
        params: &Value,
    ) -> Result<(), ProviderError> {
        if !matches!(
            method,
            "item/commandExecution/requestApproval" | "item/fileChange/requestApproval"
        ) {
            return self
                .write_message(&json!({
                    "id": request_id,
                    "error": {"code": -32601, "message": "HomeBot does not support this request"}
                }))
                .await;
        }
        let thread_id = string_at(params, &["threadId"])
            .ok_or_else(|| protocol_error("Codex approval omitted threadId"))?;
        let route_sender = {
            let routes = self.routes.lock().await;
            routes
                .values()
                .find(|route| route.thread_id == thread_id)
                .map(|route| route.events.clone())
        };
        let Some(events) = route_sender else {
            return self
                .write_message(&json!({"id": request_id, "result": {"decision": "decline"}}))
                .await;
        };
        let approval_id = Uuid::now_v7();
        self.approvals.lock().await.insert(
            approval_id,
            PendingApproval {
                request_id,
                thread_id: thread_id.clone(),
            },
        );
        let command = params
            .get("command")
            .map_or_else(|| "proposed file changes".to_owned(), display_json);
        let resource = string_at(params, &["cwd"])
            .or_else(|| string_at(params, &["grantRoot"]))
            .unwrap_or_else(|| thread_id.clone());
        events
            .send(ProviderEvent::ApprovalRequired {
                approval: ProviderApproval {
                    approval_id,
                    capability: if method.contains("commandExecution") {
                        "terminal.execute"
                    } else {
                        "filesystem.write"
                    }
                    .to_owned(),
                    action: command,
                    resource,
                    reason: string_at(params, &["reason"])
                        .unwrap_or_else(|| "Codex requested approval".to_owned()),
                },
            })
            .await
            .map_err(|_| unavailable("HomeBot stopped receiving Codex approvals"))
    }

    async fn register_route(
        &self,
        operation_id: Uuid,
        thread_id: String,
        events: mpsc::Sender<ProviderEvent>,
    ) -> Result<(), ProviderError> {
        {
            let mut routes = self.routes.lock().await;
            if routes.contains_key(&operation_id) {
                return Err(invalid_request("Codex operation is already active"));
            }
            routes.insert(
                operation_id,
                Route {
                    operation_id,
                    thread_id: thread_id.clone(),
                    turn_id: None,
                    events: events.clone(),
                    item_ids: HashMap::new(),
                    last_error: None,
                },
            );
        }
        if events
            .send(ProviderEvent::ConversationStarted {
                conversation_id: thread_id,
            })
            .await
            .is_err()
        {
            self.routes.lock().await.remove(&operation_id);
            return Err(unavailable("HomeBot stopped receiving Codex events"));
        }
        Ok(())
    }

    async fn set_turn(&self, operation_id: Uuid, turn_id: String) -> Result<(), ProviderError> {
        let mut routes = self.routes.lock().await;
        let route = routes
            .get_mut(&operation_id)
            .ok_or_else(|| invalid_request("Codex operation is no longer active"))?;
        route.turn_id = Some(turn_id);
        Ok(())
    }

    async fn remove_route(&self, operation_id: Uuid) {
        self.routes.lock().await.remove(&operation_id);
    }

    async fn operation_scope(&self, operation_id: Uuid) -> Result<(String, String), ProviderError> {
        let routes = self.routes.lock().await;
        let route = routes
            .get(&operation_id)
            .ok_or_else(|| invalid_request("Codex operation is not active"))?;
        let turn_id = route
            .turn_id
            .clone()
            .ok_or_else(|| invalid_request("Codex turn has not started"))?;
        Ok((route.thread_id.clone(), turn_id))
    }

    async fn resolve_approval(
        &self,
        approval_id: Uuid,
        decision: ApprovalDecision,
    ) -> Result<(), ProviderError> {
        let pending = self
            .approvals
            .lock()
            .await
            .remove(&approval_id)
            .ok_or_else(|| invalid_request("Codex approval is no longer pending"))?;
        let decision = match decision {
            ApprovalDecision::AllowOnce => "accept",
            ApprovalDecision::AllowForSession => "acceptForSession",
            ApprovalDecision::Deny => "decline",
            ApprovalDecision::Cancel => "cancel",
        };
        self.write_message(&json!({
            "id": pending.request_id,
            "result": {"decision": decision}
        }))
        .await
    }

    async fn fail_all(&self, error: ProviderError) {
        if !self.alive.swap(false, Ordering::AcqRel) {
            return;
        }
        for (_, sender) in self.pending.lock().await.drain() {
            let _ = sender.send(Err(error.clone()));
        }
        let routes = self
            .routes
            .lock()
            .await
            .drain()
            .map(|(_, route)| route)
            .collect::<Vec<_>>();
        for route in routes {
            let _ = route
                .events
                .send(ProviderEvent::Failed {
                    error: error.clone(),
                })
                .await;
        }
        self.approvals.lock().await.clear();
    }
}

fn resolve_binary(profile: &CodexProfile) -> Option<PathBuf> {
    let configured = &profile.binary_path;
    if configured.components().count() > 1 || configured.is_absolute() {
        return configured.is_file().then(|| configured.clone());
    }
    let path = profile.environment.get(&OsString::from("PATH"))?;
    std::env::split_paths(path)
        .map(|directory| directory.join(configured))
        .find(|candidate| candidate.is_file())
}

fn safe_codex_environment() -> BTreeMap<OsString, OsString> {
    const KEYS: &[&str] = &[
        "PATH",
        "HOME",
        "USER",
        "TMPDIR",
        "CODEX_HOME",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_CACHE_HOME",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
    ];
    KEYS.iter()
        .filter_map(|key| std::env::var_os(key).map(|value| (OsString::from(key), value)))
        .collect()
}

fn reject_attachments(attachments: &[crate::ProviderAttachment]) -> Result<(), ProviderError> {
    if attachments.is_empty() {
        Ok(())
    } else {
        Err(invalid_request(
            "Codex attachments require a resolved local path and cannot be sent from metadata alone",
        ))
    }
}

fn string_at(value: &Value, path: &[&str]) -> Option<String> {
    path.iter()
        .try_fold(value, |current, key| current.get(*key))?
        .as_str()
        .map(ToOwned::to_owned)
}

fn display_json(value: &Value) -> String {
    value
        .as_str()
        .map_or_else(|| value.to_string(), ToOwned::to_owned)
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn protocol_error(message: impl Into<String>) -> ProviderError {
    ProviderError {
        code: ProviderErrorCode::ProtocolViolation,
        message: message.into(),
        retryable: false,
        diagnostic_id: Some(Uuid::now_v7()),
    }
}

fn invalid_request(message: impl Into<String>) -> ProviderError {
    ProviderError {
        code: ProviderErrorCode::InvalidRequest,
        message: message.into(),
        retryable: false,
        diagnostic_id: None,
    }
}

fn unavailable(message: impl Into<String>) -> ProviderError {
    ProviderError {
        code: ProviderErrorCode::Unavailable,
        message: message.into(),
        retryable: true,
        diagnostic_id: Some(Uuid::now_v7()),
    }
}

fn io_error(_error: std::io::Error) -> ProviderError {
    unavailable("Codex App Server transport failed")
}

fn process_error(_error: crate::ProviderProcessError) -> ProviderError {
    ProviderError {
        code: ProviderErrorCode::ProcessCrashed,
        message: "Codex App Server process could not be supervised".to_owned(),
        retryable: true,
        diagnostic_id: Some(Uuid::now_v7()),
    }
}

#[cfg(test)]
#[path = "codex/tests.rs"]
mod tests;
