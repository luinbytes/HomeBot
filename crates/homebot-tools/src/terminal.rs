use crate::{
    ActivityKind, ActivitySink, ActivityStatus, CapabilityClass, CapabilityRequest,
    OperationContext, PolicyEngine, ToolActivity, ToolError,
};
use portable_pty::{
    Child, ChildKiller, CommandBuilder, ExitStatus, MasterPty, NativePtySystem, PtySize, PtySystem,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Mutex, mpsc};
use uuid::Uuid;

const EVENT_BUFFER: usize = 128;

#[derive(Clone, Serialize)]
pub struct TerminalCommand {
    pub program: PathBuf,
    pub arguments: Vec<String>,
    pub working_directory: PathBuf,
    pub environment: BTreeMap<String, String>,
    pub rows: u16,
    pub columns: u16,
}

impl std::fmt::Debug for TerminalCommand {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TerminalCommand")
            .field("program", &self.program.file_name())
            .field("argument_count", &self.arguments.len())
            .field("working_directory", &self.working_directory)
            .field("environment_keys", &self.environment.keys())
            .field("rows", &self.rows)
            .field("columns", &self.columns)
            .finish()
    }
}

#[derive(Clone, Debug)]
pub struct TerminalLimits {
    pub max_output_bytes: usize,
    pub max_input_bytes: usize,
    pub max_runtime: Duration,
    pub max_concurrent_processes: usize,
    pub allowed_environment: BTreeSet<String>,
}

