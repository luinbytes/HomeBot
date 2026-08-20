use super::{GitRuntime, VcsError, ensure_managed};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, path::Path};
use uuid::Uuid;

const MAX_DIFF_BYTES: usize = 8 * 1_024 * 1_024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckpointCapture {
    pub checkpoint_id: Uuid,
    pub git_ref: String,
    pub commit_oid: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointPhase {
    BeforeTurn,
    AfterTurn,
    RestoreSafety,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationReconciliation {
    Unchanged,
    Forked,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Unmerged,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FileChange {
    pub status: FileChangeStatus,
    pub path: String,
    pub previous_path: Option<String>,
    pub binary: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckpointDiff {
    pub patch: String,
    pub files: Vec<FileChange>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RestoreResult {
    pub safety_checkpoint: CheckpointCapture,
    pub restored_commit_oid: String,
}

impl GitRuntime {
    /// Captures tracked, staged, unstaged, and non-ignored untracked files through a temporary
    /// index, then anchors the resulting commit under a hidden `HomeBot` ref.
    ///
    /// The user's index and checked-out branch are never changed.
    ///
    /// # Errors
    /// Returns a bounded Git or filesystem error and removes the temporary index best-effort.
    pub async fn capture_checkpoint(
        &self,
        repository: &Path,
        metadata_root: &Path,
        chat_id: Uuid,
        checkpoint_id: Uuid,
    ) -> Result<CheckpointCapture, VcsError> {
        let repository = self.inspect_repository(repository).await?.root;
        tokio::fs::create_dir_all(metadata_root).await?;
        let metadata_root = tokio::fs::canonicalize(metadata_root).await?;
        let index = metadata_root.join(format!("{}.index", checkpoint_id.simple()));
        if tokio::fs::try_exists(&index).await? {
            return Err(VcsError::Git("checkpoint index already exists".to_owned()));
        }
        let index_value = index.to_str().ok_or(VcsError::InvalidPath)?;
        let environment = [("GIT_INDEX_FILE", index_value)];
        let result = async {
            let head = self.git(Some(&repository), &["rev-parse", "HEAD"]).await?;
            let head = oid(head.trim())?;
            self.git_bytes(
                Some(&repository),
                &["read-tree", &head],
                &environment,
                super::MAX_OUTPUT_BYTES,
            )
            .await?;
            self.git_bytes(
                Some(&repository),
                &["add", "--all", "--", "."],
                &environment,
                super::MAX_OUTPUT_BYTES,
            )
            .await?;
            let tree = self
                .git_bytes(
                    Some(&repository),
                    &["write-tree"],
                    &environment,
                    super::MAX_OUTPUT_BYTES,
                )
                .await?;
            let tree = String::from_utf8(tree)
                .map_err(|_| VcsError::Git("Git returned invalid UTF-8".to_owned()))?;
            let tree = oid(tree.trim())?;
            let message = format!("HomeBot checkpoint {checkpoint_id}");
            let commit = self
                .git(
                    Some(&repository),
                    &[
                        "-c",
                        "user.name=HomeBot",
                        "-c",
                        "user.email=checkpoint@homebot.invalid",
                        "commit-tree",
                        &tree,
                        "-p",
                        &head,
                        "-m",
                        &message,
                    ],
                )
                .await?;
            let commit_oid = oid(commit.trim())?;
            let git_ref = format!(
                "refs/homebot/checkpoints/{}/{}",
                chat_id.simple(),
                checkpoint_id.simple()
            );
            self.git(
                Some(&repository),
                &["update-ref", &git_ref, &commit_oid, ""],
            )
            .await?;
            Ok(CheckpointCapture {
                checkpoint_id,
                git_ref,
                commit_oid,
            })
        }
        .await;
        let _ = tokio::fs::remove_file(&index).await;
        let _ = tokio::fs::remove_file(index.with_extension("index.lock")).await;
        result
    }

    /// Produces an exact binary-capable patch and normalized changed-file summary.
    ///
    /// # Errors
    /// Returns when either object is invalid or bounded Git output cannot be decoded.
    pub async fn checkpoint_diff(
        &self,
        repository: &Path,
        from_commit: &str,
        to_commit: &str,
    ) -> Result<CheckpointDiff, VcsError> {
        let from_commit = oid(from_commit)?;
        let to_commit = oid(to_commit)?;
        let patch = self
            .git_bytes(
                Some(repository),
                &[
                    "diff",
                    "--binary",
                    "--full-index",
                    "--find-renames",
                    &from_commit,
                    &to_commit,
                    "--",
                ],
                &[],
                MAX_DIFF_BYTES,
            )
            .await?;
        let names = self
            .git_bytes(
                Some(repository),
                &[
                    "diff",
                    "--name-status",
                    "-z",
                    "--find-renames",
                    &from_commit,
                    &to_commit,
                    "--",
                ],
                &[],
                MAX_DIFF_BYTES,
            )
            .await?;
        let binary = binary_paths(
            &self
                .git_bytes(
                    Some(repository),
                    &["diff", "--numstat", "-z", &from_commit, &to_commit, "--"],
                    &[],
                    MAX_DIFF_BYTES,
                )
                .await?,
        )?;
        Ok(CheckpointDiff {
            patch: String::from_utf8(patch)
                .map_err(|_| VcsError::Git("Git diff returned invalid UTF-8".to_owned()))?,
            files: parse_name_status(&names, &binary)?,
        })
    }

    /// Restores a captured tree after first anchoring the current state as a safety checkpoint.
    /// The real index and branch are preserved.
    ///
    /// # Errors
    /// Returns before applying an invalid target, or with the safety ref retained after a partial
    /// filesystem failure so recovery remains possible.
    pub async fn restore_checkpoint(
        &self,
        repository: &Path,
        metadata_root: &Path,
        chat_id: Uuid,
        target_commit: &str,
        safety_checkpoint_id: Uuid,
    ) -> Result<RestoreResult, VcsError> {
        let target_commit = oid(target_commit)?;
        self.git(
            Some(repository),
            &["cat-file", "-e", &format!("{target_commit}^{{commit}}")],
        )
        .await?;
        let safety = self
            .capture_checkpoint(repository, metadata_root, chat_id, safety_checkpoint_id)
            .await?;
        let repository = self.inspect_repository(repository).await?.root;
        let current_paths = self.tree_paths(&repository, &safety.commit_oid).await?;
        let target_paths = self.tree_paths(&repository, &target_commit).await?;
        let ignored_paths = self.ignored_paths(&repository).await?;
        if ignored_paths
            .iter()
            .any(|path| target_paths.contains(path) && !current_paths.contains(path))
        {
            return Err(VcsError::RestoreConflict);
        }
        for path in current_paths.difference(&target_paths) {
            remove_captured_path(&repository, path).await?;
        }
        tokio::fs::create_dir_all(metadata_root).await?;
        let metadata_root = tokio::fs::canonicalize(metadata_root).await?;
        let index = metadata_root.join(format!("{}.restore-index", safety_checkpoint_id.simple()));
        let index_value = index.to_str().ok_or(VcsError::InvalidPath)?;
        let environment = [("GIT_INDEX_FILE", index_value)];
        let result = async {
            self.git_bytes(
                Some(&repository),
                &["read-tree", &target_commit],
                &environment,
                super::MAX_OUTPUT_BYTES,
            )
            .await?;
            let prefix = format!("{}/", repository.to_str().ok_or(VcsError::InvalidPath)?);
            self.git_bytes(
                Some(&repository),
                &[
                    "checkout-index",
                    "--all",
                    "--force",
                    &format!("--prefix={prefix}"),
                ],
                &environment,
                super::MAX_OUTPUT_BYTES,
            )
            .await?;
            Ok(RestoreResult {
                safety_checkpoint: safety,
                restored_commit_oid: target_commit,
            })
        }
        .await;
        let _ = tokio::fs::remove_file(&index).await;
        let _ = tokio::fs::remove_file(index.with_extension("restore-index.lock")).await;
        result
    }

    async fn tree_paths(
        &self,
        repository: &Path,
        commit: &str,
    ) -> Result<BTreeSet<String>, VcsError> {
        let bytes = self
            .git_bytes(
                Some(repository),
                &["ls-tree", "-r", "--name-only", "-z", commit],
                &[],
                MAX_DIFF_BYTES,
            )
            .await?;
        nul_fields(&bytes)?.into_iter().map(safe_relative).collect()
    }

    async fn ignored_paths(&self, repository: &Path) -> Result<BTreeSet<String>, VcsError> {
        let bytes = self
            .git_bytes(
                Some(repository),
                &[
                    "ls-files",
                    "--others",
                    "--ignored",
                    "--exclude-standard",
                    "-z",
                ],
                &[],
                MAX_DIFF_BYTES,
            )
            .await?;
        nul_fields(&bytes)?.into_iter().map(safe_relative).collect()
    }
}

fn oid(value: &str) -> Result<String, VcsError> {
    if value.len() != 40 && value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(VcsError::Git("invalid Git object ID".to_owned()));
    }
    Ok(value.to_ascii_lowercase())
}

fn nul_fields(bytes: &[u8]) -> Result<Vec<String>, VcsError> {
    bytes
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty())
        .map(|field| {
            std::str::from_utf8(field)
                .map(str::to_owned)
                .map_err(|_| VcsError::Git("Git returned a non-UTF-8 path".to_owned()))
        })
        .collect()
}

fn safe_relative(path: String) -> Result<String, VcsError> {
    let candidate = Path::new(&path);
    if path.is_empty()
        || candidate.is_absolute()
        || candidate
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(VcsError::UnsafeWorktreePath);
    }
    Ok(path)
}

fn binary_paths(bytes: &[u8]) -> Result<BTreeSet<String>, VcsError> {
    let mut result = BTreeSet::new();
    for entry in bytes
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
    {
        let value = std::str::from_utf8(entry)
            .map_err(|_| VcsError::Git("Git returned a non-UTF-8 path".to_owned()))?;
        let mut fields = value.splitn(3, '\t');
        if matches!((fields.next(), fields.next()), (Some("-"), Some("-")))
            && let Some(path) = fields.next()
        {
            result.insert(safe_relative(path.to_owned())?);
        }
    }
    Ok(result)
}

fn parse_name_status(bytes: &[u8], binary: &BTreeSet<String>) -> Result<Vec<FileChange>, VcsError> {
    let fields = nul_fields(bytes)?;
    let mut cursor = 0;
    let mut changes = Vec::new();
    while cursor < fields.len() {
        let code = &fields[cursor];
        cursor += 1;
        let status = match code.as_bytes().first().copied() {
            Some(b'A') => FileChangeStatus::Added,
            Some(b'M') => FileChangeStatus::Modified,
            Some(b'D') => FileChangeStatus::Deleted,
            Some(b'R') => FileChangeStatus::Renamed,
            Some(b'C') => FileChangeStatus::Copied,
            Some(b'T') => FileChangeStatus::TypeChanged,
            Some(b'U') => FileChangeStatus::Unmerged,
            _ => FileChangeStatus::Unknown,
        };
        let rename = matches!(status, FileChangeStatus::Renamed | FileChangeStatus::Copied);
        let first = fields
            .get(cursor)
            .ok_or_else(|| VcsError::Git("invalid name-status output".to_owned()))?;
        cursor += 1;
        let (previous_path, path) = if rename {
            let destination = fields
                .get(cursor)
                .ok_or_else(|| VcsError::Git("invalid rename output".to_owned()))?;
            cursor += 1;
            (
                Some(safe_relative(first.clone())?),
                safe_relative(destination.clone())?,
            )
        } else {
            (None, safe_relative(first.clone())?)
        };
        changes.push(FileChange {
            status,
            binary: binary.contains(&path),
            path,
            previous_path,
        });
    }
    Ok(changes)
}

async fn remove_captured_path(root: &Path, relative: &str) -> Result<(), VcsError> {
    let target = root.join(relative);
    ensure_managed(root, &target)?;
    match tokio::fs::symlink_metadata(&target).await {
        Ok(metadata) if metadata.is_dir() => tokio::fs::remove_dir_all(&target).await?,
        Ok(_) => tokio::fs::remove_file(&target).await?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let mut parent = target.parent();
    while let Some(directory) = parent {
        if directory == root {
            break;
        }
        if tokio::fs::remove_dir(directory).await.is_err() {
            break;
        }
        parent = directory.parent();
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
        tokio::fs::write(root.path().join("README.md"), "committed\n").await?;
        git(&runtime, root.path(), &["add", "README.md"]).await?;
        git(&runtime, root.path(), &["commit", "-m", "baseline"]).await?;
        Ok((root, runtime))
    }

    #[tokio::test]
    async fn exact_diff_and_restore_preserve_dirty_baseline_index_binary_untracked_and_rename()
    -> Result<(), Box<dyn std::error::Error>> {
        let (repository, runtime) = repository().await?;
        tokio::fs::write(repository.path().join("README.md"), "dirty baseline\n").await?;
        tokio::fs::write(repository.path().join("baseline.txt"), "before\n").await?;
        tokio::fs::write(repository.path().join("staged.txt"), "staged baseline\n").await?;
        git(&runtime, repository.path(), &["add", "staged.txt"]).await?;
        let real_index_before = git(&runtime, repository.path(), &["write-tree"]).await?;
        let metadata = tempfile::tempdir()?;
        let chat_id = Uuid::now_v7();
        let pre = runtime
            .capture_checkpoint(repository.path(), metadata.path(), chat_id, Uuid::now_v7())
            .await?;
        assert_eq!(
            git(&runtime, repository.path(), &["write-tree"]).await?,
            real_index_before
        );

        tokio::fs::rename(
            repository.path().join("README.md"),
            repository.path().join("GUIDE.md"),
        )
        .await?;
        tokio::fs::write(repository.path().join("baseline.txt"), "after\n").await?;
        tokio::fs::write(
            repository.path().join("binary.dat"),
            [0_u8, 159, 146, 150, 0, 255],
        )
        .await?;
        let post = runtime
            .capture_checkpoint(repository.path(), metadata.path(), chat_id, Uuid::now_v7())
            .await?;
        let diff = runtime
            .checkpoint_diff(repository.path(), &pre.commit_oid, &post.commit_oid)
            .await?;
        assert!(diff.patch.contains("GIT binary patch"));
        assert!(
            diff.files
                .iter()
                .any(|file| file.path == "binary.dat" && file.binary)
        );
        assert!(diff.files.iter().any(|file| file.path == "baseline.txt"));
        assert!(diff.files.iter().any(|file| {
            file.status == FileChangeStatus::Renamed
                && file.previous_path.as_deref() == Some("README.md")
                && file.path == "GUIDE.md"
        }));

        let restored = runtime
            .restore_checkpoint(
                repository.path(),
                metadata.path(),
                chat_id,
                &pre.commit_oid,
                Uuid::now_v7(),
            )
            .await?;
        assert_eq!(restored.restored_commit_oid, pre.commit_oid);
        assert_eq!(
            tokio::fs::read_to_string(repository.path().join("README.md")).await?,
            "dirty baseline\n"
        );
        assert_eq!(
            tokio::fs::read_to_string(repository.path().join("baseline.txt")).await?,
            "before\n"
        );
        assert!(!repository.path().join("GUIDE.md").exists());
        assert!(!repository.path().join("binary.dat").exists());
        assert_eq!(
            git(&runtime, repository.path(), &["write-tree"]).await?,
            real_index_before
        );
        assert_eq!(
            git(
                &runtime,
                repository.path(),
                &["rev-parse", &restored.safety_checkpoint.git_ref],
            )
            .await?
            .trim(),
            restored.safety_checkpoint.commit_oid
        );
        Ok(())
    }

    #[tokio::test]
    async fn restore_refuses_to_overwrite_ignored_content_missing_from_safety_checkpoint()
    -> Result<(), Box<dyn std::error::Error>> {
        let (repository, runtime) = repository().await?;
        tokio::fs::write(
            repository.path().join("generated.txt"),
            "checkpoint value\n",
        )
        .await?;
        git(&runtime, repository.path(), &["add", "generated.txt"]).await?;
        git(
            &runtime,
            repository.path(),
            &["commit", "-m", "tracked target"],
        )
        .await?;
        let metadata = tempfile::tempdir()?;
        let chat_id = Uuid::now_v7();
        let target = runtime
            .capture_checkpoint(repository.path(), metadata.path(), chat_id, Uuid::now_v7())
            .await?;

        tokio::fs::write(repository.path().join(".gitignore"), "generated.txt\n").await?;
        tokio::fs::remove_file(repository.path().join("generated.txt")).await?;
        git(
            &runtime,
            repository.path(),
            &["add", ".gitignore", "generated.txt"],
        )
        .await?;
        git(
            &runtime,
            repository.path(),
            &["commit", "-m", "ignore generated output"],
        )
        .await?;
        tokio::fs::write(
            repository.path().join("generated.txt"),
            "valuable ignored value\n",
        )
        .await?;

        let result = runtime
            .restore_checkpoint(
                repository.path(),
                metadata.path(),
                chat_id,
                &target.commit_oid,
                Uuid::now_v7(),
            )
            .await;
        assert!(matches!(result, Err(VcsError::RestoreConflict)));
        assert_eq!(
            tokio::fs::read_to_string(repository.path().join("generated.txt")).await?,
            "valuable ignored value\n"
        );
        Ok(())
    }
}
