//! Constrained JSONL process adapter for community backends.

use crate::{
    ApprovalDecision, CompactRequest, ProcessSpec, ProviderAdapter, ProviderAdapterId,
    ProviderAvailability, ProviderCapabilities, ProviderCapability, ProviderDescriptor,
    ProviderError, ProviderErrorCode, ProviderEvent, ProviderHealth, ProviderModel, ProviderRun,
    ResumeRequest, StartRequest, SupervisedProcess,
    supervisor::{BoundedLine, read_bounded_line},
};
use serde::Serialize;
use std::{
    collections::{BTreeMap, HashMap},
    ffi::OsString,
    path::PathBuf,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncWriteExt, BufReader},
    sync::{Mutex, mpsc, watch},
};
use uuid::Uuid;

const EVENT_BUFFER: usize = 128;
const MAX_LINE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct GenericProcessProfile {
    pub adapter_id: ProviderAdapterId,
    pub display_name: String,
    pub program: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
    pub environment: BTreeMap<OsString, OsString>,
    pub models: Vec<ProviderModel>,
}

impl GenericProcessProfile {
    #[must_use]
    pub fn new(
        adapter_id: ProviderAdapterId,
        display_name: impl Into<String>,
        program: impl Into<PathBuf>,
    ) -> Self {
        Self {
            adapter_id,
            display_name: display_name.into(),
            program: program.into(),
            arguments: Vec::new(),
            working_directory: None,
            environment: safe_generic_environment(),
            models: Vec::new(),
        }
    }

    #[must_use]
    pub fn argument(mut self, argument: impl Into<OsString>) -> Self {
        self.arguments.push(argument.into());
        self
    }

    #[must_use]
    pub fn environment(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.environment.insert(key.into(), value.into());
        self
    }

    #[must_use]
    pub fn working_directory(mut self, path: impl Into<PathBuf>) -> Self {
        self.working_directory = Some(path.into());
        self
    }

    #[must_use]
    pub fn models(mut self, models: Vec<ProviderModel>) -> Self {
        self.models = models;
        self
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum GenericRequest {
    Start(StartRequest),
    Resume(ResumeRequest),
}

pub struct GenericProcessAdapter {
    profile: GenericProcessProfile,
    operations: Arc<Mutex<HashMap<Uuid, watch::Sender<bool>>>>,
}

impl std::fmt::Debug for GenericProcessAdapter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GenericProcessAdapter")
            .field("adapter_id", &self.profile.adapter_id)
            .field("program", &self.profile.program.file_name())
            .field("argument_count", &self.profile.arguments.len())
            .field("environment_keys", &self.profile.environment.keys())
            .finish_non_exhaustive()
    }
}

impl GenericProcessAdapter {
    #[must_use]
    pub fn new(profile: GenericProcessProfile) -> Self {
        Self {
            profile,
            operations: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    async fn run(
        &self,
        operation_id: Uuid,
        request: GenericRequest,
    ) -> Result<ProviderRun, ProviderError> {
        let program = resolve_program(&self.profile).ok_or_else(not_installed)?;
        let (events_tx, events_rx) = mpsc::channel(EVENT_BUFFER);
        let (cancel_tx, mut cancel_rx) = watch::channel(false);
        let mut operations = self.operations.lock().await;
        if operations.insert(operation_id, cancel_tx).is_some() {
            return Err(invalid_request(
                "Generic provider operation is already active",
            ));
        }
        drop(operations);
        let working_directory = match &request {
            GenericRequest::Start(request) => request.working_directory.clone(),
            GenericRequest::Resume(request) => request.working_directory.clone(),
        }
        .or_else(|| self.profile.working_directory.clone());
        let setup = async {
            let mut spec = ProcessSpec::new(program);
            for argument in &self.profile.arguments {
                spec = spec.arg(argument);
            }
            for (key, value) in &self.profile.environment {
                spec = spec.environment(key, value);
            }
            if let Some(cwd) = &working_directory {
                spec = spec.current_dir(cwd);
            }
            let mut process = SupervisedProcess::spawn(spec).map_err(process_error)?;
            let mut stdin = process
                .take_stdin()
                .ok_or_else(|| protocol_error("Generic provider stdin is unavailable"))?;
            let stdout = process
                .take_stdout()
                .ok_or_else(|| protocol_error("Generic provider stdout is unavailable"))?;
            let mut input = serde_json::to_vec(&request)
                .map_err(|_| protocol_error("Could not encode generic provider request"))?;
            input.push(b'\n');
            stdin.write_all(&input).await.map_err(io_error)?;
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
            consume_generic(process, stdin, stdout, &mut cancel_rx, events_tx).await;
            active.lock().await.remove(&operation_id);
        });
        Ok(ProviderRun {
            operation_id,
            events: events_rx,
        })
    }
}

async fn consume_generic(
    process: SupervisedProcess,
    stdin: tokio::process::ChildStdin,
    stdout: tokio::process::ChildStdout,
    cancel: &mut watch::Receiver<bool>,
    events: mpsc::Sender<ProviderEvent>,
) {
    let mut reader = BufReader::new(stdout);
    let mut terminal = false;
    let mut cancelled = false;
    loop {
        tokio::select! {
            changed = cancel.changed() => {
                if changed.is_err() || *cancel.borrow() {
                    cancelled = true;
                    break;
                }
            }
            read = read_bounded_line(&mut reader, MAX_LINE_BYTES) => {
                match read {
                    Ok(BoundedLine::Eof) | Err(_) => break,
                    Ok(BoundedLine::TooLong) => {
                        let _ = events.send(ProviderEvent::Failed { error: protocol_error("Generic provider message exceeded the limit") }).await;
                        terminal = true;
                        break;
                    }
                    Ok(BoundedLine::Line(line)) => {
                        let Ok(event) = serde_json::from_str::<ProviderEvent>(&line) else {
                            let _ = events.send(ProviderEvent::Failed { error: protocol_error("Generic provider emitted invalid HomeBot event JSON") }).await;
                            terminal = true;
                            break;
                        };
                        terminal = is_terminal(&event);
                        if events.send(event).await.is_err() || terminal {
                            break;
                        }
                    }
                }
            }
        }
    }
    drop(stdin);
    let report = process.shutdown().await;
    if cancelled {
        let _ = events.send(ProviderEvent::Cancelled).await;
    } else if !terminal {
        let error = report.map_or_else(process_error, |report| ProviderError {
            code: ProviderErrorCode::ProcessCrashed,
            message: format!(
                "Generic provider ended before a terminal event (diagnostic {})",
                report.diagnostic_id
            ),
            retryable: true,
            diagnostic_id: Some(report.diagnostic_id),
        });
        let _ = events.send(ProviderEvent::Failed { error }).await;
    }
}

#[async_trait::async_trait]
impl ProviderAdapter for GenericProcessAdapter {
    fn id(&self) -> &ProviderAdapterId {
        &self.profile.adapter_id
    }

