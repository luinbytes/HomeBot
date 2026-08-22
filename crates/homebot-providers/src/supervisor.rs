//! Bounded child-process supervision for structured provider adapters.

use std::{
    collections::{BTreeMap, VecDeque},
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    task::JoinHandle,
    time::Instant,
};
use uuid::Uuid;

const SPAWN_ATTEMPTS: usize = 8;
const SPAWN_RETRY_DELAY: Duration = Duration::from_millis(2);

pub(crate) enum BoundedLine {
    Eof,
    Line(String),
    TooLong,
}

/// Reads one UTF-8 line without allowing the source to grow the destination
/// beyond `max_bytes`. A too-long frame is rejected before its remainder is
/// allocated; callers terminate the corresponding provider process.
pub(crate) async fn read_bounded_line<R>(
    reader: &mut R,
    max_bytes: usize,
) -> std::io::Result<BoundedLine>
where
    R: AsyncBufRead + Unpin,
{
    let mut bytes = Vec::with_capacity(max_bytes.min(8 * 1024));
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            if bytes.is_empty() {
                return Ok(BoundedLine::Eof);
            }
            return String::from_utf8(bytes)
                .map(BoundedLine::Line)
                .map_err(|_| std::io::Error::other("provider frame is not UTF-8"));
        }
        let take = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |position| position + 1);
        if bytes.len().saturating_add(take) > max_bytes {
            return Ok(BoundedLine::TooLong);
        }
        bytes.extend_from_slice(&available[..take]);
        reader.consume(take);
        if bytes.last() == Some(&b'\n') {
            return String::from_utf8(bytes)
                .map(BoundedLine::Line)
                .map_err(|_| std::io::Error::other("provider frame is not UTF-8"));
        }
    }
}

#[derive(Clone, Debug)]
pub struct ProcessLimits {
    pub max_stderr_bytes: usize,
    pub shutdown_grace: Duration,
}

impl Default for ProcessLimits {
    fn default() -> Self {
        Self {
            max_stderr_bytes: 64 * 1024,
            shutdown_grace: Duration::from_secs(3),
        }
    }
}

pub struct ProcessSpec {
    program: PathBuf,
    args: Vec<OsString>,
    current_dir: Option<PathBuf>,
    environment: BTreeMap<OsString, OsString>,
    secret_values: Vec<String>,
    limits: ProcessLimits,
}

impl std::fmt::Debug for ProcessSpec {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProcessSpec")
            .field("program", &self.program.file_name())
            .field("arg_count", &self.args.len())
            .field("current_dir", &self.current_dir)
            .field(
                "environment_keys",
                &self.environment.keys().collect::<Vec<_>>(),
            )
            .field("secret_values", &"redacted")
            .field("limits", &self.limits)
            .finish()
    }
}

