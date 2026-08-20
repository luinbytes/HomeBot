//! Versioned, provider-neutral contracts shared by every `HomeBot` client.

pub use homebot_routines::{
    ExpectedOutput, MissedRunPolicy, OverlapPolicy, RecordedAction, RecordedActor, RetryPolicy,
    RoutineDefinition, RoutineExecutionResult, RoutineInput, RoutineInputKind, RoutineSchedule,
    RoutineStep, RoutineStepStatus, RoutineTriggerDefinition, RoutineTriggerSource,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

pub const PROTOCOL_VERSION: u16 = 1;
pub const MIN_COMPATIBLE_PROTOCOL_VERSION: u16 = 1;

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProtocolRange {
    pub minimum: u16,
    pub maximum: u16,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClientMessage {
    Hello {
        protocol_version: u16,
        client_version: String,
        device_session: String,
        resume_after: Option<u64>,
    },
    Command {
        request_id: Uuid,
        idempotency_key: Uuid,
        command: Command,
    },
    Cancel {
        request_id: Uuid,
        operation_id: Uuid,
    },
    Pong {
        nonce: Uuid,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    CreateBot { name: String, title: String },
    SendMessage { chat_id: Uuid, content: String },
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
pub struct ServerEvent {
    pub protocol_version: u16,
    pub sequence: u64,
    pub event_id: Uuid,
    #[serde(flatten)]
    pub body: ServerEventBody,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ServerEventBody {
    Hello {
        server_version: String,
        supported_protocols: ProtocolRange,
        resume: ResumeDisposition,
        heartbeat_interval_ms: u32,
        heartbeat_timeout_ms: u32,
    },
    Snapshot {
        boundary_sequence: u64,
        snapshot: Snapshot,
    },
    BotChanged {
        bot: BotSummary,
    },
    ChatChanged {
        chat: ChatSummary,
    },
    GroupChatChanged {
        group: GroupChatSummary,
    },
    GroupParticipantChanged {
        participant: GroupParticipantSummary,
    },
    GroupParticipantRemoved {
        chat_id: Uuid,
        bot_id: Uuid,
    },
    GroupHandoffRecorded {
        handoff: OwnershipHandoffSummary,
    },
    MessageChanged {
        message: MessageSummary,
    },
    MessageDelta {
        chat_id: Uuid,
        message_id: Uuid,
        delta: String,
    },
    ActivityChanged {
        activity: ActivitySummary,
    },
    ApprovalChanged {
        approval: ApprovalSummary,
    },
    QueuedPromptChanged {
        prompt: QueuedPromptSummary,
    },
    SecretChanged {
        secret: SecretSummary,
    },
    SecretRemoved {
        secret_id: Uuid,
    },
    PluginChanged {
        plugin: PluginSummary,
    },
    PluginRemoved {
        plugin_id: Uuid,
    },
    RoutineChanged {
        routine: RoutineSummary,
    },
    RoutineRemoved {
        routine_id: Uuid,
    },
    RoutineRecordingChanged {
        recording: RoutineRecordingSummary,
    },
    RoutineRunChanged {
        run: RoutineRunSummary,
    },
    RoutineTriggerChanged {
        trigger: RoutineTriggerSummary,
    },
    RoutineTriggerRemoved {
        trigger_id: Uuid,
    },
    RoutineJobChanged {
        job: RoutineJobSummary,
    },
    CommandAccepted {
        request_id: Uuid,
        operation_id: Uuid,
    },
    CommandCompleted {
        request_id: Uuid,
        operation_id: Uuid,
        result: Value,
    },
    CommandFailed {
        request_id: Uuid,
        operation_id: Uuid,
        error: ErrorEnvelope,
    },
    CommandCancelled {
        request_id: Uuid,
        operation_id: Uuid,
    },
    Ping {
        nonce: Uuid,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResumeDisposition {
    Replayed,
    SnapshotRequired,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub bots: Vec<BotSummary>,
    pub chats: Vec<ChatSummary>,
    #[serde(default)]
    pub group_chats: Vec<GroupChatSummary>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BotSummary {
    pub id: Uuid,
    pub name: String,
    pub title: String,
    pub description: String,
    pub shape: BotShape,
    pub color: BotColor,
    pub archived: bool,
    pub unread_count: u32,
    pub attention: BotAttention,
    pub provider: BotProviderStatus,
    pub advanced: BotAdvancedSettings,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BotShape {
    Circle,
    RoundedSquare,
    Hexagon,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BotColor {
    Violet,
    Blue,
    Green,
    Orange,
    Rose,
    Slate,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BotAttention {
    None,
    Working,
    NeedsApproval,
    Failed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BotProviderStatus {
    NotConfigured,
    Ready,
    Unavailable,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BotAdvancedSettings {
    pub provider_profile_id: Option<Uuid>,
    pub permission_profile: BotPermissionProfile,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BotPermissionProfile {
    ReadOnly,
    AskBeforeChanges,
    Trusted,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateBotRequest {
    pub request_id: Uuid,
    pub idempotency_key: Uuid,
    pub name: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub shape: BotShape,
    pub color: BotColor,
    pub provider_profile_id: Option<Uuid>,
    pub permission_profile: BotPermissionProfile,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateBotRequest {
    pub request_id: Uuid,
    pub idempotency_key: Uuid,
    pub name: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub shape: BotShape,
    pub color: BotColor,
    pub provider_profile_id: Option<Uuid>,
    pub permission_profile: BotPermissionProfile,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BotMutationRequest {
    pub request_id: Uuid,
    pub idempotency_key: Uuid,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BotResponse {
    pub bot: BotSummary,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChatSummary {
    pub id: Uuid,
    pub title: String,
    pub bot_id: Uuid,
    pub unread_count: u32,
    pub running: bool,
    pub queued_count: u32,
    pub last_sequence: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GroupChatSummary {
    pub id: Uuid,
    pub title: String,
    pub ownership_bot_id: Uuid,
    pub coordination_max_turns: u32,
    pub coordination_turns_used: u32,
    pub max_parallel_bots: u32,
    pub stop_requested: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupParticipantRole {
    Owner,
    Member,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupBotStatus {
    Idle,
    Running,
    Waiting,
    Completed,
    Failed,
    Stopped,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GroupParticipantSummary {
    pub chat_id: Uuid,
    pub bot_id: Uuid,
    pub role: GroupParticipantRole,
    pub status: GroupBotStatus,
    pub active_operation_id: Option<Uuid>,
    pub updated_at_ms: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OwnershipHandoffSummary {
    pub id: Uuid,
    pub chat_id: Uuid,
    pub from_bot_id: Uuid,
    pub to_bot_id: Uuid,
    pub message_id: Option<Uuid>,
    pub reason: String,
    pub created_at_ms: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateGroupChatRequest {
    pub request_id: Uuid,
    pub idempotency_key: Uuid,
    pub title: String,
    pub bot_ids: Vec<Uuid>,
    pub ownership_bot_id: Uuid,
    pub coordination_max_turns: u32,
    pub max_parallel_bots: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateGroupChatResponse {
    pub group: GroupChatSummary,
    pub participants: Vec<GroupParticipantSummary>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GroupTimelineResponse {
    pub group: GroupChatSummary,
    pub participants: Vec<GroupParticipantSummary>,
    pub messages: Vec<MessageSummary>,
    pub handoffs: Vec<OwnershipHandoffSummary>,
    pub boundary_sequence: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SendGroupMessageRequest {
    pub request_id: Uuid,
    pub idempotency_key: Uuid,
    pub content: String,
    pub mentioned_bot_ids: Vec<Uuid>,
    pub shared_context_message_ids: Vec<Uuid>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HandoffGroupRequest {
    pub request_id: Uuid,
    pub idempotency_key: Uuid,
    pub from_bot_id: Uuid,
    pub to_bot_id: Uuid,
    pub message_id: Option<Uuid>,
    pub reason: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateGroupParticipantRequest {
    pub request_id: Uuid,
    pub idempotency_key: Uuid,
    pub status: GroupBotStatus,
    pub operation_id: Option<Uuid>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AddGroupParticipantRequest {
    pub request_id: Uuid,
    pub idempotency_key: Uuid,
    pub bot_id: Uuid,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageAuthor {
    User,
    Bot,
    System,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageStatus {
    Queued,
    Streaming,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MessagePart {
    Text {
        id: Uuid,
        ordinal: u32,
        text: String,
    },
    Attachment {
        id: Uuid,
        ordinal: u32,
        attachment: Attachment,
    },
    Notice {
        id: Uuid,
        ordinal: u32,
        text: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MessageSummary {
    pub id: Uuid,
    pub chat_id: Uuid,
    pub author: MessageAuthor,
    pub author_bot_id: Option<Uuid>,
    pub status: MessageStatus,
    pub parts: Vec<MessagePart>,
    pub reply_to_message_id: Option<Uuid>,
    pub mentioned_bot_ids: Vec<Uuid>,
    pub shared_context_message_ids: Vec<Uuid>,
    pub created_at_ms: i64,
    pub completed_at_ms: Option<i64>,
    pub error: Option<ErrorEnvelope>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityKind {
    Reasoning,
    Search,
    Tool,
    Filesystem,
    Terminal,
    Browser,
    Artifact,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    Low,
    Elevated,
    High,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ActivityDetail {
    Generic {
        summary: String,
    },
    File {
        action: String,
        workspace_path: String,
        bytes_changed: Option<u64>,
        sha256: Option<String>,
    },
    Terminal {
        command: String,
        working_directory: String,
        output_preview: String,
        exit_code: Option<i32>,
        truncated: bool,
    },
    Browser {
        action: String,
        url: String,
        page_title: Option<String>,
        screenshot_artifact_id: Option<Uuid>,
    },
    Artifact {
        artifact_id: Uuid,
        name: String,
        media_type: String,
        size_bytes: u64,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActivityPresentation {
    pub risk: RiskLevel,
    pub detail: ActivityDetail,
    pub copy_text: Option<String>,
    pub open_artifact_id: Option<Uuid>,
}

impl ActivityPresentation {
    /// Returns whether every client-visible path is a normalized workspace-relative path.
    #[must_use]
    pub fn is_remote_safe(&self) -> bool {
        match &self.detail {
            ActivityDetail::File { workspace_path, .. } => {
                normalized_workspace_path(workspace_path)
            }
            ActivityDetail::Terminal {
                working_directory, ..
            } => normalized_workspace_path(working_directory),
            ActivityDetail::Generic { .. }
            | ActivityDetail::Browser { .. }
            | ActivityDetail::Artifact { .. } => true,
        }
    }
}

fn normalized_workspace_path(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('/')
        && !value.contains(['\\', '\0'])
        && value
            .split('/')
            .all(|component| !component.is_empty() && component != "." && component != "..")
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActivitySummary {
    pub id: Uuid,
    pub chat_id: Uuid,
    pub message_id: Option<Uuid>,
    pub title: String,
    pub detail: String,
    pub kind: ActivityKind,
    pub presentation: ActivityPresentation,
    pub status: ActivityStatus,
    pub requires_attention: bool,
    pub started_at_ms: i64,
    pub finished_at_ms: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactSummary {
    pub id: Uuid,
    pub chat_id: Uuid,
    pub message_id: Option<Uuid>,
    pub activity_id: Option<Uuid>,
    pub name: String,
    pub kind: String,
    pub media_type: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub created_at_ms: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    Pending,
    Allowed,
    Denied,
    Expired,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalSummary {
    pub id: Uuid,
    pub chat_id: Uuid,
    pub message_id: Option<Uuid>,
    pub title: String,
    pub detail: String,
    pub status: ApprovalStatus,
    pub created_at_ms: i64,
    pub decided_at_ms: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalDecisionRequest {
    pub request_id: Uuid,
    pub idempotency_key: Uuid,
    pub allow: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct QueuedPromptSummary {
    pub id: Uuid,
    pub chat_id: Uuid,
    pub content: String,
    pub attachment_ids: Vec<Uuid>,
    pub position: u32,
    pub created_at_ms: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChatTimelineResponse {
    pub chat: ChatSummary,
    pub messages: Vec<MessageSummary>,
    pub activities: Vec<ActivitySummary>,
    pub approvals: Vec<ApprovalSummary>,
    pub queued_prompts: Vec<QueuedPromptSummary>,
    pub boundary_sequence: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateDirectChatRequest {
    pub request_id: Uuid,
    pub idempotency_key: Uuid,
    pub bot_id: Uuid,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateDirectChatResponse {
    pub chat: ChatSummary,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SendMessageRequest {
    pub request_id: Uuid,
    pub idempotency_key: Uuid,
    pub content: String,
    pub attachment_ids: Vec<Uuid>,
    pub reply_to_message_id: Option<Uuid>,
    pub mentioned_bot_ids: Vec<Uuid>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SendMessageResponse {
    Sent { message: MessageSummary },
    Queued { prompt: QueuedPromptSummary },
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MessageMutationRequest {
    pub request_id: Uuid,
    pub idempotency_key: Uuid,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorEnvelope {
    pub code: ErrorCode,
    pub message: String,
    pub retryable: bool,
    pub request_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Unauthenticated,
    Forbidden,
    ApprovalRequired,
    NotFound,
    Conflict,
    ValidationFailed,
    RateLimited,
    ProviderUnavailable,
    PluginUnavailable,
    SecretStoreLocked,
    SecretStoreUnavailable,
    OperationCancelled,
    ResumeUnavailable,
    ProtocolVersionUnsupported,
    Internal,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretStatus {
    Ready,
    Locked,
    Unavailable,
    Missing,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SecretSummary {
    pub id: Uuid,
    pub label: String,
    pub status: SecretStatus,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
}

/// Secret-bearing request. Deliberately does not implement `Debug` or `Serialize`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateSecretRequest {
    pub request_id: Uuid,
    pub idempotency_key: Uuid,
    pub label: String,
    pub value: String,
}

/// Secret-bearing request. Deliberately does not implement `Debug` or `Serialize`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateSecretRequest {
    pub request_id: Uuid,
    pub label: Option<String>,
    pub value: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginConnectionState {
    Connect,
    Waiting,
    Reopen,
    Connected,
    Error,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginAuthState {
    NotRequired,
    Required,
    Waiting,
    Connected,
    Error,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PluginToolSummary {
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub input_schema: Value,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PluginSummary {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub kind: String,
    pub enabled: bool,
    pub connection_state: PluginConnectionState,
    pub auth_state: PluginAuthState,
    pub error_message: Option<String>,
    pub tools: Vec<PluginToolSummary>,
    pub bot_ids: Vec<Uuid>,
    pub updated_at_unix_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateLocalMcpPluginRequest {
    pub request_id: Uuid,
    pub idempotency_key: Uuid,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub program: String,
    #[serde(default)]
    pub arguments: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PluginMutationRequest {
    pub request_id: Uuid,
    pub idempotency_key: Uuid,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PluginAssignmentRequest {
    pub request_id: Uuid,
    pub idempotency_key: Uuid,
    pub bot_id: Uuid,
    pub enabled: bool,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoutineSummary {
    pub id: Uuid,
    pub bot_id: Uuid,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub draft: bool,
    pub active_version_id: Uuid,
    pub version: u32,
    pub definition: RoutineDefinition,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateRoutineRequest {
    pub request_id: Uuid,
    pub idempotency_key: Uuid,
    pub bot_id: Uuid,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub definition: RoutineDefinition,
    #[serde(default = "default_true")]
    pub draft: bool,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateRoutineRequest {
    pub request_id: Uuid,
    pub idempotency_key: Uuid,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub definition: RoutineDefinition,
    pub draft: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DuplicateRoutineRequest {
    pub request_id: Uuid,
    pub idempotency_key: Uuid,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StartRoutineRecordingRequest {
    pub request_id: Uuid,
    pub idempotency_key: Uuid,
    pub bot_id: Uuid,
    pub name: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AppendRoutineRecordingRequest {
    pub request_id: Uuid,
    pub idempotency_key: Uuid,
    pub action: RecordedAction,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunRoutineRequest {
    pub request_id: Uuid,
    pub idempotency_key: Uuid,
    #[serde(default = "empty_object")]
    pub inputs: Value,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoutineRecordingSummary {
    pub id: Uuid,
    pub bot_id: Uuid,
    pub name: String,
    pub description: String,
    pub actions: Vec<RecordedAction>,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoutineRunSummary {
    pub id: Uuid,
    pub routine_id: Uuid,
    pub routine_version_id: Uuid,
    pub bot_id: Uuid,
    pub status: String,
    pub trigger: Value,
    pub input_metadata: Value,
    pub dry_run: bool,
    pub results: Vec<RoutineExecutionResult>,
    pub error_message: Option<String>,
    pub attempt_count: u16,
    pub scheduled_for_unix_ms: Option<u64>,
    pub started_at_unix_ms: u64,
    pub finished_at_unix_ms: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoutineTriggerSummary {
    pub id: Uuid,
    pub routine_id: Uuid,
    pub definition: RoutineTriggerDefinition,
    pub enabled: bool,
    pub last_evaluated_at_unix_ms: Option<u64>,
    pub next_fire_at_unix_ms: Option<u64>,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateRoutineTriggerRequest {
    pub request_id: Uuid,
    pub idempotency_key: Uuid,
    pub definition: RoutineTriggerDefinition,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeliverRoutineTriggerRequest {
    pub request_id: Uuid,
    pub idempotency_key: Uuid,
    pub delivery_key: String,
    #[serde(default = "empty_object")]
    pub inputs: Value,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoutineJobSummary {
    pub id: Uuid,
    pub trigger_id: Uuid,
    pub routine_id: Uuid,
    pub routine_version_id: Uuid,
    pub delivery_key: String,
    pub trigger: Value,
    pub input_metadata: Value,
    pub status: String,
    pub attempt_count: u16,
    pub scheduled_for_unix_ms: u64,
    pub next_attempt_at_unix_ms: u64,
    pub cancel_requested: bool,
    pub error_message: Option<String>,
    pub created_at_unix_ms: u64,
    pub started_at_unix_ms: Option<u64>,
    pub finished_at_unix_ms: Option<u64>,
}

const fn default_true() -> bool {
    true
}
fn empty_object() -> Value {
    Value::Object(serde_json::Map::new())
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateAttachmentRequest {
    pub request_id: Uuid,
    pub idempotency_key: Uuid,
    pub filename: String,
    pub media_type: String,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateAttachmentResponse {
    pub attachment_id: Uuid,
    pub upload_url: String,
    pub expires_at_unix_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FinalizeAttachmentRequest {
    pub request_id: Uuid,
    pub idempotency_key: Uuid,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Attachment {
    pub id: Uuid,
    pub filename: String,
    pub media_type: String,
    pub size_bytes: u64,
    pub sha256: String,
}

/// Root exported to the committed machine-readable JSON Schema.
#[derive(JsonSchema)]
pub struct ProtocolV1Schema {
    pub client_message: ClientMessage,
    pub server_event: ServerEvent,
    pub error: ErrorEnvelope,
    pub create_secret_request: CreateSecretRequest,
    pub update_secret_request: UpdateSecretRequest,
    pub secret: SecretSummary,
    pub create_local_mcp_plugin_request: CreateLocalMcpPluginRequest,
    pub plugin_mutation_request: PluginMutationRequest,
    pub plugin_assignment_request: PluginAssignmentRequest,
    pub plugin: PluginSummary,
    pub create_routine_request: CreateRoutineRequest,
    pub update_routine_request: UpdateRoutineRequest,
    pub duplicate_routine_request: DuplicateRoutineRequest,
    pub start_routine_recording_request: StartRoutineRecordingRequest,
    pub append_routine_recording_request: AppendRoutineRecordingRequest,
    pub run_routine_request: RunRoutineRequest,
    pub create_routine_trigger_request: CreateRoutineTriggerRequest,
    pub deliver_routine_trigger_request: DeliverRoutineTriggerRequest,
    pub routine: RoutineSummary,
    pub routine_recording: RoutineRecordingSummary,
    pub routine_run: RoutineRunSummary,
    pub routine_trigger: RoutineTriggerSummary,
    pub routine_job: RoutineJobSummary,
    pub create_attachment_request: CreateAttachmentRequest,
    pub create_attachment_response: CreateAttachmentResponse,
    pub finalize_attachment_request: FinalizeAttachmentRequest,
    pub attachment: Attachment,
    pub artifact: ArtifactSummary,
    pub create_bot_request: CreateBotRequest,
    pub update_bot_request: UpdateBotRequest,
    pub bot_mutation_request: BotMutationRequest,
    pub bot_response: BotResponse,
    pub create_direct_chat_request: CreateDirectChatRequest,
    pub create_direct_chat_response: CreateDirectChatResponse,
    pub send_message_request: SendMessageRequest,
    pub send_message_response: SendMessageResponse,
    pub message_mutation_request: MessageMutationRequest,
    pub chat_timeline_response: ChatTimelineResponse,
    pub approval_decision_request: ApprovalDecisionRequest,
}

/// Checks whether a client protocol is in the supported inclusive range.
///
/// # Errors
///
/// Returns a safe [`ErrorEnvelope`] when the version is unsupported.
pub fn check_compatibility(client_protocol: u16) -> Result<(), ErrorEnvelope> {
    if (MIN_COMPATIBLE_PROTOCOL_VERSION..=PROTOCOL_VERSION).contains(&client_protocol) {
        return Ok(());
    }
    Err(ErrorEnvelope {
        code: ErrorCode::ProtocolVersionUnsupported,
        message: format!(
            "client protocol {client_protocol} is incompatible with server range {MIN_COMPATIBLE_PROTOCOL_VERSION}..={PROTOCOL_VERSION}"
        ),
        retryable: false,
        request_id: None,
        retry_after_ms: None,
        details: None,
    })
}

#[must_use]
pub fn classify_sequence(previous: u64, received: u64) -> SequenceDisposition {
    if received <= previous {
        SequenceDisposition::Duplicate
    } else if received == previous.saturating_add(1) {
        SequenceDisposition::Next
    } else {
        SequenceDisposition::Gap
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SequenceDisposition {
    Duplicate,
    Next,
    Gap,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_compatibility_is_closed_and_explicit() {
        assert!(check_compatibility(PROTOCOL_VERSION).is_ok());
        assert_eq!(
            check_compatibility(0).map_err(|error| error.code),
            Err(ErrorCode::ProtocolVersionUnsupported)
        );
        assert_eq!(
            check_compatibility(PROTOCOL_VERSION + 1).map_err(|error| error.code),
            Err(ErrorCode::ProtocolVersionUnsupported)
        );
    }

    #[test]
    fn sequence_classification_detects_replays_and_gaps() {
        assert_eq!(classify_sequence(41, 41), SequenceDisposition::Duplicate);
        assert_eq!(classify_sequence(41, 42), SequenceDisposition::Next);
        assert_eq!(classify_sequence(41, 43), SequenceDisposition::Gap);
    }

    #[test]
    fn snapshot_matches_v1_golden_fixture() {
        assert!(
            serde_json::from_str::<ServerEvent>(include_str!(
                "../../../tests/fixtures/protocol/server-snapshot-v1.json"
            ))
            .is_ok()
        );
    }

    #[test]
    fn command_fixture_requires_idempotency_key() {
        let fixture = include_str!("../../../tests/fixtures/protocol/client-command-v1.json");
        assert!(serde_json::from_str::<ClientMessage>(fixture).is_ok());
        let missing = fixture.replace(
            ",\n  \"idempotency_key\": \"00000000-0000-0000-0000-000000000002\"",
            "",
        );
        assert!(serde_json::from_str::<ClientMessage>(&missing).is_err());
    }

    #[test]
    fn terminal_lifecycle_fixtures_are_distinct_and_valid() {
        for fixture in [
            include_str!("../../../tests/fixtures/protocol/command-completed-v1.json"),
            include_str!("../../../tests/fixtures/protocol/command-failed-v1.json"),
            include_str!("../../../tests/fixtures/protocol/command-cancelled-v1.json"),
        ] {
            assert!(serde_json::from_str::<ServerEvent>(fixture).is_ok());
        }
    }

    #[test]
    fn activity_paths_must_be_normalized_and_workspace_relative() {
        let presentation = |workspace_path: &str| ActivityPresentation {
            risk: RiskLevel::Low,
            detail: ActivityDetail::File {
                action: "read".to_owned(),
                workspace_path: workspace_path.to_owned(),
                bytes_changed: None,
                sha256: None,
            },
            copy_text: None,
            open_artifact_id: None,
        };
        assert!(presentation("docs/protocol.md").is_remote_safe());
        for unsafe_path in ["/etc/passwd", "../secret", "docs//file", "C:\\secret"] {
            assert!(!presentation(unsafe_path).is_remote_safe(), "{unsafe_path}");
        }
    }
}