impl Default for TerminalLimits {
    fn default() -> Self {
        Self {
            max_output_bytes: 4 * 1024 * 1024,
            max_input_bytes: 64 * 1024,
            max_runtime: Duration::from_secs(30 * 60),
            max_concurrent_processes: 16,
            allowed_environment: ["HOME", "LANG", "LC_ALL", "PATH", "TERM", "TMPDIR"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TerminalChunk {
    Started { process_id: Option<u32> },
    Output { bytes: Vec<u8> },
    Exited { exit_code: u32, success: bool },
    Cancelled,
    TimedOut,
    Failed { reason: String },
}

type SharedWriter = Arc<StdMutex<Option<Box<dyn Write + Send>>>>;
type SharedMaster = Arc<StdMutex<Box<dyn MasterPty + Send>>>;

pub struct TerminalRun {
    pub operation_id: Uuid,
    pub events: mpsc::Receiver<TerminalChunk>,
    writer: SharedWriter,
    master: SharedMaster,
    max_input_bytes: usize,
}

impl std::fmt::Debug for TerminalRun {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TerminalRun")
            .field("operation_id", &self.operation_id)
            .finish_non_exhaustive()
    }
}

impl TerminalRun {
    /// Writes bounded bytes to the PTY input stream.
    ///
    /// # Errors
    ///
    /// Rejects oversized input and closed or failed PTY writers.
    pub async fn write_input(&self, bytes: Vec<u8>) -> Result<(), ToolError> {
        if bytes.len() > self.max_input_bytes {
            return Err(ToolError::LimitExceeded);
        }
        let writer = Arc::clone(&self.writer);
        tokio::task::spawn_blocking(move || {
            let mut guard = writer.lock().map_err(|_| ToolError::OperationFailed)?;
            let writer = guard.as_mut().ok_or(ToolError::OperationFailed)?;
            writer
                .write_all(&bytes)
                .and_then(|()| writer.flush())
                .map_err(|_| ToolError::OperationFailed)
        })
        .await
        .map_err(|_| ToolError::OperationFailed)?
    }

    /// Resizes the active PTY.
    ///
    /// # Errors
    ///
    /// Rejects zero dimensions and failed PTY resize operations.
    pub async fn resize(&self, rows: u16, columns: u16) -> Result<(), ToolError> {
        if rows == 0 || columns == 0 {
            return Err(ToolError::InvalidRequest(
                "terminal dimensions must be nonzero".to_owned(),
            ));
        }
        let master = Arc::clone(&self.master);
        tokio::task::spawn_blocking(move || {
            master
                .lock()
                .map_err(|_| ToolError::OperationFailed)?
                .resize(PtySize {
                    rows,
                    cols: columns,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .map_err(|_| ToolError::OperationFailed)
        })
        .await
        .map_err(|_| ToolError::OperationFailed)?
    }
}

struct ActiveTerminal {
    cancelled: AtomicBool,
    output_limited: AtomicBool,
    killer: StdMutex<Box<dyn ChildKiller + Send + Sync>>,
}

pub struct TerminalService {
    workspace_root: PathBuf,
    policy: Arc<PolicyEngine>,
    activities: Arc<dyn ActivitySink>,
    limits: TerminalLimits,
    operations: Arc<Mutex<HashMap<Uuid, Option<Arc<ActiveTerminal>>>>>,
}

impl std::fmt::Debug for TerminalService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TerminalService")
            .field("workspace_root", &self.workspace_root)
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl TerminalService {
    /// Creates a PTY service rooted at an existing workspace.
    ///
    /// # Errors
    ///
    /// Returns an error when the workspace cannot be canonicalized.
    pub fn new(
        workspace_root: impl AsRef<Path>,
        policy: Arc<PolicyEngine>,
        activities: Arc<dyn ActivitySink>,
        limits: TerminalLimits,
    ) -> Result<Self, ToolError> {
        let workspace_root =
            std::fs::canonicalize(workspace_root).map_err(|_| ToolError::Unavailable)?;
        if !workspace_root.is_dir() {
            return Err(ToolError::InvalidRequest(
                "workspace root is not a directory".to_owned(),
            ));
        }
        Ok(Self {
            workspace_root,
            policy,
            activities,
            limits,
            operations: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Starts one direct executable in a bounded PTY.
    ///
    /// # Errors
    ///
    /// Requires authorization and rejects unsafe paths, environment keys and spawn failures.
    pub async fn start(
        &self,
        context: OperationContext,
        mut command: TerminalCommand,
        approval_id: Option<Uuid>,
    ) -> Result<TerminalRun, ToolError> {
        let cwd = self.validate_command(&mut command)?;
        let request = command_request(&context, &command, &cwd)?;
        let _authorization = self.policy.authorize(&request, approval_id).await?;
        let operation_id = context.operation_id;
        let mut operations = self.operations.lock().await;
        if operations.len() >= self.limits.max_concurrent_processes {
            return Err(ToolError::LimitExceeded);
        }
        match operations.entry(operation_id) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(None);
            }
            std::collections::hash_map::Entry::Occupied(_) => {
                return Err(ToolError::InvalidRequest(
                    "terminal operation is already active".to_owned(),
                ));
            }
        }
        drop(operations);
        self.activities
            .emit(ToolActivity::new(
                operation_id,
                ActivityKind::Terminal,
                ActivityStatus::Started,
                "Started terminal command",
                command
                    .program
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned()),
            ))
            .await;
        let spawned = match tokio::task::spawn_blocking(move || spawn_pty(&command, &cwd)).await {
            Ok(Ok(spawned)) => spawned,
            Ok(Err(error)) => {
                self.operations.lock().await.remove(&operation_id);
                self.emit_start_failure(operation_id).await;
                return Err(error);
            }
            Err(_) => {
                self.operations.lock().await.remove(&operation_id);
                self.emit_start_failure(operation_id).await;
                return Err(ToolError::OperationFailed);
            }
        };
        let active = Arc::new(ActiveTerminal {
            cancelled: AtomicBool::new(false),
            output_limited: AtomicBool::new(false),
            killer: StdMutex::new(spawned.killer),
        });
        self.operations
            .lock()
            .await
            .insert(operation_id, Some(Arc::clone(&active)));
        let (events_tx, events_rx) = mpsc::channel(EVENT_BUFFER);
        let _ = events_tx
            .send(TerminalChunk::Started {
                process_id: spawned.process_id,
            })
            .await;
        let output_bytes = Arc::new(AtomicUsize::new(0));
        let reader_active = Arc::clone(&active);
        let reader_tx = events_tx.clone();
        let max_output = self.limits.max_output_bytes;
        let read_task = tokio::task::spawn_blocking(move || {
            read_pty_output(
                spawned.reader,
                &reader_tx,
                &output_bytes,
                max_output,
                &reader_active,
            );
        });
        let wait_task = tokio::task::spawn_blocking(move || wait_for_child(spawned.child));
        let operations = Arc::clone(&self.operations);
        let activities = Arc::clone(&self.activities);
        let timeout = self.limits.max_runtime;
        tokio::spawn(async move {
            coordinate_terminal(
                operation_id,
                active,
                wait_task,
                read_task,
                timeout,
                events_tx,
                operations,
                activities,
            )
            .await;
        });
        Ok(TerminalRun {
            operation_id,
            events: events_rx,
            writer: spawned.writer,
            master: spawned.master,
            max_input_bytes: self.limits.max_input_bytes,
        })
    }

    /// Cancels and reaps an active terminal operation.
    ///
    /// # Errors
    ///
    /// Rejects unknown operations and process termination failures.
    pub async fn cancel(&self, operation_id: Uuid) -> Result<(), ToolError> {
        let active = self
            .operations
            .lock()
            .await
            .get(&operation_id)
            .and_then(Option::as_ref)
            .cloned()
            .ok_or_else(|| {
                ToolError::InvalidRequest("terminal operation is not active".to_owned())
            })?;
        active.cancelled.store(true, Ordering::SeqCst);
        let mut killer = active
            .killer
            .lock()
            .map_err(|_| ToolError::OperationFailed)?;
        let _ = killer.kill();
        Ok(())
    }

    fn validate_command(&self, command: &mut TerminalCommand) -> Result<PathBuf, ToolError> {
        if !command.program.is_absolute() {
            return Err(ToolError::InvalidRequest(
                "terminal program must be an existing absolute executable path".to_owned(),
            ));
        }
        command.program = std::fs::canonicalize(&command.program)
            .map_err(|_| ToolError::InvalidRequest("terminal program was not found".to_owned()))?;
        if !command.program.is_file() {
            return Err(ToolError::InvalidRequest(
                "terminal program must be a file".to_owned(),
            ));
        }
        if command.rows == 0 || command.columns == 0 {
            return Err(ToolError::InvalidRequest(
                "terminal dimensions must be nonzero".to_owned(),
            ));
        }
        if command
            .environment
            .keys()
            .any(|key| !self.limits.allowed_environment.contains(key))
        {
            return Err(ToolError::InvalidRequest(
                "terminal environment contains a disallowed key".to_owned(),
            ));
        }
        let relative = validate_relative_directory(&command.working_directory)?;
        let unresolved_cwd = self.workspace_root.join(&relative);
        reject_symlink_components(&self.workspace_root, &unresolved_cwd)?;
        let cwd =
            std::fs::canonicalize(unresolved_cwd).map_err(|_| ToolError::PathOutsideWorkspace)?;
        if !cwd.starts_with(&self.workspace_root) || !cwd.is_dir() {
            return Err(ToolError::PathOutsideWorkspace);
        }
        Ok(cwd)
    }

    async fn emit_start_failure(&self, operation_id: Uuid) {
        self.activities
            .emit(ToolActivity::new(
                operation_id,
                ActivityKind::Terminal,
                ActivityStatus::Failed,
                "Terminal command failed to start",
                None,
            ))
            .await;
    }
}

struct SpawnedPty {
    child: Box<dyn Child + Send + Sync>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    reader: Box<dyn Read + Send>,
    writer: SharedWriter,
    master: SharedMaster,
    process_id: Option<u32>,
}

fn spawn_pty(command: &TerminalCommand, cwd: &Path) -> Result<SpawnedPty, ToolError> {
    let pair = NativePtySystem::default()
        .openpty(PtySize {
            rows: command.rows,
            cols: command.columns,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|_| ToolError::Unavailable)?;
    let mut builder = CommandBuilder::new(&command.program);
    builder.args(&command.arguments);
    builder.cwd(cwd);
    builder.env_clear();
    for (key, value) in &command.environment {
        builder.env(key, value);
    }
    let child = pair
        .slave
        .spawn_command(builder)
        .map_err(|_| ToolError::Unavailable)?;
    let process_id = child.process_id();
    let killer = child.clone_killer();
    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|_| ToolError::Unavailable)?;
    let writer = Arc::new(StdMutex::new(Some(
        pair.master
            .take_writer()
            .map_err(|_| ToolError::Unavailable)?,
    )));
    let master = Arc::new(StdMutex::new(pair.master));
    Ok(SpawnedPty {
        child,
        killer,
        reader,
        writer,
        master,
        process_id,
    })
}

fn read_pty_output(
    mut reader: Box<dyn Read + Send>,
    events: &mpsc::Sender<TerminalChunk>,
    total: &AtomicUsize,
    max_output: usize,
    active: &ActiveTerminal,
) {
    let mut buffer = vec![0_u8; 8 * 1024];
    while let Ok(read) = reader.read(&mut buffer) {
        if read == 0 {
            break;
        }
        let previous = total.fetch_add(read, Ordering::SeqCst);
        if previous.saturating_add(read) > max_output {
            active.output_limited.store(true, Ordering::SeqCst);
            if let Ok(mut killer) = active.killer.lock() {
                let _ = killer.kill();
            }
            break;
        }
        if events
            .try_send(TerminalChunk::Output {
                bytes: buffer[..read].to_vec(),
            })
            .is_err()
        {
            active.output_limited.store(true, Ordering::SeqCst);
            if let Ok(mut killer) = active.killer.lock() {
                let _ = killer.kill();
            }
            break;
        }
    }
}

fn wait_for_child(mut child: Box<dyn Child + Send + Sync>) -> Result<ExitStatus, ToolError> {
    child.wait().map_err(|_| ToolError::OperationFailed)
}

#[allow(clippy::too_many_arguments)]
async fn coordinate_terminal(
    operation_id: Uuid,
    active: Arc<ActiveTerminal>,
    mut wait_task: tokio::task::JoinHandle<Result<ExitStatus, ToolError>>,
    read_task: tokio::task::JoinHandle<()>,
    timeout: Duration,
    events: mpsc::Sender<TerminalChunk>,
    operations: Arc<Mutex<HashMap<Uuid, Option<Arc<ActiveTerminal>>>>>,
    activities: Arc<dyn ActivitySink>,
) {
    let mut timed_out = false;
    let status = tokio::select! {
        status = &mut wait_task => status,
        () = tokio::time::sleep(timeout) => {
            timed_out = true;
            if let Ok(mut killer) = active.killer.lock() {
                let _ = killer.kill();
            }
            wait_task.await
        }
    };
    let _ = read_task.await;
    let (chunk, activity_status, title) = if timed_out {
        (
            TerminalChunk::TimedOut,
            ActivityStatus::Failed,
            "Terminal command timed out",
        )
    } else if active.cancelled.load(Ordering::SeqCst) {
        (
            TerminalChunk::Cancelled,
            ActivityStatus::Cancelled,
            "Terminal command cancelled",
        )
    } else if active.output_limited.load(Ordering::SeqCst) {
        (
            TerminalChunk::Failed {
                reason: "output limit exceeded".to_owned(),
            },
            ActivityStatus::Failed,
            "Terminal output limit exceeded",
        )
    } else {
        match status {
            Ok(Ok(status)) => (
                TerminalChunk::Exited {
                    exit_code: status.exit_code(),
                    success: status.success(),
                },
                if status.success() {
                    ActivityStatus::Completed
                } else {
                    ActivityStatus::Failed
                },
                "Terminal command finished",
            ),
            _ => (
                TerminalChunk::Failed {
                    reason: "process supervision failed".to_owned(),
                },
                ActivityStatus::Failed,
                "Terminal command failed",
            ),
        }
    };
    let _ = events.send(chunk).await;
    activities
        .emit(ToolActivity::new(
            operation_id,
            ActivityKind::Terminal,
            activity_status,
            title,
            None,
        ))
        .await;
    operations.lock().await.remove(&operation_id);
}

fn command_request(
    context: &OperationContext,
    command: &TerminalCommand,
    cwd: &Path,
) -> Result<CapabilityRequest, ToolError> {
    let encoded = serde_json::to_vec(command).map_err(|_| ToolError::OperationFailed)?;
    let digest = Sha256::digest(encoded);
    Ok(CapabilityRequest {
        context: context.clone(),
        capability: CapabilityClass::ProcessExecute,
        action: "terminal.execute".to_owned(),
        canonical_resource: format!("sha256:{digest:x}"),
        summary: format!(
            "Run {} in {}",
            command
                .program
                .file_name()
                .map_or_else(|| "program".into(), |name| name.to_string_lossy()),
            cwd.display()
        ),
        destructive: true,
    })
}

fn validate_relative_directory(path: &Path) -> Result<PathBuf, ToolError> {
    if path.is_absolute() {
        return Err(ToolError::PathOutsideWorkspace);
    }
    let mut clean = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => clean.push(value),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(ToolError::PathOutsideWorkspace);
            }
        }
    }
    Ok(clean)
}

fn reject_symlink_components(root: &Path, target: &Path) -> Result<(), ToolError> {
    let relative = target
        .strip_prefix(root)
        .map_err(|_| ToolError::PathOutsideWorkspace)?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        if std::fs::symlink_metadata(&current)
            .map_err(|_| ToolError::OperationFailed)?
            .file_type()
            .is_symlink()
        {
            return Err(ToolError::SymlinkRejected);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PolicyEffect, PolicyRule, RecordingActivitySink};

    fn context() -> OperationContext {
        OperationContext {
            operation_id: Uuid::now_v7(),
            owner_id: Uuid::now_v7(),
            device_id: Uuid::now_v7(),
            bot_id: Uuid::now_v7(),
            chat_id: Uuid::now_v7(),
            workspace_id: Uuid::now_v7(),
        }
    }

    async fn service(root: &Path, limits: TerminalLimits) -> Result<TerminalService, ToolError> {
        let activities = Arc::new(RecordingActivitySink::default());
        let policy = Arc::new(PolicyEngine::new(
            Duration::from_secs(60),
            activities.clone(),
        ));
        policy
            .replace_rules(vec![PolicyRule::new(
                CapabilityClass::ProcessExecute,
                PolicyEffect::Allow,
            )])
            .await;
        TerminalService::new(root, policy, activities, limits)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn pty_streams_output_and_reports_exit() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let service = service(directory.path(), TerminalLimits::default()).await?;
        let mut run = service
            .start(
                context(),
                TerminalCommand {
                    program: PathBuf::from("/bin/sh"),
                    arguments: vec!["-c".to_owned(), "printf homebot".to_owned()],
                    working_directory: PathBuf::new(),
                    environment: BTreeMap::from([("TERM".to_owned(), "xterm".to_owned())]),
                    rows: 24,
                    columns: 80,
                },
                None,
            )
            .await?;
        assert!(matches!(
            run.events.recv().await,
            Some(TerminalChunk::Started { .. })
        ));
        let mut output = Vec::new();
        loop {
            match run.events.recv().await {
                Some(TerminalChunk::Output { bytes }) => output.extend(bytes),
                Some(TerminalChunk::Exited { success: true, .. }) => break,
                event => return Err(format!("unexpected terminal event: {event:?}").into()),
            }
        }
        assert!(String::from_utf8_lossy(&output).contains("homebot"));
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_duplicates_and_environment_are_enforced()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let limits = TerminalLimits {
            max_runtime: Duration::from_millis(100),
            max_output_bytes: 64,
            ..TerminalLimits::default()
        };
        let service = service(directory.path(), limits).await?;
        let invalid = service
            .start(
                context(),
                TerminalCommand {
                    program: PathBuf::from("/bin/sh"),
                    arguments: Vec::new(),
                    working_directory: PathBuf::new(),
                    environment: BTreeMap::from([(
                        "OPENAI_API_KEY".to_owned(),
                        "canary".to_owned(),
                    )]),
                    rows: 24,
                    columns: 80,
                },
                None,
            )
            .await;
        assert!(matches!(invalid, Err(ToolError::InvalidRequest(_))));

        let operation = context();
        let operation_id = operation.operation_id;
        let mut run = service
            .start(
                operation.clone(),
                TerminalCommand {
                    program: PathBuf::from("/bin/sh"),
                    arguments: vec!["-c".to_owned(), "sleep 30".to_owned()],
                    working_directory: PathBuf::new(),
                    environment: BTreeMap::new(),
                    rows: 24,
                    columns: 80,
                },
                None,
            )
            .await?;
        let _ = run.events.recv().await;
        assert!(matches!(
            service
                .start(
                    operation,
                    TerminalCommand {
                        program: PathBuf::from("/bin/sh"),
                        arguments: vec!["-c".to_owned(), "printf duplicate".to_owned()],
                        working_directory: PathBuf::new(),
                        environment: BTreeMap::new(),
                        rows: 24,
                        columns: 80,
                    },
                    None,
                )
                .await,
            Err(ToolError::InvalidRequest(_))
        ));
        service.cancel(operation_id).await?;
        assert!(matches!(
            run.events.recv().await,
            Some(TerminalChunk::Cancelled)
        ));
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_and_output_limits_terminate_the_process()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let limits = TerminalLimits {
            max_runtime: Duration::from_millis(100),
            max_output_bytes: 64,
            ..TerminalLimits::default()
        };
        let service = service(directory.path(), limits).await?;
        let mut timed = service
            .start(
                context(),
                TerminalCommand {
                    program: PathBuf::from("/bin/sh"),
                    arguments: vec!["-c".to_owned(), "sleep 30".to_owned()],
                    working_directory: PathBuf::new(),
                    environment: BTreeMap::new(),
                    rows: 24,
                    columns: 80,
                },
                None,
            )
            .await?;
        let _ = timed.events.recv().await;
        assert!(matches!(
            timed.events.recv().await,
            Some(TerminalChunk::TimedOut)
        ));

        let mut noisy = service
            .start(
                context(),
                TerminalCommand {
                    program: PathBuf::from("/bin/sh"),
                    arguments: vec!["-c".to_owned(), "printf '%0200d' 0".to_owned()],
                    working_directory: PathBuf::new(),
                    environment: BTreeMap::new(),
                    rows: 24,
                    columns: 80,
                },
                None,
            )
            .await?;
        let _ = noisy.events.recv().await;
        assert!(matches!(
            noisy.events.recv().await,
            Some(TerminalChunk::Failed { reason }) if reason == "output limit exceeded"
        ));
        Ok(())
    }
}