impl ProcessSpec {
    #[must_use]
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            current_dir: None,
            environment: BTreeMap::new(),
            secret_values: Vec::new(),
            limits: ProcessLimits::default(),
        }
    }

    #[must_use]
    pub fn arg(mut self, argument: impl AsRef<OsStr>) -> Self {
        self.args.push(argument.as_ref().to_owned());
        self
    }

    #[must_use]
    pub fn current_dir(mut self, directory: impl Into<PathBuf>) -> Self {
        self.current_dir = Some(directory.into());
        self
    }

    #[must_use]
    pub fn environment(mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> Self {
        self.environment
            .insert(key.as_ref().to_owned(), value.as_ref().to_owned());
        self
    }

    #[must_use]
    pub fn redact_value(mut self, value: impl Into<String>) -> Self {
        let value = value.into();
        if !value.is_empty() {
            self.secret_values.push(value);
        }
        self
    }

    #[must_use]
    pub fn limits(mut self, limits: ProcessLimits) -> Self {
        self.limits = limits;
        self
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessTermination {
    Exited,
    Crashed,
    CleanShutdown,
    KilledAfterGrace,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessReport {
    pub diagnostic_id: Uuid,
    pub termination: ProcessTermination,
    pub exit_code: Option<i32>,
    pub runtime_ms: u64,
    pub stderr_tail: Vec<String>,
    pub stderr_truncated: bool,
}

impl ProcessReport {
    #[must_use]
    pub fn succeeded(&self) -> bool {
        matches!(
            self.termination,
            ProcessTermination::Exited | ProcessTermination::CleanShutdown
        ) && self.exit_code == Some(0)
    }
}

#[derive(Debug, Default)]
struct BoundedDiagnostics {
    lines: VecDeque<String>,
    bytes: usize,
    truncated: bool,
}

pub struct SupervisedProcess {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    stdout: Option<ChildStdout>,
    stderr: Option<JoinHandle<BoundedDiagnostics>>,
    limits: ProcessLimits,
    started: Instant,
}

impl std::fmt::Debug for SupervisedProcess {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SupervisedProcess")
            .field("pid", &self.child.as_ref().and_then(Child::id))
            .field("stdin", &self.stdin.is_some())
            .field("stdout", &self.stdout.is_some())
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl SupervisedProcess {
    /// Spawns a child with a cleared environment and bounded redacted diagnostics.
    ///
    /// # Errors
    ///
    /// Returns a safe spawn error without including arguments or environment values.
    pub fn spawn(spec: ProcessSpec) -> Result<Self, ProviderProcessError> {
        let ProcessSpec {
            program,
            args,
            current_dir,
            environment,
            secret_values,
            limits,
        } = spec;
        let mut command = Command::new(&program);
        command
            .args(args)
            .env_clear()
            .envs(environment)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(directory) = current_dir {
            command.current_dir(directory);
        }
        let mut child = spawn_child(&mut command, &program)?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child
            .stderr
            .take()
            .ok_or(ProviderProcessError::MissingPipe)?;
        let stderr_limit = limits.max_stderr_bytes.max(1);
        let stderr_task = tokio::spawn(read_bounded_stderr(stderr, stderr_limit, secret_values));
        Ok(Self {
            child: Some(child),
            stdin,
            stdout,
            stderr: Some(stderr_task),
            limits,
            started: Instant::now(),
        })
    }

    #[must_use]
    pub fn stdin_mut(&mut self) -> Option<&mut ChildStdin> {
        self.stdin.as_mut()
    }

    #[must_use]
    pub fn take_stdin(&mut self) -> Option<ChildStdin> {
        self.stdin.take()
    }

    #[must_use]
    pub fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.stdout.take()
    }

    /// Waits for natural completion and classifies nonzero/signal exits as crashes.
    ///
    /// # Errors
    ///
    /// Returns an I/O or diagnostic-task error if supervision cannot finish safely.
    pub async fn wait(mut self) -> Result<ProcessReport, ProviderProcessError> {
        self.stdin.take();
        let status = self
            .child
            .as_mut()
            .ok_or(ProviderProcessError::AlreadyFinished)?
            .wait()
            .await?;
        self.child.take();
        let termination = if status.success() {
            ProcessTermination::Exited
        } else {
            ProcessTermination::Crashed
        };
        self.report(termination, status.code()).await
    }

    /// Requests clean shutdown by closing stdin, then kills after the grace deadline.
    ///
    /// # Errors
    ///
    /// Returns an I/O or diagnostic-task error if the process cannot be reaped.
    pub async fn shutdown(mut self) -> Result<ProcessReport, ProviderProcessError> {
        self.stdin.take();
        let child = self
            .child
            .as_mut()
            .ok_or(ProviderProcessError::AlreadyFinished)?;
        if let Ok(status) = tokio::time::timeout(self.limits.shutdown_grace, child.wait()).await {
            let status = status?;
            self.child.take();
            self.report(ProcessTermination::CleanShutdown, status.code())
                .await
        } else {
            child.start_kill()?;
            let status = child.wait().await?;
            self.child.take();
            self.report(ProcessTermination::KilledAfterGrace, status.code())
                .await
        }
    }

    async fn report(
        &mut self,
        termination: ProcessTermination,
        exit_code: Option<i32>,
    ) -> Result<ProcessReport, ProviderProcessError> {
        let diagnostics = match self.stderr.take() {
            Some(task) => task.await.map_err(ProviderProcessError::DiagnosticTask)?,
            None => BoundedDiagnostics::default(),
        };
        Ok(ProcessReport {
            diagnostic_id: Uuid::now_v7(),
            termination,
            exit_code,
            runtime_ms: u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX),
            stderr_tail: diagnostics.lines.into_iter().collect(),
            stderr_truncated: diagnostics.truncated,
        })
    }
}

fn spawn_child(command: &mut Command, program: &Path) -> Result<Child, ProviderProcessError> {
    for attempt in 1..=SPAWN_ATTEMPTS {
        match command.spawn() {
            Ok(child) => return Ok(child),
            Err(source)
                if source.kind() == std::io::ErrorKind::ExecutableFileBusy
                    && attempt < SPAWN_ATTEMPTS =>
            {
                std::thread::sleep(SPAWN_RETRY_DELAY);
            }
            Err(source) => {
                return Err(ProviderProcessError::Spawn {
                    program: program
                        .file_name()
                        .and_then(OsStr::to_str)
                        .unwrap_or("provider")
                        .to_owned(),
                    source,
                });
            }
        }
    }
    unreachable!("spawn loop always returns on its final attempt")
}

impl Drop for SupervisedProcess {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.start_kill();
        }
    }
}

