//! Versioned, provider-neutral contracts shared by every `HomeBot` client.

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
    MessageDelta {
        chat_id: Uuid,
        message_id: Uuid,
        delta: String,
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
    pub last_sequence: u64,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
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
    OperationCancelled,
    ResumeUnavailable,
    ProtocolVersionUnsupported,
    Internal,
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
    pub create_attachment_request: CreateAttachmentRequest,
    pub create_attachment_response: CreateAttachmentResponse,
    pub finalize_attachment_request: FinalizeAttachmentRequest,
    pub attachment: Attachment,
    pub create_bot_request: CreateBotRequest,
    pub update_bot_request: UpdateBotRequest,
    pub bot_mutation_request: BotMutationRequest,
    pub bot_response: BotResponse,
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
}
