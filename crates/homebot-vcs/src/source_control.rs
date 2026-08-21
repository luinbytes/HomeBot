use super::{GitRuntime, VcsError, validate_ref};
use crate::CheckpointDiff;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path, process::Stdio};
use tokio::time::timeout;

const MAX_DIFF_BYTES: usize = 8 * 1_024 * 1_024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VcsChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Unmerged,
    Untracked,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VcsStatusEntry {
    pub path: String,
    pub previous_path: Option<String>,
    pub staged: Option<VcsChangeKind>,
    pub unstaged: Option<VcsChangeKind>,
    pub conflicted: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VcsRemoteSummary {
    pub name: String,
    pub fetch_configured: bool,
    pub push_configured: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VcsStatus {
    pub head_oid: Option<String>,
    pub branch: Option<String>,
    pub detached: bool,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub conflicted: bool,
    pub entries: Vec<VcsStatusEntry>,
    pub remotes: Vec<VcsRemoteSummary>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VcsCommitResult {
    pub commit_oid: String,
    pub branch: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VcsPushResult {
    pub remote: String,
    pub branch: String,
    pub updated: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PullRequestProvider {
    GitHub,
    Unsupported,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PullRequestSummary {
    pub number: u64,
    pub url: String,
    pub title: String,
    pub state: String,
    pub head_branch: String,
    pub base_branch: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PullRequestMetadata {
    pub remote: String,
    pub provider: PullRequestProvider,
    pub repository: Option<String>,
    pub head_branch: String,
    pub base_branch: String,
    pub compare_url: Option<String>,
    pub create_available: bool,
    pub current: Option<PullRequestSummary>,
}

impl GitRuntime {
    /// Reads the complete staged, unstaged, conflict, branch, upstream and remote projection.
    /// Remote URLs are deliberately not returned because they may embed credentials.
    ///
    /// # Errors
    /// Returns a bounded Git or path-decoding error.
    pub async fn source_status(&self, repository: &Path) -> Result<VcsStatus, VcsError> {
        let repository = self.inspect_repository(repository).await?.root;
        let bytes = self
            .git_bytes(
                Some(&repository),
                &[
                    "status",
                    "--porcelain=v2",
                    "--branch",
                    "-z",
                    "--untracked-files=all",
                ],
                &[],
                super::MAX_OUTPUT_BYTES,
            )
            .await?;
        let mut status = parse_status(&bytes)?;
        status.remotes = self.remote_summaries(&repository).await?;
        Ok(status)
    }

    /// Returns an exact binary-capable staged or unstaged patch and changed-file summary.
    ///
    /// # Errors
    /// Returns a bounded Git or path-decoding error.
    pub async fn working_diff(
        &self,
        repository: &Path,
        staged: bool,
    ) -> Result<CheckpointDiff, VcsError> {
        let mut patch_args = vec!["diff"];
        let mut names_args = vec!["diff"];
        let mut numstat_args = vec!["diff"];
        if staged {
            patch_args.push("--cached");
            names_args.push("--cached");
            numstat_args.push("--cached");
        }
        patch_args.extend(["--binary", "--full-index", "--find-renames", "--"]);
        names_args.extend(["--name-status", "-z", "--find-renames", "--"]);
        numstat_args.extend(["--numstat", "-z", "--"]);
        let patch = self
            .git_bytes(Some(repository), &patch_args, &[], MAX_DIFF_BYTES)
            .await?;
        let names = self
            .git_bytes(Some(repository), &names_args, &[], MAX_DIFF_BYTES)
            .await?;
        let binary = super::checkpoints::binary_paths(
            &self
                .git_bytes(Some(repository), &numstat_args, &[], MAX_DIFF_BYTES)
                .await?,
        )?;
        Ok(CheckpointDiff {
            patch: String::from_utf8(patch)
                .map_err(|_| VcsError::Git("Git diff returned invalid UTF-8".to_owned()))?,
            files: super::checkpoints::parse_name_status(&names, &binary)?,
        })
    }

    /// Creates a commit from the current index, optionally staging all non-ignored changes first.
    ///
    /// # Errors
    /// Rejects invalid messages and reports Git failures without discarding workspace changes.
    pub async fn commit(
        &self,
        repository: &Path,
        message: &str,
        stage_all: bool,
    ) -> Result<VcsCommitResult, VcsError> {
        let message = message.trim();
        if message.is_empty() || message.len() > 10_000 || message.contains('\0') {
            return Err(VcsError::Git("invalid commit message".to_owned()));
        }
        if stage_all {
            self.git(Some(repository), &["add", "--all", "--", "."])
                .await?;
        }
        self.git(
            Some(repository),
            &[
                "-c",
                "core.hooksPath=/dev/null",
                "commit",
                "--no-gpg-sign",
                "-m",
                message,
            ],
        )
        .await?;
        let commit_oid = self.git(Some(repository), &["rev-parse", "HEAD"]).await?;
        let branch = self
            .git(
                Some(repository),
                &["symbolic-ref", "--quiet", "--short", "HEAD"],
            )
            .await
            .ok()
            .map(|value| value.trim().to_owned());
        Ok(VcsCommitResult {
            commit_oid: super::checkpoints::oid(commit_oid.trim())?,
            branch,
        })
    }

    /// Creates and checks out a new branch only from a clean, non-conflicted workspace.
    ///
    /// # Errors
    /// Rejects unsafe names, dirty workspaces and Git failures without data loss.
    pub async fn create_branch(
        &self,
        repository: &Path,
        branch: &str,
        start_point: Option<&str>,
    ) -> Result<String, VcsError> {
        validate_ref(branch)?;
        if self.condition(repository).await? != crate::WorkingTreeCondition::Clean {
            return Err(VcsError::DirtyWorktree);
        }
        let start = start_point.unwrap_or("HEAD");
        validate_ref(start)?;
        self.git(
            Some(repository),
            &[
                "-c",
                "core.hooksPath=/dev/null",
                "switch",
                "--create",
                branch,
                start,
            ],
        )
        .await?;
        Ok(branch.to_owned())
    }

    /// Pushes one explicit local branch to one configured remote without invoking a shell.
    ///
    /// # Errors
    /// Rejects unsafe names, detached/missing branches, missing remotes and authentication errors.
    pub async fn push(
        &self,
        repository: &Path,
        remote: &str,
        branch: &str,
        set_upstream: bool,
    ) -> Result<VcsPushResult, VcsError> {
        validate_remote(remote)?;
        validate_ref(branch)?;
        let remotes = self.remote_summaries(repository).await?;
        if !remotes
            .iter()
            .any(|candidate| candidate.name == remote && candidate.push_configured)
        {
            return Err(VcsError::NoRemote);
        }
        self.git(
            Some(repository),
            &["show-ref", "--verify", &format!("refs/heads/{branch}")],
        )
        .await?;
        let mut args = vec!["-c", "core.hooksPath=/dev/null", "push", "--porcelain"];
        if set_upstream {
            args.push("--set-upstream");
        }
        let refspec = format!("refs/heads/{branch}:refs/heads/{branch}");
        args.extend([remote, &refspec]);
        self.git_remote(repository, &args).await?;
        Ok(VcsPushResult {
            remote: remote.to_owned(),
            branch: branch.to_owned(),
            updated: true,
        })
    }

    /// Returns normalized pull-request metadata without exposing remote credentials.
    ///
    /// # Errors
    /// Rejects unsafe names and reports authentication/tool failures explicitly.
    pub async fn pull_request_metadata(
        &self,
        repository: &Path,
        remote: &str,
        head_branch: &str,
        base_branch: &str,
    ) -> Result<PullRequestMetadata, VcsError> {
        validate_remote(remote)?;
        validate_ref(head_branch)?;
        validate_ref(base_branch)?;
        let url = self
            .git(Some(repository), &["remote", "get-url", remote])
            .await
            .map_err(|_| VcsError::NoRemote)?;
        let Some(slug) = github_slug(url.trim()) else {
            return Ok(PullRequestMetadata {
                remote: remote.to_owned(),
                provider: PullRequestProvider::Unsupported,
                repository: None,
                head_branch: head_branch.to_owned(),
                base_branch: base_branch.to_owned(),
                compare_url: None,
                create_available: false,
                current: None,
            });
        };
        let current = if self.github_cli.is_some() {
            self.view_pull_request(repository, &slug, head_branch)
                .await?
        } else {
            None
        };
        Ok(PullRequestMetadata {
            remote: remote.to_owned(),
            provider: PullRequestProvider::GitHub,
            repository: Some(slug.clone()),
            head_branch: head_branch.to_owned(),
            base_branch: base_branch.to_owned(),
            compare_url: Some(format!(
                "https://github.com/{slug}/compare/{base_branch}...{head_branch}?expand=1"
            )),
            create_available: self.github_cli.is_some(),
            current,
        })
    }

    /// Creates a GitHub pull request through the authenticated structured GitHub CLI and then
    /// reads its normalized JSON representation.
    ///
    /// # Errors
    /// Rejects unsupported remotes, invalid input, unavailable CLI/auth, and malformed output.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_pull_request(
        &self,
        repository: &Path,
        remote: &str,
        head_branch: &str,
        base_branch: &str,
        title: &str,
        body: &str,
        draft: bool,
    ) -> Result<PullRequestSummary, VcsError> {
        validate_remote(remote)?;
        validate_ref(head_branch)?;
        validate_ref(base_branch)?;
        if title.trim().is_empty()
            || title.len() > 256
            || title.contains('\0')
            || body.len() > 64 * 1_024
            || body.contains('\0')
        {
            return Err(VcsError::Git("invalid pull request content".to_owned()));
        }
        let url = self
            .git(Some(repository), &["remote", "get-url", remote])
            .await
            .map_err(|_| VcsError::NoRemote)?;
        let slug = github_slug(url.trim()).ok_or(VcsError::PullRequestUnavailable)?;
        let mut arguments = vec![
            "pr",
            "create",
            "--repo",
            &slug,
            "--head",
            head_branch,
            "--base",
            base_branch,
            "--title",
            title.trim(),
            "--body",
            body,
        ];
        if draft {
            arguments.push("--draft");
        }
        self.gh(repository, &arguments, false).await?;
        self.view_pull_request(repository, &slug, head_branch)
            .await?
            .ok_or_else(|| {
                VcsError::Git("GitHub did not return the created pull request".to_owned())
            })
    }

    async fn view_pull_request(
        &self,
        repository: &Path,
        slug: &str,
        head_branch: &str,
    ) -> Result<Option<PullRequestSummary>, VcsError> {
        let output = self
            .gh(
                repository,
                &[
                    "pr",
                    "view",
                    head_branch,
                    "--repo",
                    slug,
                    "--json",
                    "number,url,state,title,headRefName,baseRefName",
                ],
                true,
            )
            .await?;
        let Some(output) = output else {
            return Ok(None);
        };
        let value: GhPullRequest = serde_json::from_str(&output)
            .map_err(|_| VcsError::Git("GitHub returned malformed pull request data".to_owned()))?;
        Ok(Some(value.into()))
    }

    async fn gh(
        &self,
        repository: &Path,
        arguments: &[&str],
        missing_is_none: bool,
    ) -> Result<Option<String>, VcsError> {
        let executable = self
            .github_cli
            .as_ref()
            .ok_or(VcsError::PullRequestUnavailable)?;
        let mut command = tokio::process::Command::new(executable);
        command
            .env_clear()
            .env("GH_PROMPT_DISABLED", "1")
            .env("LC_ALL", "C")
            .current_dir(repository)
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for key in ["HOME", "XDG_CONFIG_HOME", "GH_HOST", "GH_TOKEN"] {
            if let Ok(value) = std::env::var(key) {
                command.env(key, value);
            }
        }
        let output = timeout(self.command_timeout, command.output())
            .await
            .map_err(|_| VcsError::Timeout)??;
        if output.stdout.len() > super::MAX_OUTPUT_BYTES
            || output.stderr.len() > super::MAX_OUTPUT_BYTES
        {
            return Err(VcsError::OutputLimit);
        }
        if !output.status.success() {
            let message = String::from_utf8_lossy(&output.stderr);
            if super::is_authentication_error(&message) {
                return Err(VcsError::AuthenticationFailed);
            }
            if missing_is_none
                && (message.contains("no pull requests found")
                    || message.contains("Could not resolve to a PullRequest"))
            {
                return Ok(None);
            }
            return Err(VcsError::Git(
                "GitHub pull request operation failed".to_owned(),
            ));
        }
        String::from_utf8(output.stdout)
            .map(Some)
            .map_err(|_| VcsError::Git("GitHub returned invalid UTF-8".to_owned()))
    }

    async fn remote_summaries(&self, repository: &Path) -> Result<Vec<VcsRemoteSummary>, VcsError> {
        let output = self.git(Some(repository), &["remote", "-v"]).await?;
        let mut remotes = BTreeMap::<String, (bool, bool)>::new();
        for line in output.lines() {
            let mut fields = line.split_whitespace();
            let Some(name) = fields.next() else { continue };
            let _url = fields.next();
            let direction = fields.next();
            let entry = remotes.entry(name.to_owned()).or_default();
            match direction {
                Some("(fetch)") => entry.0 = true,
                Some("(push)") => entry.1 = true,
                _ => {}
            }
        }
        Ok(remotes
            .into_iter()
            .map(
                |(name, (fetch_configured, push_configured))| VcsRemoteSummary {
                    name,
                    fetch_configured,
                    push_configured,
                },
            )
            .collect())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhPullRequest {
    number: u64,
    url: String,
    title: String,
    state: String,
    head_ref_name: String,
    base_ref_name: String,
}

impl From<GhPullRequest> for PullRequestSummary {
    fn from(value: GhPullRequest) -> Self {
        Self {
            number: value.number,
            url: value.url,
            title: value.title,
            state: value.state.to_ascii_lowercase(),
            head_branch: value.head_ref_name,
            base_branch: value.base_ref_name,
        }
    }
}

fn github_slug(url: &str) -> Option<String> {
    let trimmed = url.trim_end_matches('/').trim_end_matches(".git");
    let candidate = trimmed
        .strip_prefix("git@github.com:")
        .or_else(|| trimmed.split_once("github.com/").map(|(_, path)| path))?;
    let mut parts = candidate.split('/');
    let owner = parts.next()?;
    let repository = parts.next()?;
    if parts.next().is_some()
        || [owner, repository].iter().any(|part| {
            part.is_empty()
                || !part
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || ".-_".contains(character))
        })
    {
        return None;
    }
    Some(format!("{owner}/{repository}"))
}

fn parse_status(bytes: &[u8]) -> Result<VcsStatus, VcsError> {
    let fields = super::checkpoints::nul_fields(bytes)?;
    let mut status = VcsStatus {
        head_oid: None,
        branch: None,
        detached: false,
        upstream: None,
        ahead: 0,
        behind: 0,
        conflicted: false,
        entries: Vec::new(),
        remotes: Vec::new(),
    };
    let mut cursor = 0;
    while cursor < fields.len() {
        let record = &fields[cursor];
        cursor += 1;
        if let Some(value) = record.strip_prefix("# branch.oid ") {
            if value != "(initial)" {
                status.head_oid = Some(super::checkpoints::oid(value)?);
            }
        } else if let Some(value) = record.strip_prefix("# branch.head ") {
            status.detached = value == "(detached)";
            if !status.detached && value != "(unknown)" {
                status.branch = Some(value.to_owned());
            }
        } else if let Some(value) = record.strip_prefix("# branch.upstream ") {
            status.upstream = Some(value.to_owned());
        } else if let Some(value) = record.strip_prefix("# branch.ab ") {
            for item in value.split_whitespace() {
                if let Some(ahead) = item.strip_prefix('+') {
                    status.ahead = ahead.parse().unwrap_or(0);
                } else if let Some(behind) = item.strip_prefix('-') {
                    status.behind = behind.parse().unwrap_or(0);
                }
            }
        } else if let Some(path) = record.strip_prefix("? ") {
            status.entries.push(VcsStatusEntry {
                path: safe_path(path)?,
                previous_path: None,
                staged: None,
                unstaged: Some(VcsChangeKind::Untracked),
                conflicted: false,
            });
        } else if record.starts_with("1 ") || record.starts_with("u ") {
            let unmerged = record.starts_with("u ");
            let limit = if unmerged { 11 } else { 9 };
            let parts = record.splitn(limit, ' ').collect::<Vec<_>>();
            let path = parts
                .last()
                .ok_or_else(|| VcsError::Git("invalid Git status output".to_owned()))?;
            let xy = parts.get(1).copied().unwrap_or("..");
            let entry = status_entry(path, None, xy, unmerged)?;
            status.conflicted |= entry.conflicted;
            status.entries.push(entry);
        } else if record.starts_with("2 ") {
            let parts = record.splitn(10, ' ').collect::<Vec<_>>();
            let path = parts
                .last()
                .ok_or_else(|| VcsError::Git("invalid Git rename status".to_owned()))?;
            let previous = fields
                .get(cursor)
                .ok_or_else(|| VcsError::Git("invalid Git rename source".to_owned()))?;
            cursor += 1;
            let xy = parts.get(1).copied().unwrap_or("..");
            status
                .entries
                .push(status_entry(path, Some(safe_path(previous)?), xy, false)?);
        }
    }
    status
        .entries
        .sort_by(|left, right| left.path.cmp(&right.path));
    Ok(status)
}

fn status_entry(
    path: &str,
    previous_path: Option<String>,
    xy: &str,
    unmerged: bool,
) -> Result<VcsStatusEntry, VcsError> {
    let mut bytes = xy.bytes();
    let staged = bytes.next().and_then(change_kind);
    let unstaged = bytes.next().and_then(change_kind);
    Ok(VcsStatusEntry {
        path: safe_path(path)?,
        previous_path,
        staged,
        unstaged,
        conflicted: unmerged || matches!(xy, "DD" | "AU" | "UD" | "UA" | "DU" | "AA" | "UU"),
    })
}

fn change_kind(value: u8) -> Option<VcsChangeKind> {
    match value {
        b'A' => Some(VcsChangeKind::Added),
        b'M' => Some(VcsChangeKind::Modified),
        b'D' => Some(VcsChangeKind::Deleted),
        b'R' => Some(VcsChangeKind::Renamed),
        b'C' => Some(VcsChangeKind::Copied),
        b'T' => Some(VcsChangeKind::TypeChanged),
        b'U' => Some(VcsChangeKind::Unmerged),
        _ => None,
    }
}

fn safe_path(value: &str) -> Result<String, VcsError> {
    super::checkpoints::safe_relative(value.to_owned())
}

fn validate_remote(value: &str) -> Result<(), VcsError> {
    if value.is_empty()
        || value.len() > 120
        || value.starts_with('-')
        || value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(VcsError::Git("invalid Git remote".to_owned()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn git(runtime: &GitRuntime, root: &Path, args: &[&str]) -> Result<String, VcsError> {
        runtime.git(Some(root), args).await
    }

    async fn repository() -> Result<(tempfile::TempDir, GitRuntime), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let runtime = GitRuntime::discover()?;
        git(&runtime, root.path(), &["init", "-b", "main"]).await?;
        git(
            &runtime,
            root.path(),
            &["config", "user.name", "HomeBot Fixture"],
        )
        .await?;
        git(
            &runtime,
            root.path(),
            &["config", "user.email", "fixture@homebot.invalid"],
        )
        .await?;
        tokio::fs::write(root.path().join("README.md"), "baseline\n").await?;
        git(&runtime, root.path(), &["add", "README.md"]).await?;
        git(&runtime, root.path(), &["commit", "-m", "baseline"]).await?;
        Ok((root, runtime))
    }

    #[tokio::test]
    async fn status_diffs_commit_branch_detached_and_local_push_are_normalized()
    -> Result<(), Box<dyn std::error::Error>> {
        let (repository, runtime) = repository().await?;
        tokio::fs::write(repository.path().join("README.md"), "unstaged\n").await?;
        tokio::fs::write(repository.path().join("staged.txt"), "staged\n").await?;
        tokio::fs::write(repository.path().join("untracked.txt"), "untracked\n").await?;
        git(&runtime, repository.path(), &["add", "staged.txt"]).await?;

        let status = runtime.source_status(repository.path()).await?;
        assert_eq!(status.branch.as_deref(), Some("main"));
        assert!(!status.detached);
        assert!(status.entries.iter().any(|entry| {
            entry.path == "README.md" && entry.unstaged == Some(VcsChangeKind::Modified)
        }));
        assert!(status.entries.iter().any(|entry| {
            entry.path == "staged.txt" && entry.staged == Some(VcsChangeKind::Added)
        }));
        assert!(status.entries.iter().any(|entry| {
            entry.path == "untracked.txt" && entry.unstaged == Some(VcsChangeKind::Untracked)
        }));
        assert!(
            runtime
                .working_diff(repository.path(), true)
                .await?
                .patch
                .contains("staged.txt")
        );
        assert!(
            runtime
                .working_diff(repository.path(), false)
                .await?
                .patch
                .contains("unstaged")
        );
        assert!(matches!(
            runtime
                .create_branch(repository.path(), "feature", None)
                .await,
            Err(VcsError::DirtyWorktree)
        ));

        let commit = runtime
            .commit(repository.path(), "capture workspace", true)
            .await?;
        assert_eq!(commit.branch.as_deref(), Some("main"));
        runtime
            .create_branch(repository.path(), "feature", Some("main"))
            .await?;

        let remote = tempfile::tempdir()?;
        git(&runtime, remote.path(), &["init", "--bare"]).await?;
        let remote_path = remote.path().to_str().ok_or(VcsError::InvalidPath)?;
        git(
            &runtime,
            repository.path(),
            &["remote", "add", "origin", remote_path],
        )
        .await?;
        let pushed = runtime
            .push(repository.path(), "origin", "feature", true)
            .await?;
        assert_eq!(pushed.remote, "origin");
        let status = runtime.source_status(repository.path()).await?;
        assert_eq!(status.upstream.as_deref(), Some("origin/feature"));
        assert_eq!(status.remotes.len(), 1);
        assert_eq!(status.remotes[0].name, "origin");

        git(
            &runtime,
            repository.path(),
            &["checkout", "--detach", "HEAD"],
        )
        .await?;
        let detached = runtime.source_status(repository.path()).await?;
        assert!(detached.detached);
        assert!(detached.branch.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn merge_conflict_and_missing_remote_are_explicit_without_losing_files()
    -> Result<(), Box<dyn std::error::Error>> {
        let (repository, runtime) = repository().await?;
        git(&runtime, repository.path(), &["switch", "-c", "side"]).await?;
        tokio::fs::write(repository.path().join("README.md"), "side\n").await?;
        git(&runtime, repository.path(), &["commit", "-am", "side"]).await?;
        git(&runtime, repository.path(), &["switch", "main"]).await?;
        tokio::fs::write(repository.path().join("README.md"), "main\n").await?;
        git(&runtime, repository.path(), &["commit", "-am", "main"]).await?;
        assert!(
            git(&runtime, repository.path(), &["merge", "side"])
                .await
                .is_err()
        );
        let status = runtime.source_status(repository.path()).await?;
        assert!(status.conflicted);
        assert!(status.entries.iter().any(|entry| entry.conflicted));
        assert_eq!(
            tokio::fs::read_to_string(repository.path().join("README.md")).await?,
            "<<<<<<< HEAD\nmain\n=======\nside\n>>>>>>> side\n"
        );
        assert!(matches!(
            runtime
                .push(repository.path(), "origin", "main", false)
                .await,
            Err(VcsError::NoRemote)
        ));
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn server_owned_mutations_do_not_execute_repository_hooks()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;

        let (repository, runtime) = repository().await?;
        let hooks = repository.path().join(".git/hooks");
        let install_hook = |name: &str, canary: &Path| -> Result<(), Box<dyn std::error::Error>> {
            let hook = hooks.join(name);
            std::fs::write(&hook, format!("#!/bin/sh\ntouch '{}'\n", canary.display()))?;
            let mut permissions = std::fs::metadata(&hook)?.permissions();
            permissions.set_mode(0o700);
            std::fs::set_permissions(hook, permissions)?;
            Ok(())
        };

        let commit_canary = repository.path().join("pre-commit-ran");
        install_hook("pre-commit", &commit_canary)?;
        tokio::fs::write(repository.path().join("safe.txt"), "safe\n").await?;
        runtime
            .commit(repository.path(), "safe commit", true)
            .await?;
        assert!(!commit_canary.exists());

        let checkout_canary = repository.path().join("post-checkout-ran");
        install_hook("post-checkout", &checkout_canary)?;
        runtime
            .create_branch(repository.path(), "safe-branch", None)
            .await?;
        assert!(!checkout_canary.exists());

        let remote = tempfile::tempdir()?;
        git(&runtime, remote.path(), &["init", "--bare"]).await?;
        git(
            &runtime,
            repository.path(),
            &[
                "remote",
                "add",
                "origin",
                remote.path().to_str().ok_or(VcsError::InvalidPath)?,
            ],
        )
        .await?;
        let push_canary = repository.path().join("pre-push-ran");
        install_hook("pre-push", &push_canary)?;
        runtime
            .push(repository.path(), "origin", "safe-branch", true)
            .await?;
        assert!(!push_canary.exists());
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn hostile_repository_config_is_denied_before_any_git_operation()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::io::Write as _;

        for hostile_config in [
            "[credential]\n\thelper = !touch credential-helper-ran",
            "[core]\n\tfsmonitor = !touch fsmonitor-ran",
            "[filter \"hostile\"]\n\tclean = touch filter-ran",
            "[diff \"hostile\"]\n\ttextconv = touch textconv-ran",
        ] {
            let (repository, runtime) = repository().await?;
            let mut config = std::fs::OpenOptions::new()
                .append(true)
                .open(repository.path().join(".git/config"))?;
            writeln!(config, "{hostile_config}")?;
            drop(config);

            assert!(matches!(
                runtime.source_status(repository.path()).await,
                Err(VcsError::UnsafeRepositoryConfig)
            ));
            for canary in [
                "credential-helper-ran",
                "fsmonitor-ran",
                "filter-ran",
                "textconv-ran",
            ] {
                assert!(!repository.path().join(canary).exists());
            }
        }
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn github_pull_request_metadata_and_create_use_normalized_cli_json()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;

        let (repository, runtime) = repository().await?;
        git(
            &runtime,
            repository.path(),
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/luinbytes/HomeBot.git",
            ],
        )
        .await?;
        let fixture = tempfile::tempdir()?;
        let gh = fixture.path().join("gh");
        tokio::fs::write(
            &gh,
            "#!/bin/sh\nif [ \"$2\" = \"create\" ]; then echo https://github.com/luinbytes/HomeBot/pull/42; exit 0; fi\necho '{\"number\":42,\"url\":\"https://github.com/luinbytes/HomeBot/pull/42\",\"title\":\"Safe change\",\"state\":\"OPEN\",\"headRefName\":\"main\",\"baseRefName\":\"trunk\"}'\n",
        )
        .await?;
        let mut permissions = tokio::fs::metadata(&gh).await?.permissions();
        permissions.set_mode(0o700);
        tokio::fs::set_permissions(&gh, permissions).await?;
        let runtime = runtime.with_github_cli(Some(gh));
        let metadata = runtime
            .pull_request_metadata(repository.path(), "origin", "main", "trunk")
            .await?;
        assert_eq!(metadata.provider, PullRequestProvider::GitHub);
        assert_eq!(metadata.repository.as_deref(), Some("luinbytes/HomeBot"));
        assert!(metadata.create_available);
        assert_eq!(
            metadata.current.as_ref().map(|value| value.number),
            Some(42)
        );
        assert_eq!(
            runtime
                .create_pull_request(
                    repository.path(),
                    "origin",
                    "main",
                    "trunk",
                    "Safe change",
                    "Verified body",
                    false,
                )
                .await?
                .number,
            42
        );
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn github_authentication_failure_is_normalized_without_secret_output()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;

        let (repository, runtime) = repository().await?;
        git(
            &runtime,
            repository.path(),
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/luinbytes/HomeBot.git",
            ],
        )
        .await?;
        let fixture = tempfile::tempdir()?;
        let gh = fixture.path().join("gh");
        tokio::fs::write(
            &gh,
            "#!/bin/sh\necho 'authentication failed: token super-secret-value' >&2\nexit 1\n",
        )
        .await?;
        let mut permissions = tokio::fs::metadata(&gh).await?.permissions();
        permissions.set_mode(0o700);
        tokio::fs::set_permissions(&gh, permissions).await?;
        let runtime = runtime.with_github_cli(Some(gh));
        let result = runtime
            .create_pull_request(
                repository.path(),
                "origin",
                "main",
                "trunk",
                "Safe change",
                "Verified body",
                false,
            )
            .await;
        let Err(error) = result else {
            return Err("authentication unexpectedly succeeded".into());
        };
        assert!(matches!(error, VcsError::AuthenticationFailed));
        assert!(!error.to_string().contains("super-secret-value"));
        Ok(())
    }

    #[test]
    fn github_remote_parser_rejects_non_github_and_credential_output() {
        assert_eq!(
            github_slug("git@github.com:luinbytes/HomeBot.git").as_deref(),
            Some("luinbytes/HomeBot")
        );
        assert_eq!(
            github_slug("https://secret@github.com/luinbytes/HomeBot.git").as_deref(),
            Some("luinbytes/HomeBot")
        );
        assert!(github_slug("https://example.com/luinbytes/HomeBot.git").is_none());
    }
}
