//! Claude Code adapter using the supported structured `stream-json` CLI surface.

#[path = "claude/protocol.rs"]
mod protocol;

use crate::{
    ApprovalDecision, CompactRequest, ExecutionMode, ProcessSpec, ProviderAdapter,
    ProviderAdapterId, ProviderAvailability, ProviderCapabilities, ProviderCapability,
    ProviderDescriptor, ProviderError, ProviderErrorCode, ProviderEvent, ProviderHealth,
    ProviderModel, ProviderRun, ResumeRequest, StartRequest, SupervisedProcess,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    ffi::OsString,
    path::PathBuf,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{ChildStdin, ChildStdout},
    sync::{Mutex, mpsc, watch},
};
use uuid::Uuid;

use protocol::normalize_message;

const EVENT_BUFFER: usize = 128;
const MAX_LINE_BYTES: usize = 4 * 1024 * 1024;
const PROBE_BYTES: u64 = 64 * 1024;

#[derive(Clone, Debug)]
pub struct ClaudeProfile {
    pub adapter_id: ProviderAdapterId,
    pub binary_path: PathBuf,
    pub working_directory: Option<PathBuf>,
    pub environment: BTreeMap<OsString, OsString>,
}

impl ClaudeProfile {
    #[must_use]
    pub fn new(adapter_id: ProviderAdapterId, binary_path: impl Into<PathBuf>) -> Self {
        Self {
            adapter_id,
            binary_path: binary_path.into(),
            working_directory: None,
            environment: safe_claude_environment(),
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

pub struct ClaudeAdapter {
    profile: ClaudeProfile,
    operations: Arc<Mutex<HashMap<Uuid, watch::Sender<bool>>>>,
}

impl std::fmt::Debug for ClaudeAdapter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ClaudeAdapter")
            .field("adapter_id", &self.profile.adapter_id)
            .field("binary", &self.profile.binary_path.file_name())
            .field("working_directory", &self.profile.working_directory)
            .field("environment_keys", &self.profile.environment.keys())
            .finish_non_exhaustive()
    }
}

impl ClaudeAdapter {
    #[must_use]
    pub fn new(profile: ClaudeProfile) -> Self {
        Self {
            profile,
            operations: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    async fn run(
        &self,
        operation_id: Uuid,
        conversation_id: Option<String>,
        prompt: String,
        model: Option<String>,
        mode: ExecutionMode,
    ) -> Result<ProviderRun, ProviderError> {
        let binary = resolve_binary(&self.profile).ok_or_else(not_installed)?;
        let (events_tx, events_rx) = mpsc::channel(EVENT_BUFFER);
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let mut operations = self.operations.lock().await;
        if operations.insert(operation_id, cancel_tx).is_some() {
            return Err(invalid_request("Claude Code operation is already active"));
        }
        drop(operations);
        let setup = async {
            let mut spec = ProcessSpec::new(binary)
                .arg("-p")
                .arg("--input-format")
                .arg("stream-json")
                .arg("--output-format")
                .arg("stream-json")
                .arg("--verbose")
                .arg("--include-partial-messages")
                .arg("--replay-user-messages");
            if let Some(conversation_id) = &conversation_id {
                spec = spec.arg("--resume").arg(conversation_id);
            }
            if let Some(model) = &model {
                spec = spec.arg("--model").arg(model);
            }
            if mode == ExecutionMode::Plan {
                spec = spec.arg("--permission-mode").arg("plan");
            }
            if let Some(cwd) = &self.profile.working_directory {
                spec = spec.current_dir(cwd);
            }
            for (key, value) in &self.profile.environment {
                spec = spec.environment(key, value);
            }
            let mut process = SupervisedProcess::spawn(spec).map_err(process_error)?;
            let mut stdin = process
                .take_stdin()
                .ok_or_else(|| protocol_error("Claude Code stdin is unavailable"))?;
            let stdout = process
                .take_stdout()
                .ok_or_else(|| protocol_error("Claude Code stdout is unavailable"))?;
            let input = json!({
                "type": "user",
                "message": {"role": "user", "content": prompt},
                "parent_tool_use_id": null
            });
            let mut encoded = serde_json::to_vec(&input)
                .map_err(|_| protocol_error("Could not encode Claude Code input"))?;
            encoded.push(b'\n');
            stdin.write_all(&encoded).await.map_err(io_error)?;
            stdin.flush().await.map_err(io_error)?;
            Ok::<_, ProviderError>((process, stdin, stdout))
        }
        .await;
        let (process, stdin, stdout) = match setup {
            Ok(setup) => setup,
            Err(error) => {
                self.operations.lock().await.remove(&operation_id);
                return Err(error);
            }
        };
        let active = Arc::clone(&self.operations);
        tokio::spawn(async move {
            run_process(
                conversation_id,
                process,
                stdin,
                stdout,
                cancel_rx,
                events_tx,
            )
            .await;
            active.lock().await.remove(&operation_id);
        });
        Ok(ProviderRun {
            operation_id,
            events: events_rx,
        })
    }
}

#[async_trait::async_trait]
impl ProviderAdapter for ClaudeAdapter {
    fn id(&self) -> &ProviderAdapterId {
        &self.profile.adapter_id
    }

    async fn discover(&self) -> Result<ProviderDescriptor, ProviderError> {
        Ok(ProviderDescriptor {
            adapter_id: self.profile.adapter_id.clone(),
            display_name: "Claude".to_owned(),
            executable: resolve_binary(&self.profile).map(|path| path.display().to_string()),
            capabilities: ProviderCapabilities {
                supported: [
                    ProviderCapability::ConversationResume,
                    ProviderCapability::Streaming,
                    ProviderCapability::Activities,
                    ProviderCapability::Cancellation,
                    ProviderCapability::Usage,
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
                message: "Claude Code was not found at the configured path".to_owned(),
                checked_at_unix_ms,
            };
        }
        match probe(&self.profile, &["auth", "status"]).await {
            Ok((true, output)) => {
                let authenticated = serde_json::from_slice::<Value>(&output)
                    .ok()
                    .and_then(|value| {
                        value
                            .get("loggedIn")
                            .or_else(|| value.get("authenticated"))
                            .and_then(Value::as_bool)
                    })
                    .unwrap_or(true);
                ProviderHealth {
                    availability: if authenticated {
                        ProviderAvailability::Available
                    } else {
                        ProviderAvailability::AuthenticationRequired
                    },
                    message: if authenticated {
                        "Claude Code is ready".to_owned()
                    } else {
                        "Claude Code requires authentication".to_owned()
                    },
                    checked_at_unix_ms,
                }
            }
            Ok((false, _)) => ProviderHealth {
                availability: ProviderAvailability::AuthenticationRequired,
                message: "Claude Code requires authentication".to_owned(),
                checked_at_unix_ms,
            },
            Err(error) => ProviderHealth {
                availability: ProviderAvailability::Degraded,
                message: error.message,
                checked_at_unix_ms,
            },
        }
    }

    async fn models(&self) -> Result<Vec<ProviderModel>, ProviderError> {
        Ok(["sonnet", "opus", "haiku", "fable"]
            .into_iter()
            .map(|id| ProviderModel {
                id: id.to_owned(),
                display_name: format!("Claude {id}"),
                context_window_tokens: None,
                supports_reasoning: true,
            })
            .collect())
    }

    async fn start(&self, request: StartRequest) -> Result<ProviderRun, ProviderError> {
        reject_attachments(&request.attachments)?;
        self.run(
            request.operation_id,
            None,
            request.prompt,
            request.model,
            request.mode,
        )
        .await
    }

    async fn resume(&self, request: ResumeRequest) -> Result<ProviderRun, ProviderError> {
        reject_attachments(&request.attachments)?;
        self.run(
            request.operation_id,
            Some(request.conversation_id),
            request.prompt,
            request.model,
            request.mode,
        )
        .await
    }

    async fn cancel(&self, operation_id: Uuid) -> Result<(), ProviderError> {
        self.operations
            .lock()
            .await
            .get(&operation_id)
            .ok_or_else(|| invalid_request("Claude Code operation is not active"))?
            .send(true)
            .map_err(|_| invalid_request("Claude Code operation is not active"))
    }

    async fn resolve_approval(
        &self,
        _approval_id: Uuid,
        _decision: ApprovalDecision,
    ) -> Result<(), ProviderError> {
        Err(invalid_request(
            "Claude Code CLI approvals require a configured permission prompt tool",
        ))
    }

    async fn compact(&self, _request: CompactRequest) -> Result<(), ProviderError> {
        Err(invalid_request(
            "Claude Code does not expose manual compaction through this CLI surface",
        ))
    }

    async fn recover(&self) -> Result<Vec<ProviderRun>, ProviderError> {
        Ok(Vec::new())
    }
}

enum ProcessOutcome {
    Terminal,
    Cancelled,
    Eof,
    Failed(ProviderError),
}

async fn run_process(
    expected_conversation_id: Option<String>,
    process: SupervisedProcess,
    stdin: ChildStdin,
    stdout: ChildStdout,
    mut cancel: watch::Receiver<bool>,
    events: mpsc::Sender<ProviderEvent>,
) {
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    let outcome = loop {
        line.clear();
        tokio::select! {
            changed = cancel.changed() => {
                if changed.is_err() || *cancel.borrow() {
                    break ProcessOutcome::Cancelled;
                }
            }
            read = reader.read_line(&mut line) => {
                match read {
                    Ok(0) => break ProcessOutcome::Eof,
                    Ok(_) if line.len() > MAX_LINE_BYTES => {
                        break ProcessOutcome::Failed(protocol_error("Claude Code message exceeded the limit"));
                    }
                    Ok(_) => {
                        let Ok(message) = serde_json::from_str::<Value>(&line) else {
                            break ProcessOutcome::Failed(protocol_error("Claude Code emitted invalid JSON"));
                        };
                        let normalized = normalize_message(&message, expected_conversation_id.as_deref());
                        let terminal = normalized.iter().any(is_terminal);
                        let mut receiver_closed = false;
                        for event in normalized {
                            if events.send(event).await.is_err() {
                                receiver_closed = true;
                                break;
                            }
                        }
                        if receiver_closed {
                            break ProcessOutcome::Cancelled;
                        }
                        if terminal {
                            break ProcessOutcome::Terminal;
                        }
                    }
                    Err(_) => break ProcessOutcome::Failed(io_error(std::io::Error::other("read failed"))),
                }
            }
        }
    };
    drop(stdin);
    match outcome {
        ProcessOutcome::Terminal => {
            let _ = process.shutdown().await;
        }
        ProcessOutcome::Cancelled => {
            let _ = process.shutdown().await;
            let _ = events.send(ProviderEvent::Cancelled).await;
        }
        ProcessOutcome::Failed(error) => {
            let _ = process.shutdown().await;
            let _ = events.send(ProviderEvent::Failed { error }).await;
        }
        ProcessOutcome::Eof => {
            let error = match process.wait().await {
                Ok(report) => ProviderError {
                    code: ProviderErrorCode::ProcessCrashed,
                    message: format!(
                        "Claude Code ended before a result (diagnostic {})",
                        report.diagnostic_id
                    ),
                    retryable: true,
                    diagnostic_id: Some(report.diagnostic_id),
                },
                Err(error) => process_error(error),
            };
            let _ = events.send(ProviderEvent::Failed { error }).await;
        }
    }
}

fn is_terminal(event: &ProviderEvent) -> bool {
    matches!(
        event,
        ProviderEvent::Completed | ProviderEvent::Cancelled | ProviderEvent::Failed { .. }
    )
}

async fn probe(
    profile: &ClaudeProfile,
    arguments: &[&str],
) -> Result<(bool, Vec<u8>), ProviderError> {
    let binary = resolve_binary(profile).ok_or_else(not_installed)?;
    let mut spec = ProcessSpec::new(binary);
    for argument in arguments {
        spec = spec.arg(argument);
    }
    for (key, value) in &profile.environment {
        spec = spec.environment(key, value);
    }
    if let Some(cwd) = &profile.working_directory {
        spec = spec.current_dir(cwd);
    }
    let mut process = SupervisedProcess::spawn(spec).map_err(process_error)?;
    let stdout = process
        .take_stdout()
        .ok_or_else(|| protocol_error("Claude Code probe stdout is unavailable"))?;
    let mut output = Vec::new();
    stdout
        .take(PROBE_BYTES + 1)
        .read_to_end(&mut output)
        .await
        .map_err(io_error)?;
    if output.len() as u64 > PROBE_BYTES {
        let _ = process.shutdown().await;
        return Err(protocol_error(
            "Claude Code probe output exceeded the limit",
        ));
    }
    let report = process.wait().await.map_err(process_error)?;
    Ok((report.succeeded(), output))
}

fn resolve_binary(profile: &ClaudeProfile) -> Option<PathBuf> {
    let configured = &profile.binary_path;
    if configured.components().count() > 1 || configured.is_absolute() {
        return configured.is_file().then(|| configured.clone());
    }
    let path = profile.environment.get(&OsString::from("PATH"))?;
    std::env::split_paths(path)
        .map(|directory| directory.join(configured))
        .find(|candidate| candidate.is_file())
}

fn safe_claude_environment() -> BTreeMap<OsString, OsString> {
    const KEYS: &[&str] = &[
        "PATH",
        "HOME",
        "USER",
        "TMPDIR",
        "CLAUDE_CONFIG_DIR",
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
            "Claude attachments require resolved local content and cannot be sent from metadata alone",
        ))
    }
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn not_installed() -> ProviderError {
    ProviderError {
        code: ProviderErrorCode::NotInstalled,
        message: "Claude Code was not found at the configured path".to_owned(),
        retryable: false,
        diagnostic_id: None,
    }
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

fn io_error(_error: std::io::Error) -> ProviderError {
    ProviderError {
        code: ProviderErrorCode::Unavailable,
        message: "Claude Code transport failed".to_owned(),
        retryable: true,
        diagnostic_id: Some(Uuid::now_v7()),
    }
}

fn process_error(_error: crate::ProviderProcessError) -> ProviderError {
    ProviderError {
        code: ProviderErrorCode::ProcessCrashed,
        message: "Claude Code process could not be supervised".to_owned(),
        retryable: true,
        diagnostic_id: Some(Uuid::now_v7()),
    }
}

#[cfg(test)]
#[path = "claude/tests.rs"]
mod tests;
