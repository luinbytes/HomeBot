use crate::{ProviderEvent, ProviderTool, ProviderToolCall, ProviderToolResult};
use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fmt::Write as _,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Mutex, mpsc, oneshot, watch};
use uuid::Uuid;

const TOOL_RESULT_TIMEOUT: Duration = Duration::from_secs(300);

struct PendingCall {
    operation_id: Uuid,
    result: oneshot::Sender<ProviderToolResult>,
}

#[derive(Default)]
pub(super) struct ToolCallRegistry {
    pending: Mutex<HashMap<String, PendingCall>>,
}

impl ToolCallRegistry {
    pub(super) async fn resolve(
        &self,
        call_id: String,
        result: ProviderToolResult,
    ) -> Result<(), ()> {
        self.pending
            .lock()
            .await
            .remove(&call_id)
            .ok_or(())?
            .result
            .send(result)
            .map_err(|_| ())
    }

    pub(super) async fn clear_operation(&self, operation_id: Uuid) {
        self.pending
            .lock()
            .await
            .retain(|_, pending| pending.operation_id != operation_id);
    }
}

#[derive(Clone)]
struct BridgeState {
    operation_id: Uuid,
    tools: Arc<Vec<ProviderTool>>,
    events: mpsc::Sender<ProviderEvent>,
    calls: Arc<ToolCallRegistry>,
    counter: Arc<AtomicU64>,
    shutdown: watch::Receiver<bool>,
}

pub(super) struct ToolBridge {
    url: String,
    shutdown: watch::Sender<bool>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl ToolBridge {
    pub(super) fn config(&self) -> String {
        json!({"mcpServers":{"homebot":{"type":"http","url":self.url}}}).to_string()
    }

    pub(super) async fn shutdown(mut self) {
        let _ = self.shutdown.send(true);
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

impl Drop for ToolBridge {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

pub(super) async fn start(
    operation_id: Uuid,
    tools: Vec<ProviderTool>,
    events: mpsc::Sender<ProviderEvent>,
    calls: Arc<ToolCallRegistry>,
) -> std::io::Result<ToolBridge> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let mut token = [0_u8; 16];
    getrandom::fill(&mut token).map_err(std::io::Error::other)?;
    let mut token_hex = String::with_capacity(32);
    for byte in token {
        let _ = write!(token_hex, "{byte:02x}");
    }
    let path = format!("/{token_hex}/mcp");
    let (shutdown, shutdown_rx) = watch::channel(false);
    let state = BridgeState {
        operation_id,
        tools: Arc::new(tools),
        events,
        calls,
        counter: Arc::new(AtomicU64::new(1)),
        shutdown: shutdown_rx.clone(),
    };
    let app = Router::new().route(&path, post(handle)).with_state(state);
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let mut shutdown = shutdown_rx;
                let _ = shutdown.changed().await;
            })
            .await;
    });
    Ok(ToolBridge {
        url: format!("http://{address}{path}"),
        shutdown,
        task: Some(task),
    })
}

async fn handle(State(state): State<BridgeState>, Json(message): Json<Value>) -> Response {
    let id = message.get("id").cloned().unwrap_or(Value::Null);
    match message.get("method").and_then(Value::as_str) {
        Some("initialize") => Json(json!({
            "jsonrpc":"2.0", "id":id,
            "result":{
                "protocolVersion":"2025-06-18",
                "capabilities":{"tools":{}},
                "serverInfo":{"name":"HomeBot","version":env!("CARGO_PKG_VERSION")}
            }
        }))
        .into_response(),
        Some("notifications/initialized") => StatusCode::ACCEPTED.into_response(),
        Some("tools/list") => Json(json!({
            "jsonrpc":"2.0", "id":id,
            "result":{"tools":state.tools.iter().map(|tool| json!({
                "name":tool.name,
                "description":tool.description,
                "inputSchema":tool.input_schema
            })).collect::<Vec<_>>()}
        }))
        .into_response(),
        Some("tools/call") => call_tool(state, id, &message).await,
        _ => Json(json!({
            "jsonrpc":"2.0", "id":id,
            "error":{"code":-32601,"message":"Method not found"}
        }))
        .into_response(),
    }
}

