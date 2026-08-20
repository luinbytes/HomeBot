//! Bounded, shell-free Git repository and isolated-worktree lifecycle.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{process::Command, time::timeout};
use uuid::Uuid;

mod checkpoints;
mod source_control;

pub use checkpoints::{
    CheckpointCapture, CheckpointDiff, CheckpointPhase, ConversationReconciliation, FileChange,
    FileChangeStatus, RestoreResult,
};
pub use source_control::{
    PullRequestMetadata, PullRequestProvider, PullRequestSummary, VcsChangeKind, VcsCommitResult,
    VcsPushResult, VcsRemoteSummary, VcsStatus, VcsStatusEntry,
};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_OUTPUT_BYTES: usize = 256 * 1_024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkingTreeCondition {
    Clean,
    Dirty,
    Conflicted,
    /// The durable association exists, but Git can no longer inspect its path.
    Unavailable,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceMode {
    Primary,
    Isolated,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryInspection {
    pub root: PathBuf,
    pub display_name: String,
    pub branch: Option<String>,
    pub condition: WorkingTreeCondition,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorktreeInspection {
    pub path: PathBuf,
    pub branch: String,
    pub condition: WorkingTreeCondition,
}

#[derive(Clone, Debug)]
pub struct GitRuntime {
    executable: PathBuf,
    github_cli: Option<PathBuf>,
    command_timeout: Duration,
}

#[derive(Debug, thiserror::Error)]
pub enum VcsError {
    #[error("Git is not installed in a supported location")]
    GitUnavailable,
    #[error("Repository path is invalid")]
    InvalidPath,
    #[error("Path is not a Git repository")]
    NotRepository,
    #[error("Git operation timed out")]
    Timeout,
    #[error("Git output exceeded the safety limit")]
    OutputLimit,
    #[error("Git operation failed: {0}")]
    Git(String),
    #[error("Worktree contains changes and was preserved")]
    DirtyWorktree,
    #[error("Restore would overwrite an ignored workspace path and was refused")]
    RestoreConflict,
    #[error("No push remote is configured")]
    NoRemote,
    #[error("Remote authentication failed")]
    AuthenticationFailed,
    #[error("Pull-request integration is unavailable")]
    PullRequestUnavailable,
    #[error("Managed worktree path is outside its root")]
    UnsafeWorktreePath,
    #[error("filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),
}

impl GitRuntime {
    /// Discovers Git only from an explicit absolute override or fixed platform locations.
    ///
    /// # Errors
    /// Returns `GitUnavailable` when no executable is present.
    pub fn discover() -> Result<Self, VcsError> {
        let explicit = std::env::var_os("HOMEBOT_GIT_BIN").map(PathBuf::from);
        let mut candidates = explicit.into_iter().chain([
            PathBuf::from("/usr/bin/git"),
            PathBuf::from("/opt/homebrew/bin/git"),
            PathBuf::from("/usr/local/bin/git"),
        ]);
        let executable = candidates
            .find(|path| path.is_absolute() && path.is_file())
            .ok_or(VcsError::GitUnavailable)?;
        Ok(Self {
            executable,
            github_cli: discover_github_cli(),
            command_timeout: COMMAND_TIMEOUT,
        })
    }

    #[must_use]
    pub fn with_executable(executable: PathBuf) -> Self {
        Self {
            executable,
            github_cli: discover_github_cli(),
            command_timeout: COMMAND_TIMEOUT,
        }
    }

    #[must_use]
    pub fn with_github_cli(mut self, executable: Option<PathBuf>) -> Self {
        self.github_cli = executable;
        self
    }

    /// Canonicalizes and inspects a repository without changing it.
    ///
    /// # Errors
    /// Returns a safe path, process, timeout, or repository error.
    pub async fn inspect_repository(
        &self,
        candidate: &Path,
    ) -> Result<RepositoryInspection, VcsError> {
        let candidate = tokio::fs::canonicalize(candidate)
            .await
            .map_err(|_| VcsError::InvalidPath)?;
        if !candidate.is_dir() {
            return Err(VcsError::InvalidPath);
        }
        let root = self
            .git(Some(&candidate), &["rev-parse", "--show-toplevel"])
            .await
            .map_err(|error| match error {
                VcsError::Git(_) => VcsError::NotRepository,
                other => other,
            })?;
        let root = tokio::fs::canonicalize(root.trim())
            .await
            .map_err(|_| VcsError::NotRepository)?;
        let branch = self
            .git(Some(&root), &["symbolic-ref", "--quiet", "--short", "HEAD"])
            .await
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        let condition = self.condition(&root).await?;
        let display_name = root
            .file_name()
            .and_then(|value| value.to_str())
            .filter(|value| !value.is_empty())
            .ok_or(VcsError::InvalidPath)?
            .to_owned();
        Ok(RepositoryInspection {
            root,
            display_name,
            branch,
            condition,
        })
    }

    /// Lists local branch names in deterministic order.
    ///
    /// # Errors
    /// Returns a bounded Git execution error.
    pub async fn branches(&self, repository: &Path) -> Result<Vec<String>, VcsError> {
        let output = self
            .git(
                Some(repository),
                &["for-each-ref", "--format=%(refname:short)", "refs/heads"],
            )
            .await?;
        let mut branches = output
            .lines()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        branches.sort();
        branches.dedup();
        Ok(branches)
    }

    /// Creates a deterministic isolated worktree under the server-owned root.
    ///
    /// Existing paths are never overwritten. Refs are literal process arguments, never shell
    /// fragments.
    ///
    /// # Errors
    /// Returns when paths/refs are unsafe, already exist, or Git cannot create the worktree.
    pub async fn create_worktree(
        &self,
        repository: &Path,
        managed_root: &Path,
        chat_id: Uuid,
        base_ref: &str,
        branch: Option<&str>,
    ) -> Result<WorktreeInspection, VcsError> {
        validate_ref(base_ref)?;
        let branch = branch.map_or_else(
            || format!("homebot/{}", chat_id.simple()),
            ToOwned::to_owned,
        );
        validate_ref(&branch)?;
        tokio::fs::create_dir_all(managed_root).await?;
        let managed_root = tokio::fs::canonicalize(managed_root).await?;
        let path = managed_root.join(chat_id.simple().to_string());
        if tokio::fs::try_exists(&path).await? {
            return Err(VcsError::Git("managed worktree already exists".to_owned()));
        }
        let path_arg = path.to_str().ok_or(VcsError::InvalidPath)?;
        if self.branches(repository).await?.contains(&branch) {
            self.git(
                Some(repository),
                &["worktree", "add", "--no-track", path_arg, &branch],
            )
            .await?;
        } else {
            self.git(
                Some(repository),
                &[
                    "worktree",
                    "add",
                    "--no-track",
                    "-b",
                    &branch,
                    path_arg,
                    base_ref,
                ],
            )
            .await?;
        }
        let canonical = tokio::fs::canonicalize(&path).await?;
        ensure_managed(&managed_root, &canonical)?;
        Ok(WorktreeInspection {
            condition: self.condition(&canonical).await?,
            path: canonical,
            branch,
        })
    }

    /// Removes a clean managed worktree. Dirty/conflicted worktrees are always preserved.
    ///
    /// # Errors
    /// Returns without removal when the path is outside the managed root or contains changes.
    pub async fn remove_worktree(
        &self,
        repository: &Path,
        managed_root: &Path,
        worktree: &Path,
    ) -> Result<(), VcsError> {
        let managed_root = tokio::fs::canonicalize(managed_root).await?;
        let worktree = tokio::fs::canonicalize(worktree).await?;
        ensure_managed(&managed_root, &worktree)?;
        if self.condition(&worktree).await? != WorkingTreeCondition::Clean {
            return Err(VcsError::DirtyWorktree);
        }
        self.git(
            Some(repository),
            &[
                "worktree",
                "remove",
                worktree.to_str().ok_or(VcsError::InvalidPath)?,
            ],
        )
        .await?;
        Ok(())
    }

    async fn condition(&self, repository: &Path) -> Result<WorkingTreeCondition, VcsError> {
        let output = self
            .git(Some(repository), &["status", "--porcelain=v1", "-z"])
            .await?;
        if output.is_empty() {
            return Ok(WorkingTreeCondition::Clean);
        }
        let conflicted = output.split('\0').any(|line| {
            let status = line.as_bytes().get(..2).unwrap_or_default();
            matches!(
                status,
                b"DD" | b"AU" | b"UD" | b"UA" | b"DU" | b"AA" | b"UU"
            )
        });
        Ok(if conflicted {
            WorkingTreeCondition::Conflicted
        } else {
            WorkingTreeCondition::Dirty
        })
    }

    async fn git(&self, repository: Option<&Path>, arguments: &[&str]) -> Result<String, VcsError> {
        let output = self
            .git_bytes(repository, arguments, &[], MAX_OUTPUT_BYTES)
            .await?;
        String::from_utf8(output)
            .map_err(|_| VcsError::Git("Git returned invalid UTF-8".to_owned()))
    }

    async fn git_bytes(
        &self,
        repository: Option<&Path>,
        arguments: &[&str],
        environment: &[(&str, &str)],
        output_limit: usize,
    ) -> Result<Vec<u8>, VcsError> {
        let mut command = Command::new(&self.executable);
        command
            .env_clear()
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("LC_ALL", "C")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for (key, value) in environment {
            command.env(key, value);
        }
        if let Some(repository) = repository {
            command.arg("-C").arg(repository);
        }
        command.args(arguments);
        let output = timeout(self.command_timeout, command.output())
            .await
            .map_err(|_| VcsError::Timeout)??;
        if output.stdout.len() > output_limit || output.stderr.len() > output_limit {
            return Err(VcsError::OutputLimit);
        }
        if !output.status.success() {
            let message = String::from_utf8_lossy(&output.stderr);
            return Err(VcsError::Git(redact_git_error(message.trim())));
        }
        Ok(output.stdout)
    }

    async fn git_remote(&self, repository: &Path, arguments: &[&str]) -> Result<String, VcsError> {
        let mut environment = Vec::new();
        for key in ["HOME", "XDG_CONFIG_HOME", "SSH_AUTH_SOCK"] {
            if let Ok(value) = std::env::var(key) {
                environment.push((key, value));
            }
        }
        let borrowed = environment
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect::<Vec<_>>();
        let output = match self
            .git_bytes(Some(repository), arguments, &borrowed, MAX_OUTPUT_BYTES)
            .await
        {
            Err(VcsError::Git(message)) if is_authentication_error(&message) => {
                return Err(VcsError::AuthenticationFailed);
            }
            result => result?,
        };
        String::from_utf8(output)
            .map_err(|_| VcsError::Git("Git returned invalid UTF-8".to_owned()))
    }
}

fn discover_github_cli() -> Option<PathBuf> {
    [
        PathBuf::from("/usr/bin/gh"),
        PathBuf::from("/opt/homebrew/bin/gh"),
        PathBuf::from("/usr/local/bin/gh"),
    ]
    .into_iter()
    .find(|path| path.is_file())
}

fn is_authentication_error(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    [
        "authentication failed",
        "permission denied",
        "could not read username",
        "repository not found",
        "http 401",
        "http 403",
    ]
    .iter()
    .any(|candidate| message.contains(candidate))
}

pub(crate) fn validate_ref(value: &str) -> Result<(), VcsError> {
    if value.is_empty()
        || value.len() > 240
        || value.starts_with('-')
        || value.chars().any(char::is_control)
        || value.contains("..")
        || value.contains("@{")
        || value.ends_with(['.', '/'])
        || value.contains([' ', '~', '^', ':', '?', '*', '[', '\\'])
    {
        return Err(VcsError::Git("invalid Git ref".to_owned()));
    }
    Ok(())
}

fn ensure_managed(root: &Path, worktree: &Path) -> Result<(), VcsError> {
    if worktree == root || !worktree.starts_with(root) {
        return Err(VcsError::UnsafeWorktreePath);
    }
    Ok(())
}

fn redact_git_error(message: &str) -> String {
    let first = message.lines().next().unwrap_or("Git operation failed");
    let mut value = first.chars().take(500).collect::<String>();
    for prefix in ["https://", "http://"] {
        while let Some(start) = value.find(prefix) {
            let suffix = &value[start..];
            let end = suffix.find(char::is_whitespace).unwrap_or(suffix.len());
            value.replace_range(start..start + end, "[REMOTE]");
        }
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::fs;

    async fn git(runtime: &GitRuntime, directory: &Path, args: &[&str]) -> Result<(), VcsError> {
        runtime.git(Some(directory), args).await.map(|_| ())
    }

    async fn repository() -> Result<(tempfile::TempDir, GitRuntime), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let runtime = GitRuntime::discover()?;
        git(&runtime, directory.path(), &["init", "-b", "main"]).await?;
        git(
            &runtime,
            directory.path(),
            &["config", "user.name", "HomeBot Fixture"],
        )
        .await?;
        git(
            &runtime,
            directory.path(),
            &["config", "user.email", "fixture@homebot.invalid"],
        )
        .await?;
        fs::write(directory.path().join("README.md"), "fixture\n").await?;
        git(&runtime, directory.path(), &["add", "README.md"]).await?;
        git(&runtime, directory.path(), &["commit", "-m", "fixture"]).await?;
        Ok((directory, runtime))
    }

    #[tokio::test]
    async fn dirty_primary_is_preserved_while_isolated_worktree_is_created()
    -> Result<(), Box<dyn std::error::Error>> {
        let (repository, runtime) = repository().await?;
        fs::write(repository.path().join("README.md"), "valuable user edit\n").await?;
        fs::write(repository.path().join("untracked.txt"), "do not delete\n").await?;
        let inspected = runtime.inspect_repository(repository.path()).await?;
        assert_eq!(inspected.condition, WorkingTreeCondition::Dirty);
        let managed = tempfile::tempdir()?;
        let chat_id = Uuid::now_v7();
        let worktree = runtime
            .create_worktree(repository.path(), managed.path(), chat_id, "main", None)
            .await?;
        assert_eq!(worktree.condition, WorkingTreeCondition::Clean);
        assert_eq!(
            fs::read_to_string(repository.path().join("README.md")).await?,
            "valuable user edit\n"
        );
        assert_eq!(
            fs::read_to_string(repository.path().join("untracked.txt")).await?,
            "do not delete\n"
        );
        runtime
            .remove_worktree(repository.path(), managed.path(), &worktree.path)
            .await?;
        assert!(!worktree.path.exists());
        Ok(())
    }

    #[tokio::test]
    async fn dirty_or_outside_worktree_cleanup_fails_without_data_loss()
    -> Result<(), Box<dyn std::error::Error>> {
        let (repository, runtime) = repository().await?;
        let managed = tempfile::tempdir()?;
        let worktree = runtime
            .create_worktree(
                repository.path(),
                managed.path(),
                Uuid::now_v7(),
                "main",
                None,
            )
            .await?;
        fs::write(worktree.path.join("valuable.txt"), "keep me\n").await?;
        assert!(matches!(
            runtime
                .remove_worktree(repository.path(), managed.path(), &worktree.path)
                .await,
            Err(VcsError::DirtyWorktree)
        ));
        assert_eq!(
            fs::read_to_string(worktree.path.join("valuable.txt")).await?,
            "keep me\n"
        );
        assert!(matches!(
            runtime
                .remove_worktree(repository.path(), managed.path(), repository.path())
                .await,
            Err(VcsError::UnsafeWorktreePath)
        ));
        Ok(())
    }

    #[tokio::test]
    async fn option_like_refs_are_rejected_before_git_or_filesystem_mutation()
    -> Result<(), Box<dyn std::error::Error>> {
        let (repository, runtime) = repository().await?;
        let managed = tempfile::tempdir()?;
        let chat_id = Uuid::now_v7();
        assert!(matches!(
            runtime
                .create_worktree(repository.path(), managed.path(), chat_id, "--force", None)
                .await,
            Err(VcsError::Git(message)) if message == "invalid Git ref"
        ));
        assert!(!managed.path().join(chat_id.simple().to_string()).exists());
        assert_eq!(runtime.branches(repository.path()).await?, vec!["main"]);
        Ok(())
    }

    #[tokio::test]
    async fn detached_head_can_seed_a_deterministic_isolated_branch()
    -> Result<(), Box<dyn std::error::Error>> {
        let (repository, runtime) = repository().await?;
        git(&runtime, repository.path(), &["checkout", "--detach"]).await?;
        assert_eq!(
            runtime.inspect_repository(repository.path()).await?.branch,
            None
        );
        let managed = tempfile::tempdir()?;
        let chat_id = Uuid::now_v7();
        let worktree = runtime
            .create_worktree(repository.path(), managed.path(), chat_id, "HEAD", None)
            .await?;
        assert_eq!(worktree.branch, format!("homebot/{}", chat_id.simple()));
        runtime
            .remove_worktree(repository.path(), managed.path(), &worktree.path)
            .await?;
        Ok(())
    }
}
