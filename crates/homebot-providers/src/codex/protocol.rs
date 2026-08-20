//! Mapping from Codex App Server wire values into `HomeBot` provider events.

use super::{display_json, string_at};
use crate::{
    ActivityKind, ActivityStatus, ProviderActivity, ProviderError, ProviderErrorCode,
    ProviderEvent, ProviderUsage,
};
use serde_json::Value;
use uuid::Uuid;

pub(super) fn notification_events(
    method: &str,
    params: &Value,
    activity_id: Option<Uuid>,
    last_error: Option<ProviderError>,
) -> Vec<ProviderEvent> {
    match method {
        "item/agentMessage/delta" => string_at(params, &["delta"])
            .map(|text| vec![ProviderEvent::ContentDelta { text }])
            .unwrap_or_default(),
        "item/plan/delta" | "item/reasoning/summaryTextDelta" | "item/reasoning/textDelta" => {
            activity_id.map_or_else(Vec::new, |activity_id| {
                vec![ProviderEvent::Activity {
                    activity: ProviderActivity {
                        activity_id,
                        kind: ActivityKind::Reasoning,
                        title: string_at(params, &["delta"])
                            .unwrap_or_else(|| "Reasoning".to_owned()),
                        status: ActivityStatus::Updated,
                    },
                }]
            })
        }
        "item/commandExecution/outputDelta" => activity_id.map_or_else(Vec::new, |activity_id| {
            vec![ProviderEvent::Activity {
                activity: ProviderActivity {
                    activity_id,
                    kind: ActivityKind::Terminal,
                    title: string_at(params, &["delta"])
                        .unwrap_or_else(|| "Command output".to_owned()),
                    status: ActivityStatus::Updated,
                },
            }]
        }),
        "item/started" | "item/completed" => {
            normalize_item(method == "item/completed", params, activity_id)
        }
        "thread/tokenUsage/updated" => normalize_usage(params)
            .map(|usage| vec![ProviderEvent::Usage { usage }])
            .unwrap_or_default(),
        "turn/completed" => {
            let status = string_at(params, &["turn", "status"]).unwrap_or_default();
            vec![match status.as_str() {
                "completed" => ProviderEvent::Completed,
                "interrupted" => ProviderEvent::Cancelled,
                _ => ProviderEvent::Failed {
                    error: last_error.unwrap_or_else(|| normalize_codex_error(params)),
                },
            }]
        }
        _ => Vec::new(),
    }
}

fn normalize_item(
    completed: bool,
    params: &Value,
    activity_id: Option<Uuid>,
) -> Vec<ProviderEvent> {
    let Some(activity_id) = activity_id else {
        return Vec::new();
    };
    let item = params.get("item").unwrap_or(params);
    let item_type = item.get("type").and_then(Value::as_str).unwrap_or("tool");
    if item_type == "agentMessage" || item_type == "userMessage" {
        return Vec::new();
    }
    if item_type == "contextCompaction" && completed {
        if let Some(conversation_id) = string_at(params, &["threadId"]) {
            return vec![ProviderEvent::Compacted { conversation_id }];
        }
    }
    let kind = match item_type {
        "commandExecution" => ActivityKind::Terminal,
        "fileChange" | "imageView" => ActivityKind::Filesystem,
        "webSearch" => ActivityKind::Search,
        "reasoning" | "plan" => ActivityKind::Reasoning,
        _ => ActivityKind::Tool,
    };
    let status = if completed {
        match item.get("status").and_then(Value::as_str) {
            Some("failed") => ActivityStatus::Failed,
            Some("declined" | "cancelled") => ActivityStatus::Cancelled,
            _ => ActivityStatus::Completed,
        }
    } else {
        ActivityStatus::Started
    };
    let title = string_at(item, &["command"])
        .or_else(|| string_at(item, &["query"]))
        .or_else(|| string_at(item, &["tool"]))
        .or_else(|| string_at(item, &["text"]))
        .unwrap_or_else(|| humanize_item_type(item_type));
    vec![ProviderEvent::Activity {
        activity: ProviderActivity {
            activity_id,
            kind,
            title,
            status,
        },
    }]
}

fn normalize_usage(params: &Value) -> Option<ProviderUsage> {
    let usage = params
        .get("tokenUsage")
        .or_else(|| params.get("usage"))
        .unwrap_or(params);
    let total = usage.get("total").unwrap_or(usage);
    Some(ProviderUsage {
        input_tokens: number_at(total, &["inputTokens"])
            .or_else(|| number_at(total, &["input_tokens"]))?,
        output_tokens: number_at(total, &["outputTokens"])
            .or_else(|| number_at(total, &["output_tokens"]))
            .unwrap_or(0),
        cached_input_tokens: number_at(total, &["cachedInputTokens"])
            .or_else(|| number_at(total, &["cached_input_tokens"]))
            .unwrap_or(0),
    })
}

pub(super) fn normalize_codex_error(value: &Value) -> ProviderError {
    let error = value
        .get("error")
        .or_else(|| value.get("turn").and_then(|turn| turn.get("error")))
        .unwrap_or(value);
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("Codex App Server reported a provider failure")
        .to_owned();
    let info_text = error
        .get("codexErrorInfo")
        .map(display_json)
        .unwrap_or_default();
    let code = if info_text.contains("Unauthorized") {
        ProviderErrorCode::AuthenticationRequired
    } else if info_text.contains("BadRequest") {
        ProviderErrorCode::InvalidRequest
    } else {
        ProviderErrorCode::Unavailable
    };
    ProviderError {
        code,
        message,
        retryable: matches!(code, ProviderErrorCode::Unavailable),
        diagnostic_id: Some(Uuid::now_v7()),
    }
}

pub(super) fn rpc_error(error: &Value) -> ProviderError {
    let code = error
        .get("code")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("Codex App Server request failed")
        .to_owned();
    ProviderError {
        code: if code == -32001 {
            ProviderErrorCode::Unavailable
        } else if message.to_ascii_lowercase().contains("auth") {
            ProviderErrorCode::AuthenticationRequired
        } else {
            ProviderErrorCode::InvalidRequest
        },
        message,
        retryable: code == -32001,
        diagnostic_id: Some(Uuid::now_v7()),
    }
}

fn number_at(value: &Value, path: &[&str]) -> Option<u64> {
    path.iter()
        .try_fold(value, |current, key| current.get(*key))?
        .as_u64()
}

fn humanize_item_type(value: &str) -> String {
    let mut result = String::new();
    for character in value.chars() {
        if character.is_ascii_uppercase() && !result.is_empty() {
            result.push(' ');
        }
        result.push(character);
    }
    let mut chars = result.chars();
    chars.next().map_or_else(
        || "Tool".to_owned(),
        |first| first.to_uppercase().collect::<String>() + chars.as_str(),
    )
}
