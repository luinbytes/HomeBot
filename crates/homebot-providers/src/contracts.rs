//! Provider-neutral discovery, execution, streaming, and recovery contracts.

use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, fmt};
use tokio::sync::mpsc;
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ProviderAdapterId(String);

impl ProviderAdapterId {
    /// Creates a stable adapter identifier such as `codex` or `claude-code`.
    ///
    /// # Errors
    ///
    /// Rejects empty or non-ASCII identifiers and characters outside the safe slug set.
    pub fn new(value: impl Into<String>) -> Result<Self, ProviderContractError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err(ProviderContractError::InvalidAdapterId);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProviderAdapterId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderAvailability {
    Available,
    AuthenticationRequired,
    NotInstalled,
    Degraded,
    Unavailable,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderHealth {
    pub availability: ProviderAvailability,
    pub message: String,
    pub checked_at_unix_ms: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderCapabilities {
    pub supported: BTreeSet<ProviderCapability>,
}

impl ProviderCapabilities {
    #[must_use]
    pub fn supports(&self, capability: ProviderCapability) -> bool {
        self.supported.contains(&capability)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCapability {
    ConversationResume,
    Streaming,
    Activities,
    Approvals,
    Cancellation,
    Usage,
    Compaction,
    PlanMode,
    Attachments,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderDescriptor {
    pub adapter_id: ProviderAdapterId,
    pub display_name: String,
    pub executable: Option<String>,
    pub capabilities: ProviderCapabilities,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderModel {
    pub id: String,
    pub display_name: String,
    pub context_window_tokens: Option<u64>,
    pub supports_reasoning: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    Normal,
    Plan,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderAttachment {
    pub attachment_id: Uuid,
    pub media_type: String,
    pub size_bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StartRequest {
    pub operation_id: Uuid,
    pub bot_id: Uuid,
    pub chat_id: Uuid,
    pub prompt: String,
    pub model: Option<String>,
    pub mode: ExecutionMode,
    pub attachments: Vec<ProviderAttachment>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResumeRequest {
    pub operation_id: Uuid,
    pub conversation_id: String,
    pub prompt: String,
    pub model: Option<String>,
    pub mode: ExecutionMode,
    pub attachments: Vec<ProviderAttachment>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CompactRequest {
    pub conversation_id: String,
    pub target_tokens: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderActivity {
    pub activity_id: Uuid,
    pub kind: ActivityKind,
    pub title: String,
    pub status: ActivityStatus,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityKind {
    Reasoning,
    Terminal,
    Filesystem,
    Browser,
    Tool,
    Search,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityStatus {
    Started,
    Updated,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderApproval {
    pub approval_id: Uuid,
    pub capability: String,
    pub action: String,
    pub resource: String,
    pub reason: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProviderEvent {
    ConversationStarted { conversation_id: String },
    ContentDelta { text: String },
    Activity { activity: ProviderActivity },
    ApprovalRequired { approval: ProviderApproval },
    Usage { usage: ProviderUsage },
    Compacted { conversation_id: String },
    Completed,
    Cancelled,
    Failed { error: ProviderError },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderErrorCode {
    NotInstalled,
    AuthenticationRequired,
    Unavailable,
    InvalidRequest,
    ConversationUnavailable,
    ProtocolViolation,
    ProcessCrashed,
    TimedOut,
    Cancelled,
    Internal,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderError {
    pub code: ProviderErrorCode,
    pub message: String,
    pub retryable: bool,
    pub diagnostic_id: Option<Uuid>,
}

impl ProviderError {
    #[must_use]
    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            code: ProviderErrorCode::Internal,
            message: message.into(),
            retryable: false,
            diagnostic_id: Some(Uuid::now_v7()),
        }
    }
}

pub struct ProviderRun {
    pub operation_id: Uuid,
    pub events: mpsc::Receiver<ProviderEvent>,
}

impl fmt::Debug for ProviderRun {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderRun")
            .field("operation_id", &self.operation_id)
            .field("events", &"bounded receiver")
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderContractError {
    #[error("provider adapter ID must be a lowercase ASCII slug")]
    InvalidAdapterId,
}

#[async_trait::async_trait]
pub trait ProviderAdapter: Send + Sync {
    fn id(&self) -> &ProviderAdapterId;

    async fn discover(&self) -> Result<ProviderDescriptor, ProviderError>;

    async fn health(&self) -> ProviderHealth;

    async fn models(&self) -> Result<Vec<ProviderModel>, ProviderError>;

    async fn start(&self, request: StartRequest) -> Result<ProviderRun, ProviderError>;

    async fn resume(&self, request: ResumeRequest) -> Result<ProviderRun, ProviderError>;

    async fn cancel(&self, operation_id: Uuid) -> Result<(), ProviderError>;

    async fn compact(&self, request: CompactRequest) -> Result<(), ProviderError>;

    async fn recover(&self) -> Result<Vec<ProviderRun>, ProviderError>;
}