async fn read_bounded_stderr(
    stderr: tokio::process::ChildStderr,
    limit: usize,
    secret_values: Vec<String>,
) -> BoundedDiagnostics {
    let mut reader = BufReader::new(stderr);
    let mut output = BoundedDiagnostics::default();
    let mut chunk = [0_u8; 4 * 1024];
    let mut line = Vec::with_capacity(4 * 1024);
    let mut discarding_line = false;
    loop {
        let Ok(read) = reader.read(&mut chunk).await else {
            output.truncated = true;
            break;
        };
        if read == 0 {
            if !line.is_empty() {
                retain_diagnostic(&mut output, &line, limit, &secret_values);
            }
            break;
        }
        for byte in &chunk[..read] {
            if *byte == b'\n' {
                if !discarding_line {
                    retain_diagnostic(&mut output, &line, limit, &secret_values);
                }
                line.clear();
                discarding_line = false;
            } else if !discarding_line {
                if line.len() < 4_096 {
                    line.push(*byte);
                } else {
                    line.clear();
                    discarding_line = true;
                    output.truncated = true;
                }
            }
        }
    }
    output
}

fn retain_diagnostic(
    output: &mut BoundedDiagnostics,
    bytes: &[u8],
    limit: usize,
    secret_values: &[String],
) {
    let mut line = String::from_utf8_lossy(bytes).trim_end().to_owned();
    for secret in secret_values {
        line = line.replace(secret, "[REDACTED]");
    }
    let line_bytes = line.len();
    while output.bytes.saturating_add(line_bytes) > limit && !output.lines.is_empty() {
        if let Some(removed) = output.lines.pop_front() {
            output.bytes = output.bytes.saturating_sub(removed.len());
            output.truncated = true;
        }
    }
    if line_bytes > limit {
        output.truncated = true;
        return;
    }
    output.bytes = output.bytes.saturating_add(line_bytes);
    output.lines.push_back(line);
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderProcessError {
    #[error("could not start provider executable {program}: {source}")]
    Spawn {
        program: String,
        #[source]
        source: std::io::Error,
    },
    #[error("provider process pipe setup failed")]
    MissingPipe,
    #[error("provider process already finished")]
    AlreadyFinished,
    #[error("provider process I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("provider diagnostic task failed: {0}")]
    DiagnosticTask(tokio::task::JoinError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncWriteExt, BufReader, duplex};

    #[tokio::test]
    async fn bounded_line_rejects_an_unterminated_frame_at_the_limit() {
        let (mut writer, reader) = duplex(64);
        let write = tokio::spawn(async move {
            writer
                .write_all(b"123456789")
                .await
                .unwrap_or_else(|error| panic!("fixture write failed: {error}"));
        });
        let frame = read_bounded_line(&mut BufReader::new(reader), 8)
            .await
            .unwrap_or_else(|error| panic!("bounded read failed: {error}"));
        assert!(matches!(frame, BoundedLine::TooLong));
        write
            .await
            .unwrap_or_else(|error| panic!("fixture task failed: {error}"));
    }

    #[tokio::test]
    async fn bounded_line_accepts_newline_and_partial_eof_frames() {
        let mut first = BufReader::new(&b"ready\nnext"[..]);
        assert!(matches!(
            read_bounded_line(&mut first, 16).await,
            Ok(BoundedLine::Line(line)) if line == "ready\n"
        ));
        assert!(matches!(
            read_bounded_line(&mut first, 16).await,
            Ok(BoundedLine::Line(line)) if line == "next"
        ));
        assert!(matches!(
            read_bounded_line(&mut first, 16).await,
            Ok(BoundedLine::Eof)
        ));
    }
}
