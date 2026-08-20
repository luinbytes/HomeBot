use crate::{
    ActivityKind, ActivitySink, ActivityStatus, CapabilityClass, CapabilityRequest,
    OperationContext, PolicyEngine, ToolActivity, ToolError,
};
use cap_std::{
    ambient_authority,
    fs::{Dir, OpenOptions},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::Arc,
};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct FilesystemLimits {
    pub max_read_bytes: usize,
    pub max_write_bytes: usize,
    pub max_directory_entries: usize,
}

impl Default for FilesystemLimits {
    fn default() -> Self {
        Self {
            max_read_bytes: 8 * 1024 * 1024,
            max_write_bytes: 8 * 1024 * 1024,
            max_directory_entries: 10_000,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DirectoryEntry {
    pub name: String,
    pub is_file: bool,
    pub is_directory: bool,
}

pub struct ScopedFilesystem {
    root_path: PathBuf,
    root: Arc<Dir>,
    policy: Arc<PolicyEngine>,
    activities: Arc<dyn ActivitySink>,
    limits: FilesystemLimits,
}

impl std::fmt::Debug for ScopedFilesystem {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ScopedFilesystem")
            .field("root_path", &self.root_path)
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl ScopedFilesystem {
    /// Opens an existing workspace root as a capability directory.
    ///
    /// # Errors
    ///
    /// Returns a safe error if the root is missing, not a directory, or cannot be opened.
    pub fn new(
        root: impl AsRef<Path>,
        policy: Arc<PolicyEngine>,
        activities: Arc<dyn ActivitySink>,
        limits: FilesystemLimits,
    ) -> Result<Self, ToolError> {
        let root_path = std::fs::canonicalize(root).map_err(|_| ToolError::Unavailable)?;
        if !root_path.is_dir() {
            return Err(ToolError::InvalidRequest(
                "workspace root is not a directory".to_owned(),
            ));
        }
        let directory = Dir::open_ambient_dir(&root_path, ambient_authority())
            .map_err(|_| ToolError::Unavailable)?;
        Ok(Self {
            root_path,
            root: Arc::new(directory),
            policy,
            activities,
            limits,
        })
    }

    /// Reads a regular workspace file up to the configured byte limit.
    ///
    /// # Errors
    ///
    /// Requires authorization and rejects traversal, symlinks and oversized files.
    pub async fn read(
        &self,
        context: OperationContext,
        path: impl AsRef<Path>,
        approval_id: Option<Uuid>,
    ) -> Result<Vec<u8>, ToolError> {
        let relative = validate_relative(path.as_ref(), false)?;
        ensure_no_symlinks(&self.root, &relative, false)?;
        let request = self.request(
            context,
            CapabilityClass::FilesystemRead,
            "filesystem.read",
            &relative,
            false,
        );
        let _authorization = self.policy.authorize(&request, approval_id).await?;
        self.emit(&request, ActivityStatus::Started, "Reading file")
            .await;
        let root = Arc::clone(&self.root);
        let limit = self.limits.max_read_bytes;
        let path_for_read = relative.clone();
        let result =
            tokio::task::spawn_blocking(move || read_bounded(&root, &path_for_read, limit))
                .await
                .map_err(|_| ToolError::OperationFailed)?;
        self.finish_activity(&request, "Read file", &result).await;
        result
    }

    /// Lists a workspace directory up to the configured entry limit.
    ///
    /// # Errors
    ///
    /// Requires authorization and rejects traversal, symlinks and oversized listings.
    pub async fn list(
        &self,
        context: OperationContext,
        path: impl AsRef<Path>,
        approval_id: Option<Uuid>,
    ) -> Result<Vec<DirectoryEntry>, ToolError> {
        let relative = validate_relative(path.as_ref(), true)?;
        ensure_no_symlinks(&self.root, &relative, false)?;
        let request = self.request(
            context,
            CapabilityClass::FilesystemRead,
            "filesystem.list",
            &relative,
            false,
        );
        let _authorization = self.policy.authorize(&request, approval_id).await?;
        self.emit(&request, ActivityStatus::Started, "Listing directory")
            .await;
        let root = Arc::clone(&self.root);
        let limit = self.limits.max_directory_entries;
        let path_for_list = relative.clone();
        let result =
            tokio::task::spawn_blocking(move || list_bounded(&root, &path_for_list, limit))
                .await
                .map_err(|_| ToolError::OperationFailed)?;
        self.finish_activity(&request, "Listed directory", &result)
            .await;
        result
    }

    /// Atomically replaces a workspace file after authorization.
    ///
    /// # Errors
    ///
    /// Rejects traversal, symlinks, oversized contents and failed atomic replacement.
    pub async fn write(
        &self,
        context: OperationContext,
        path: impl AsRef<Path>,
        contents: Vec<u8>,
        approval_id: Option<Uuid>,
    ) -> Result<(), ToolError> {
        if contents.len() > self.limits.max_write_bytes {
            return Err(ToolError::LimitExceeded);
        }
        let relative = validate_relative(path.as_ref(), false)?;
        ensure_no_symlinks(&self.root, &relative, true)?;
        let mut request = self.request(
            context,
            CapabilityClass::FilesystemWrite,
            "filesystem.write",
            &relative,
            false,
        );
        request.canonical_resource = format!(
            "{}:content-sha256:{:x}",
            request.canonical_resource,
            Sha256::digest(&contents)
        );
        let _authorization = self.policy.authorize(&request, approval_id).await?;
        self.emit(&request, ActivityStatus::Started, "Writing file")
            .await;
        let root = Arc::clone(&self.root);
        let path_for_write = relative.clone();
        let result =
            tokio::task::spawn_blocking(move || write_atomic(&root, &path_for_write, &contents))
                .await
                .map_err(|_| ToolError::OperationFailed)?;
        self.finish_activity(&request, "Wrote file", &result).await;
        result
    }

    /// Creates one directory inside the workspace.
    ///
    /// # Errors
    ///
    /// Requires authorization and rejects traversal and symlink components.
    pub async fn create_directory(
        &self,
        context: OperationContext,
        path: impl AsRef<Path>,
        approval_id: Option<Uuid>,
    ) -> Result<(), ToolError> {
        let relative = validate_relative(path.as_ref(), false)?;
        ensure_no_symlinks(&self.root, &relative, true)?;
        let request = self.request(
            context,
            CapabilityClass::FilesystemWrite,
            "filesystem.create_directory",
            &relative,
            false,
        );
        let _authorization = self.policy.authorize(&request, approval_id).await?;
        self.emit(&request, ActivityStatus::Started, "Creating directory")
            .await;
        let root = Arc::clone(&self.root);
        let path_for_create = relative.clone();
        let result = tokio::task::spawn_blocking(move || {
            root.create_dir(&path_for_create)
                .map_err(|_| ToolError::OperationFailed)
        })
        .await
        .map_err(|_| ToolError::OperationFailed)?;
        self.finish_activity(&request, "Created directory", &result)
            .await;
        result
    }

    /// Removes one non-symlink file inside the workspace.
    ///
    /// # Errors
    ///
    /// Requires authorization and rejects traversal, symlinks and failed removal.
    pub async fn remove_file(
        &self,
        context: OperationContext,
        path: impl AsRef<Path>,
        approval_id: Option<Uuid>,
    ) -> Result<(), ToolError> {
        let relative = validate_relative(path.as_ref(), false)?;
        ensure_no_symlinks(&self.root, &relative, false)?;
        let request = self.request(
            context,
            CapabilityClass::FilesystemWrite,
            "filesystem.remove_file",
            &relative,
            true,
        );
        let _authorization = self.policy.authorize(&request, approval_id).await?;
        self.emit(&request, ActivityStatus::Started, "Removing file")
            .await;
        let root = Arc::clone(&self.root);
        let path_for_remove = relative.clone();
        let result = tokio::task::spawn_blocking(move || {
            root.remove_file(&path_for_remove)
                .map_err(|_| ToolError::OperationFailed)
        })
        .await
        .map_err(|_| ToolError::OperationFailed)?;
        self.finish_activity(&request, "Removed file", &result)
            .await;
        result
    }

    fn request(
        &self,
        context: OperationContext,
        capability: CapabilityClass,
        action: &str,
        relative: &Path,
        destructive: bool,
    ) -> CapabilityRequest {
        let display = if relative.as_os_str().is_empty() {
            self.root_path.clone()
        } else {
            self.root_path.join(relative)
        };
        CapabilityRequest {
            context,
            capability,
            action: action.to_owned(),
            canonical_resource: display.to_string_lossy().into_owned(),
            summary: format!("{action} {}", relative.display()),
            destructive,
        }
    }

    async fn emit(&self, request: &CapabilityRequest, status: ActivityStatus, title: &str) {
        self.activities
            .emit(ToolActivity::new(
                request.context.operation_id,
                ActivityKind::Filesystem,
                status,
                title,
                Some(request.canonical_resource.clone()),
            ))
            .await;
    }

    async fn finish_activity<T>(
        &self,
        request: &CapabilityRequest,
        success_title: &str,
        result: &Result<T, ToolError>,
    ) {
        let (status, title) = if result.is_ok() {
            (ActivityStatus::Completed, success_title)
        } else {
            (ActivityStatus::Failed, "Filesystem operation failed")
        };
        self.emit(request, status, title).await;
    }
}

fn validate_relative(path: &Path, allow_empty: bool) -> Result<PathBuf, ToolError> {
    if path.as_os_str().is_empty() && allow_empty {
        return Ok(PathBuf::new());
    }
    if path.as_os_str().is_empty() || path.is_absolute() {
        return Err(ToolError::PathOutsideWorkspace);
    }
    let mut clean = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(component) => clean.push(component),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(ToolError::PathOutsideWorkspace);
            }
        }
    }
    if clean.as_os_str().is_empty() && !allow_empty {
        return Err(ToolError::PathOutsideWorkspace);
    }
    Ok(clean)
}

fn ensure_no_symlinks(root: &Dir, path: &Path, allow_missing_leaf: bool) -> Result<(), ToolError> {
    let components = path.components().count();
    let mut current = PathBuf::new();
    for (index, component) in path.components().enumerate() {
        current.push(component.as_os_str());
        match root.symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(ToolError::SymlinkRejected);
            }
            Ok(_) => {}
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    && allow_missing_leaf
                    && index + 1 == components =>
            {
                return Ok(());
            }
            Err(_) => return Err(ToolError::OperationFailed),
        }
    }
    Ok(())
}

