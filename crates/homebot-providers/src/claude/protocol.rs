//! Claude Code `stream-json` normalization.

use crate::{
    ActivityKind, ActivityStatus, ProviderActivity, ProviderError, ProviderErrorCode,
    ProviderEvent, ProviderUsage,
};
use serde_json::Value;
use uuid::Uuid;

pub(super) fn normalize_message(
    message: &Value,
    expected_conversation_id: Option<&str>,
) -> Vec<ProviderEvent> {
    let message_type = message
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match message_type {
        "system" if message.get("subtype").and_then(Value::as_str) == Some("init") => {
            conversation_event(message, expected_conversation_id)
        }
        "system" if message.get("subtype").and_then(Value::as_str) == Some("api_retry") => {
            vec![activity(
                ActivityKind::Tool,
                "Claude API retry",
                ActivityStatus::Updated,
            )]
        }
        "stream_event" => normalize_stream_event(message),
        "assistant" | "user" => normalize_blocks(message),
        "result" => normalize_result(message),
        _ => Vec::new(),
    }
}

fn conversation_event(message: &Value, expected: Option<&str>) -> Vec<ProviderEvent> {
    let session_id = message
        .get("session_id")
        .and_then(Value::as_str)
        .or(expected);
    session_id.map_or_else(Vec::new, |conversation_id| {
        vec![ProviderEvent::ConversationStarted {
            conversation_id: conversation_id.to_owned(),
        }]
    })
}

fn normalize_stream_event(message: &Value) -> Vec<ProviderEvent> {
    let event = message.get("event").unwrap_or(message);
    let event_type = event
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let delta = event.get("delta").unwrap_or(event);
    match (event_type, delta.get("type").and_then(Value::as_str)) {
        ("content_block_delta", Some("text_delta")) => delta
            .get("text")
            .and_then(Value::as_str)
            .map(|text| {
                vec![ProviderEvent::ContentDelta {
                    text: text.to_owned(),
                }]
            })
            .unwrap_or_default(),
        ("content_block_delta", Some("thinking_delta")) => vec![activity(
            ActivityKind::Reasoning,
            delta
                .get("thinking")
                .and_then(Value::as_str)
                .unwrap_or("Reasoning"),
            ActivityStatus::Updated,
        )],
        _ => Vec::new(),
    }
}

fn normalize_blocks(message: &Value) -> Vec<ProviderEvent> {
    let blocks = message
        .get("message")
        .and_then(|value| value.get("content"))
        .and_then(Value::as_array)
        .or_else(|| message.get("content").and_then(Value::as_array));
    let Some(blocks) = blocks else {
        return Vec::new();
    };
    blocks
        .iter()
        .filter_map(|block| match block.get("type").and_then(Value::as_str) {
            Some("tool_use") => Some(activity(
                classify_tool(
                    block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                ),
                block
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("Claude tool"),
                ActivityStatus::Started,
            )),
            Some("tool_result") => Some(activity(
                ActivityKind::Tool,
                "Claude tool result",
                if block.get("is_error").and_then(Value::as_bool) == Some(true) {
                    ActivityStatus::Failed
                } else {
                    ActivityStatus::Completed
                },
            )),
            _ => None,
        })
        .collect()
}

fn normalize_result(message: &Value) -> Vec<ProviderEvent> {
    let mut events = Vec::new();
    if let Some(usage) = message.get("usage") {
        events.push(ProviderEvent::Usage {
            usage: ProviderUsage {
                input_tokens: number(usage, &["input_tokens"]).unwrap_or(0),
                output_tokens: number(usage, &["output_tokens"]).unwrap_or(0),
                cached_input_tokens: number(usage, &["cache_read_input_tokens"]).unwrap_or(0),
            },
        });
    }
    if message.get("subtype").and_then(Value::as_str) == Some("success")
        || message.get("is_error").and_then(Value::as_bool) == Some(false)
    {
        events.push(ProviderEvent::Completed);
    } else {
        events.push(ProviderEvent::Failed {
            error: normalize_error(message),
        });
    }
    events
}

fn normalize_error(message: &Value) -> ProviderError {
    let subtype = message
        .get("subtype")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let text = message
        .get("error")
        .or_else(|| message.get("result"))
        .and_then(Value::as_str)
        .unwrap_or("Claude Code reported a provider failure")
        .to_owned();
    let code = if subtype.contains("auth") || text.to_ascii_lowercase().contains("auth") {
        ProviderErrorCode::AuthenticationRequired
    } else if subtype.contains("invalid") {
        ProviderErrorCode::InvalidRequest
    } else {
        ProviderErrorCode::Unavailable
    };
    ProviderError {
        code,
        message: text,
        retryable: matches!(code, ProviderErrorCode::Unavailable),
        diagnostic_id: Some(Uuid::now_v7()),
    }
}

fn activity(kind: ActivityKind, title: &str, status: ActivityStatus) -> ProviderEvent {
    ProviderEvent::Activity {
        activity: ProviderActivity {
            activity_id: Uuid::now_v7(),
            kind,
            title: title.to_owned(),
            status,
        },
    }
}

fn classify_tool(name: &str) -> ActivityKind {
    match name {
        "Bash" => ActivityKind::Terminal,
        "Read" | "Write" | "Edit" | "Glob" | "Grep" => ActivityKind::Filesystem,
        "WebFetch" => ActivityKind::Browser,
        "WebSearch" => ActivityKind::Search,
        _ => ActivityKind::Tool,
    }
}

fn number(value: &Value, path: &[&str]) -> Option<u64> {
    path.iter()
        .try_fold(value, |current, key| current.get(*key))?
        .as_u64()
}