async fn call_tool(state: BridgeState, id: Value, message: &Value) -> Response {
    let Some(name) = message.pointer("/params/name").and_then(Value::as_str) else {
        return invalid_params(&id);
    };
    let arguments = message
        .pointer("/params/arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    if !arguments.is_object() || !state.tools.iter().any(|tool| tool.name == name) {
        return invalid_params(&id);
    }
    let sequence = state.counter.fetch_add(1, Ordering::Relaxed);
    let call_id = format!("claude_{}_{sequence}", state.operation_id.simple());
    let (result_tx, result_rx) = oneshot::channel();
    state.calls.pending.lock().await.insert(
        call_id.clone(),
        PendingCall {
            operation_id: state.operation_id,
            result: result_tx,
        },
    );
    if state
        .events
        .send(ProviderEvent::ToolCall {
            call: ProviderToolCall {
                call_id: call_id.clone(),
                name: name.to_owned(),
                arguments,
            },
        })
        .await
        .is_err()
    {
        state.calls.pending.lock().await.remove(&call_id);
        return tool_error(&id, "HomeBot turn ended");
    }
    let mut shutdown = state.shutdown.clone();
    let result = tokio::select! {
        result = tokio::time::timeout(TOOL_RESULT_TIMEOUT, result_rx) => result.ok().and_then(Result::ok),
        _ = shutdown.changed() => None,
    };
    state.calls.pending.lock().await.remove(&call_id);
    let Some(result) = result else {
        return tool_error(&id, "HomeBot tool result was unavailable");
    };
    Json(json!({
        "jsonrpc":"2.0", "id":id,
        "result":{
            "content":[{"type":"text","text":result.content}],
            "isError":!result.success
        }
    }))
    .into_response()
}

fn invalid_params(id: &Value) -> Response {
    Json(json!({
        "jsonrpc":"2.0", "id":id,
        "error":{"code":-32602,"message":"Invalid tool call"}
    }))
    .into_response()
}

fn tool_error(id: &Value, message: &str) -> Response {
    Json(json!({
        "jsonrpc":"2.0", "id":id,
        "error":{"code":-32000,"message":message}
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bridge_lists_calls_and_resolves_homebot_tools()
    -> Result<(), Box<dyn std::error::Error>> {
        let operation_id = Uuid::now_v7();
        let calls = Arc::new(ToolCallRegistry::default());
        let (events, mut event_rx) = mpsc::channel(8);
        let bridge = start(
            operation_id,
            vec![ProviderTool {
                name: "homebot_handoff".to_owned(),
                description: "Hand work to a teammate".to_owned(),
                input_schema: json!({"type":"object","required":["bot"]}),
            }],
            events,
            Arc::clone(&calls),
        )
        .await?;
        let client = reqwest::Client::new();
        let listed: Value = client
            .post(&bridge.url)
            .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}))
            .send()
            .await?
            .json()
            .await?;
        assert_eq!(
            listed.pointer("/result/tools/0/name"),
            Some(&json!("homebot_handoff"))
        );

        let url = bridge.url.clone();
        let call = tokio::spawn(async move {
            client
                .post(url)
                .json(&json!({
                    "jsonrpc":"2.0","id":2,"method":"tools/call",
                    "params":{"name":"homebot_handoff","arguments":{"bot":"Patch"}}
                }))
                .send()
                .await?
                .json::<Value>()
                .await
        });
        let ProviderEvent::ToolCall { call: tool_call } = event_rx.recv().await.ok_or("no call")?
        else {
            return Err("unexpected provider event".into());
        };
        assert_eq!(tool_call.name, "homebot_handoff");
        assert_eq!(tool_call.arguments, json!({"bot":"Patch"}));
        calls
            .resolve(
                tool_call.call_id,
                ProviderToolResult {
                    success: true,
                    content: "Patch completed the handoff".to_owned(),
                },
            )
            .await
            .map_err(|()| "resolve failed")?;
        let response = call.await??;
        assert_eq!(
            response.pointer("/result/content/0/text"),
            Some(&json!("Patch completed the handoff"))
        );
        bridge.shutdown().await;
        Ok(())
    }
}