fn read_bounded(root: &Dir, path: &Path, limit: usize) -> Result<Vec<u8>, ToolError> {
    let file = root.open(path).map_err(|_| ToolError::OperationFailed)?;
    let metadata = file.metadata().map_err(|_| ToolError::OperationFailed)?;
    if metadata.len() > u64::try_from(limit).unwrap_or(u64::MAX) {
        return Err(ToolError::LimitExceeded);
    }
    let mut output = Vec::new();
    file.take(u64::try_from(limit.saturating_add(1)).unwrap_or(u64::MAX))
        .read_to_end(&mut output)
        .map_err(|_| ToolError::OperationFailed)?;
    if output.len() > limit {
        return Err(ToolError::LimitExceeded);
    }
    Ok(output)
}

fn list_bounded(root: &Dir, path: &Path, limit: usize) -> Result<Vec<DirectoryEntry>, ToolError> {
    let mut output = Vec::new();
    let path = if path.as_os_str().is_empty() {
        Path::new(".")
    } else {
        path
    };
    let entries = root
        .read_dir(path)
        .map_err(|_| ToolError::OperationFailed)?;
    for entry in entries {
        if output.len() >= limit {
            return Err(ToolError::LimitExceeded);
        }
        let entry = entry.map_err(|_| ToolError::OperationFailed)?;
        let file_type = entry.file_type().map_err(|_| ToolError::OperationFailed)?;
        output.push(DirectoryEntry {
            name: entry.file_name().to_string_lossy().into_owned(),
            is_file: file_type.is_file(),
            is_directory: file_type.is_dir(),
        });
    }
    output.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(output)
}

