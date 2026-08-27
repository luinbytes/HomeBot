//! Normalization for Responses and Chat Completions streaming events.

use super::OpenAiApiStyle;
use crate::{
    ActivityKind, ActivityStatus, ProviderActivity, ProviderError, ProviderErrorCode,
    ProviderEvent, ProviderToolCall, ProviderUsage,
};
use serde_json::Value;
use uuid::Uuid;

pub(super) fn normalize_event(style: OpenAiApiStyle, value: &Value) -> Vec<ProviderEvent> {
    match style {
        OpenAiApiStyle::Responses => normalize_responses(value),
        OpenAiApiStyle::ChatCompletions => normalize_chat(value),
    }
}

fn normalize_responses(value: &Value) -> Vec<ProviderEvent> {
    match value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "response.created" => string(value, &["response", "id"])
            .map(|conversation_id| vec![ProviderEvent::ConversationStarted { conversation_id }])
            .unwrap_or_default(),
        "response.output_text.delta" => string(value, &["delta"])
            .map(|text| vec![ProviderEvent::ContentDelta { text }])
            .unwrap_or_default(),
        "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
            vec![activity(
                ActivityKind::Reasoning,
                string(value, &["delta"]).unwrap_or_else(|| "Reasoning".to_owned()),
                ActivityStatus::Updated,
            )]
        }
        "response.function_call_arguments.delta" => vec![activity(
            ActivityKind::Tool,
            "Function call".to_owned(),
            ActivityStatus::Updated,
        )],
        "response.output_item.done"
            if string(value, &["item", "type"]).as_deref() == Some("function_call") =>
        {
            let call_id = string(value, &["item", "call_id"]);
            let name = string(value, &["item", "name"]);
            let arguments = string(value, &["item", "arguments"])
                .and_then(|arguments| serde_json::from_str(&arguments).ok());
            match (call_id, name, arguments) {
                (Some(call_id), Some(name), Some(arguments)) => {
                    vec![ProviderEvent::ToolCall {
                        call: ProviderToolCall {
                            call_id,
                            name,
                            arguments,
                        },
                    }]
                }
                _ => vec![ProviderEvent::Failed {
                    error: protocol_error("Provider function call was malformed"),
                }],
            }
        }
        "response.completed" => {
            let mut events = usage(value);
            events.push(ProviderEvent::Completed);
            events
        }
        "response.failed" | "response.incomplete" | "error" => vec![ProviderEvent::Failed {
            error: response_error(value),
        }],
        _ => Vec::new(),
    }
}

fn protocol_error(message: &str) -> ProviderError {
    ProviderError {
        code: ProviderErrorCode::ProtocolViolation,
        message: message.to_owned(),
        retryable: false,
        diagnostic_id: Some(Uuid::now_v7()),
    }
}

fn normalize_chat(value: &Value) -> Vec<ProviderEvent> {
    let mut events = Vec::new();
    if let Some(id) = value.get("id").and_then(Value::as_str) {
        events.push(ProviderEvent::ConversationStarted {
            conversation_id: id.to_owned(),
        });
    }
    if let Some(text) = string(value, &["choices", "0", "delta", "content"]) {
        events.push(ProviderEvent::ContentDelta { text });
    }
    if value.get("usage").is_some() {
        events.extend(usage(value));
    }
    if value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("finish_reason"))
        .is_some_and(|reason| !reason.is_null())
    {
        events.push(ProviderEvent::Completed);
    }
    events
}

fn usage(value: &Value) -> Vec<ProviderEvent> {
    let usage = value
        .get("response")
        .and_then(|response| response.get("usage"))
        .or_else(|| value.get("usage"));
    usage.map_or_else(Vec::new, |usage| {
        vec![ProviderEvent::Usage {
            usage: ProviderUsage {
                input_tokens: number(usage, &["input_tokens"]).unwrap_or(0),
                output_tokens: number(usage, &["output_tokens"]).unwrap_or(0),
                cached_input_tokens: number(usage, &["input_tokens_details", "cached_tokens"])
                    .unwrap_or(0),
            },
        }]
    })
}

fn response_error(value: &Value) -> ProviderError {
    let error = value
        .get("response")
        .and_then(|response| response.get("error"))
        .or_else(|| value.get("error"))
        .unwrap_or(value);
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("Provider response failed")
        .to_owned();
    ProviderError {
        code: ProviderErrorCode::Unavailable,
        message,
        retryable: true,
        diagnostic_id: Some(Uuid::now_v7()),
    }
}

fn activity(kind: ActivityKind, title: String, status: ActivityStatus) -> ProviderEvent {
    ProviderEvent::Activity {
        activity: ProviderActivity {
            activity_id: Uuid::now_v7(),
            kind,
            title,
            status,
        },
    }
}

fn string(value: &Value, path: &[&str]) -> Option<String> {
    let mut current = value;
    for key in path {
        current = if let Ok(index) = key.parse::<usize>() {
            current.as_array()?.get(index)?
        } else {
            current.get(*key)?
        };
    }
    current.as_str().map(ToOwned::to_owned)
}

fn number(value: &Value, path: &[&str]) -> Option<u64> {
    path.iter()
        .try_fold(value, |current, key| current.get(*key))?
        .as_u64()
}