    async fn discover(&self) -> Result<ProviderDescriptor, ProviderError> {
        Ok(ProviderDescriptor {
            adapter_id: self.profile.adapter_id.clone(),
            display_name: self.profile.display_name.clone(),
            executable: resolve_program(&self.profile).map(|path| path.display().to_string()),
            capabilities: ProviderCapabilities {
                supported: [
                    ProviderCapability::ConversationResume,
                    ProviderCapability::Streaming,
                    ProviderCapability::Activities,
                    ProviderCapability::Cancellation,
                    ProviderCapability::Usage,
                ]
                .into_iter()
                .collect(),
            },
        })
    }

    async fn health(&self) -> ProviderHealth {
        ProviderHealth {
            availability: if resolve_program(&self.profile).is_some() {
                ProviderAvailability::Available
            } else {
                ProviderAvailability::NotInstalled
            },
            message: if resolve_program(&self.profile).is_some() {
                "Generic provider executable is available".to_owned()
            } else {
                "Generic provider executable was not found".to_owned()
            },
            checked_at_unix_ms: unix_ms(),
        }
    }

    async fn models(&self) -> Result<Vec<ProviderModel>, ProviderError> {
        Ok(self.profile.models.clone())
    }

    async fn start(&self, request: StartRequest) -> Result<ProviderRun, ProviderError> {
        let operation_id = request.operation_id;
        self.run(operation_id, GenericRequest::Start(request)).await
    }

    async fn resume(&self, request: ResumeRequest) -> Result<ProviderRun, ProviderError> {
        let operation_id = request.operation_id;
        self.run(operation_id, GenericRequest::Resume(request))
            .await
    }

    async fn cancel(&self, operation_id: Uuid) -> Result<(), ProviderError> {
        self.operations
            .lock()
            .await
            .get(&operation_id)
            .ok_or_else(|| invalid_request("Generic provider operation is not active"))?
            .send(true)
            .map_err(|_| invalid_request("Generic provider operation is not active"))
    }

    async fn resolve_approval(
        &self,
        _approval_id: Uuid,
        _decision: ApprovalDecision,
    ) -> Result<(), ProviderError> {
        Err(invalid_request(
            "Generic provider does not advertise interactive approvals",
        ))
    }

    async fn compact(&self, _request: CompactRequest) -> Result<(), ProviderError> {
        Err(invalid_request(
            "Generic provider does not advertise manual compaction",
        ))
    }

    async fn recover(&self) -> Result<Vec<ProviderRun>, ProviderError> {
        Ok(Vec::new())
    }
}

fn resolve_program(profile: &GenericProcessProfile) -> Option<PathBuf> {
    if profile.program.components().count() > 1 || profile.program.is_absolute() {
        return profile.program.is_file().then(|| profile.program.clone());
    }
    let path = profile.environment.get(&OsString::from("PATH"))?;
    std::env::split_paths(path)
        .map(|directory| directory.join(&profile.program))
        .find(|candidate| candidate.is_file())
}

fn safe_generic_environment() -> BTreeMap<OsString, OsString> {
    let inherited = std::env::var_os("PATH");
    crate::discovery::executable_search_path(inherited.as_ref())
        .map(|path| [(OsString::from("PATH"), path)].into_iter().collect())
        .unwrap_or_default()
}

fn is_terminal(event: &ProviderEvent) -> bool {
    matches!(
        event,
        ProviderEvent::Completed | ProviderEvent::Cancelled | ProviderEvent::Failed { .. }
    )
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
        message: "Generic provider executable was not found".to_owned(),
        retryable: false,
        diagnostic_id: None,
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

fn protocol_error(message: impl Into<String>) -> ProviderError {
    ProviderError {
        code: ProviderErrorCode::ProtocolViolation,
        message: message.into(),
        retryable: false,
        diagnostic_id: Some(Uuid::now_v7()),
    }
}

fn io_error(_error: std::io::Error) -> ProviderError {
    ProviderError {
        code: ProviderErrorCode::Unavailable,
        message: "Generic provider transport failed".to_owned(),
        retryable: true,
        diagnostic_id: Some(Uuid::now_v7()),
    }
}

fn process_error(_error: crate::ProviderProcessError) -> ProviderError {
    ProviderError {
        code: ProviderErrorCode::ProcessCrashed,
        message: "Generic provider process could not be supervised".to_owned(),
        retryable: true,
        diagnostic_id: Some(Uuid::now_v7()),
    }
}

#[cfg(test)]
#[path = "generic_process/tests.rs"]
mod tests;