fn write_atomic(root: &Dir, path: &Path, contents: &[u8]) -> Result<(), ToolError> {
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let temp_name = format!(".homebot-write-{}", Uuid::now_v7());
    let temporary = parent.join(temp_name);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = root
        .open_with(&temporary, &options)
        .map_err(|_| ToolError::OperationFailed)?;
    let result = file
        .write_all(contents)
        .and_then(|()| file.sync_all())
        .map_err(|_| ToolError::OperationFailed)
        .and_then(|()| {
            drop(file);
            root.rename(&temporary, root, path)
                .map_err(|_| ToolError::OperationFailed)
        });
    if result.is_err() {
        let _ = root.remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PolicyEffect, PolicyRule, RecordingActivitySink};
    use std::time::Duration;

    fn context(workspace_id: Uuid) -> OperationContext {
        OperationContext {
            operation_id: Uuid::now_v7(),
            owner_id: Uuid::now_v7(),
            device_id: Uuid::now_v7(),
            bot_id: Uuid::now_v7(),
            chat_id: Uuid::now_v7(),
            workspace_id,
        }
    }

    async fn allowed_filesystem(root: &Path) -> Result<ScopedFilesystem, ToolError> {
        let activities = Arc::new(RecordingActivitySink::default());
        let policy = Arc::new(PolicyEngine::new(
            Duration::from_secs(60),
            activities.clone(),
        ));
        policy
            .replace_rules(vec![
                PolicyRule::new(CapabilityClass::FilesystemRead, PolicyEffect::Allow),
                PolicyRule::new(CapabilityClass::FilesystemWrite, PolicyEffect::Allow),
            ])
            .await;
        ScopedFilesystem::new(root, policy, activities, FilesystemLimits::default())
    }

    #[tokio::test]
    async fn scoped_operations_are_atomic_bounded_and_emit_activity()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let filesystem = allowed_filesystem(directory.path()).await?;
        let workspace_id = Uuid::now_v7();
        filesystem
            .write(context(workspace_id), "notes.txt", b"hello".to_vec(), None)
            .await
            .map_err(|error| format!("write failed: {error:?}"))?;
        assert_eq!(
            filesystem
                .read(context(workspace_id), "notes.txt", None)
                .await
                .map_err(|error| format!("read failed: {error:?}"))?,
            b"hello"
        );
        assert_eq!(
            filesystem
                .list(context(workspace_id), "", None)
                .await
                .map_err(|error| format!("list failed: {error:?}"))?[0]
                .name,
            "notes.txt"
        );
        filesystem
            .remove_file(context(workspace_id), "notes.txt", None)
            .await
            .map_err(|error| format!("remove failed: {error:?}"))?;
        assert!(!directory.path().join("notes.txt").exists());
        Ok(())
    }

    #[tokio::test]
    async fn traversal_and_symlink_escape_are_rejected_before_policy()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let outside = tempfile::tempdir()?;
        std::fs::write(outside.path().join("secret"), "canary")?;
        let filesystem = allowed_filesystem(directory.path()).await?;
        let workspace_id = Uuid::now_v7();
        assert_eq!(
            filesystem
                .read(context(workspace_id), "../secret", None)
                .await,
            Err(ToolError::PathOutsideWorkspace)
        );
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(outside.path(), directory.path().join("escape"))?;
            assert_eq!(
                filesystem
                    .read(context(workspace_id), "escape/secret", None)
                    .await,
                Err(ToolError::SymlinkRejected)
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn server_policy_blocks_mutation_before_filesystem_side_effect()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let activities = Arc::new(RecordingActivitySink::default());
        let policy = Arc::new(PolicyEngine::new(
            Duration::from_secs(60),
            activities.clone(),
        ));
        let filesystem = ScopedFilesystem::new(
            directory.path(),
            policy.clone(),
            activities,
            FilesystemLimits::default(),
        )?;
        let operation = context(Uuid::now_v7());
        let ToolError::ApprovalRequired(ticket) = filesystem
            .write(operation.clone(), "blocked", b"first".to_vec(), None)
            .await
            .err()
            .unwrap_or(ToolError::OperationFailed)
        else {
            return Err("expected approval".into());
        };
        policy
            .decide(ticket.approval_id, crate::ApprovalDecision::AllowOnce)
            .await?;
        assert_eq!(
            filesystem
                .write(
                    operation,
                    "blocked",
                    b"substituted".to_vec(),
                    Some(ticket.approval_id),
                )
                .await,
            Err(ToolError::InvalidApproval)
        );
        assert!(!directory.path().join("blocked").exists());
        Ok(())
    }
}
