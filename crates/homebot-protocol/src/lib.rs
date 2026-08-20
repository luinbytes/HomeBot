//! Versioned, provider-neutral contracts shared by every HomeBot client.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const PROTOCOL_VERSION: u16 = 1;
pub const MIN_COMPATIBLE_PROTOCOL_VERSION: u16 = 1;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClientMessage {
    Hello {
        protocol_version: u16,
        client_version: String,
        resume_after: Option<u64>,
    },
    Command {
        request_id: Uuid,
        idempotency_key: Uuid,
        command: Command,
    },
    Cancel { operation_id: Uuid },
    Pong { nonce: Uuid },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    CreateBot { name: String, title: String },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ServerEvent {
    pub protocol_version: u16,
    pub sequence: u64,
    pub event_id: Uuid,
    #[serde(flatten)]
    pub body: ServerEventBody,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ServerEventBody {
    Hello { server_version: String, resume_accepted: bool },
    Snapshot { snapshot_version: u64 },
    CommandAccepted { request_id: Uuid, operation_id: Uuid },
    CommandCompleted { request_id: Uuid, operation_id: Uuid },
    CommandFailed { request_id: Uuid, operation_id: Uuid, error: ErrorEnvelope },
    Ping { nonce: Uuid },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ErrorEnvelope {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    pub request_id: Option<Uuid>,
}

/// Verifies that a client version is inside the server compatibility window.
///
/// # Errors
///
/// Returns a safe, structured error when the client protocol is too old or too new.
pub fn check_compatibility(client_protocol: u16) -> Result<(), ErrorEnvelope> {
    if (MIN_COMPATIBLE_PROTOCOL_VERSION..=PROTOCOL_VERSION).contains(&client_protocol) {
        return Ok(());
    }

    Err(ErrorEnvelope {
        code: "protocol_version_unsupported".to_owned(),
        message: format!(
            "client protocol {client_protocol} is incompatible with server range {MIN_COMPATIBLE_PROTOCOL_VERSION}..={PROTOCOL_VERSION}"
        ),
        retryable: false,
        request_id: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_protocol_versions_outside_compatibility_window() {
        let result = check_compatibility(0);
        assert!(matches!(
            result,
            Err(ErrorEnvelope { ref code, .. }) if code == "protocol_version_unsupported"
        ));
    }

    #[test]
    fn event_shape_is_stably_tagged() {
        let event = ServerEvent {
            protocol_version: 1,
            sequence: 42,
            event_id: Uuid::nil(),
            body: ServerEventBody::Snapshot { snapshot_version: 7 },
        };
        let value = serde_json::to_value(event);
        assert!(matches!(
            value,
            Ok(ref json) if json["kind"] == "snapshot" && json["sequence"] == 42
        ));
    }

    #[test]
    fn snapshot_matches_v1_golden_fixture() {
        let event = ServerEvent {
            protocol_version: 1,
            sequence: 42,
            event_id: Uuid::nil(),
            body: ServerEventBody::Snapshot { snapshot_version: 7 },
        };
        let actual = serde_json::to_value(event);
        let expected = serde_json::from_str::<serde_json::Value>(include_str!(
            "../../../tests/fixtures/protocol/server-snapshot-v1.json"
        ));

        assert!(matches!((actual, expected), (Ok(actual), Ok(expected)) if actual == expected));
    }
}
