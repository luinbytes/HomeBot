use crate::ApprovalTicket;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityClass {
    FilesystemRead,
    FilesystemWrite,
    ProcessExecute,
    BrowserObserve,
    BrowserAct,
    GitRead,
    GitWrite,
    GitRemote,
    PluginRead,
    PluginWrite,
    ExternalCommunication,
    ExternalMutation,
    SecretUse,
    DeviceAdministration,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OperationContext {
    pub operation_id: Uuid,
    pub owner_id: Uuid,
    pub device_id: Uuid,
    pub bot_id: Uuid,
    pub chat_id: Uuid,
    pub workspace_id: Uuid,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CapabilityRequest {
    pub context: OperationContext,
    pub capability: CapabilityClass,
    pub action: String,
    /// Canonical, server-derived resource identity. Never client display text.
    pub canonical_resource: String,
    pub summary: String,
    pub destructive: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityKind {
    Filesystem,
    Terminal,
    Browser,
    Approval,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityStatus {
    Started,
    Updated,
    Completed,
    Cancelled,
    Failed,
    Denied,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolActivity {
    pub activity_id: Uuid,
    pub operation_id: Uuid,
    pub kind: ActivityKind,
    pub status: ActivityStatus,
    pub title: String,
    pub detail: Option<String>,
    pub occurred_at_unix_ms: u64,
}

impl ToolActivity {
    #[must_use]
    pub fn new(
        operation_id: Uuid,
        kind: ActivityKind,
        status: ActivityStatus,
        title: impl Into<String>,
        detail: Option<String>,
    ) -> Self {
        Self {
            activity_id: Uuid::now_v7(),
            operation_id,
            kind,
            status,
            title: title.into(),
            detail,
            occurred_at_unix_ms: unix_ms(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ToolError {
    #[error("capability denied")]
    Denied,
    #[error("approval required")]
    ApprovalRequired(ApprovalTicket),
    #[error("approval is invalid or no longer usable")]
    InvalidApproval,
    #[error("requested path is outside the workspace boundary")]
    PathOutsideWorkspace,
    #[error("symbolic links are not accepted for this operation")]
    SymlinkRejected,
    #[error("request exceeds a configured limit")]
    LimitExceeded,
    #[error("request is invalid: {0}")]
    InvalidRequest(String),
    #[error("local capability is unavailable")]
    Unavailable,
    #[error("local capability operation failed")]
    OperationFailed,
    #[error("operation was cancelled")]
    Cancelled,
    #[error("operation timed out")]
    TimedOut,
    #[error("browser protocol failed")]
    BrowserProtocol,
}

pub(crate) fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}
