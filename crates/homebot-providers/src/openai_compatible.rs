//! OpenAI-compatible HTTP provider with secret-reference-only profiles.

#[path = "openai_compatible/protocol.rs"]
mod protocol;

use crate::{
    ApprovalDecision, CompactRequest, ExecutionMode, ProviderAdapter, ProviderAdapterId,
    ProviderAvailability, ProviderCapabilities, ProviderCapability, ProviderDescriptor,
    ProviderError, ProviderErrorCode, ProviderEvent, ProviderHealth, ProviderModel, ProviderRun,
    ProviderSecretResolver, ProviderTool, ProviderToolResult, ResumeRequest, SecretReference,
    StartRequest,
};
use futures_util::StreamExt;
use reqwest::{Client, StatusCode, Url};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{Mutex, mpsc, oneshot, watch};
use uuid::Uuid;

use protocol::normalize_event;

const EVENT_BUFFER: usize = 128;
const MAX_SSE_BUFFER: usize = 4 * 1024 * 1024;
const MAX_TOOL_CALLS_PER_RESPONSE: usize = 32;

struct PendingToolCall {
    operation_id: Uuid,
    result: oneshot::Sender<ProviderToolResult>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpenAiApiStyle {
    Responses,
    ChatCompletions,
}

#[derive(Clone, Debug)]
pub struct OpenAiCompatibleProfile {
    pub adapter_id: ProviderAdapterId,
    pub display_name: String,
    pub base_url: Url,
    pub api_style: OpenAiApiStyle,
    pub secret_reference: SecretReference,
    pub default_model: String,
}

impl OpenAiCompatibleProfile {
    /// Creates a profile that stores only an opaque reference to its credential.
    ///
    /// # Errors
    ///
    /// Rejects non-HTTPS remote endpoints so bearer credentials cannot cross cleartext links.
    pub fn new(
        adapter_id: ProviderAdapterId,
        display_name: impl Into<String>,
        mut base_url: Url,
        api_style: OpenAiApiStyle,
        secret_reference: SecretReference,
        default_model: impl Into<String>,
    ) -> Result<Self, ProviderError> {
        let loopback = base_url
            .host_str()
            .is_some_and(|host| matches!(host, "localhost" | "127.0.0.1" | "[::1]" | "::1"));
        if base_url.scheme() != "https" && !(base_url.scheme() == "http" && loopback) {
            return Err(invalid_request(
                "OpenAI-compatible endpoints require HTTPS except on loopback",
            ));
        }
        if !base_url.username().is_empty()
            || base_url.password().is_some()
            || base_url.query().is_some()
            || base_url.fragment().is_some()
        {
            return Err(invalid_request(
                "OpenAI-compatible endpoint credentials and parameters must not be embedded in the URL",
            ));
        }
        if !base_url.path().ends_with('/') {
            let path = format!("{}/", base_url.path());
            base_url.set_path(&path);
        }
        Ok(Self {
            adapter_id,
            display_name: display_name.into(),
            base_url,
            api_style,
            secret_reference,
            default_model: default_model.into(),
        })
    }
}

pub struct OpenAiCompatibleAdapter {
    profile: OpenAiCompatibleProfile,
    secrets: Arc<dyn ProviderSecretResolver>,
    client: Client,
    operations: Arc<Mutex<HashMap<Uuid, watch::Sender<bool>>>>,
    tool_calls: Arc<Mutex<HashMap<String, PendingToolCall>>>,
}

impl std::fmt::Debug for OpenAiCompatibleAdapter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OpenAiCompatibleAdapter")
            .field("adapter_id", &self.profile.adapter_id)
            .field("base_url", &self.profile.base_url)
            .field("api_style", &self.profile.api_style)
            .field("secret_reference", &self.profile.secret_reference)
            .finish_non_exhaustive()
    }
}

impl OpenAiCompatibleAdapter {
    /// Builds a bounded HTTP adapter.
    ///
    /// # Errors
    ///
    /// Returns a normalized error if the HTTP client cannot be configured.
    pub fn new(
        profile: OpenAiCompatibleProfile,
        secrets: Arc<dyn ProviderSecretResolver>,
    ) -> Result<Self, ProviderError> {
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(15 * 60))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("HomeBot/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| unavailable("Could not configure provider HTTP client"))?;
        Ok(Self {
            profile,
            secrets,
            client,
            operations: Arc::new(Mutex::new(HashMap::new())),
            tool_calls: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    fn endpoint(&self, path: &str) -> Result<Url, ProviderError> {
        self.profile
            .base_url
            .join(path)
            .map_err(|_| invalid_request("Provider endpoint URL is invalid"))
    }

    async fn authorized_get(&self, path: &str) -> Result<reqwest::Response, ProviderError> {
        let secret = self.secrets.resolve(self.profile.secret_reference).await?;
        let response = self
            .client
            .get(self.endpoint(path)?)
            .bearer_auth(secret.expose())
            .send()
            .await
            .map_err(http_transport_error)?;
        classify_status(response)
    }

    async fn run(
        &self,
        operation_id: Uuid,
        previous_response_id: Option<String>,
        prompt: String,
        model: Option<String>,
        mode: ExecutionMode,
        tools: Vec<ProviderTool>,
    ) -> Result<ProviderRun, ProviderError> {
        if mode == ExecutionMode::Plan {
            return Err(invalid_request(
                "This OpenAI-compatible profile does not expose provider plan mode",
            ));
        }
        if self.profile.api_style == OpenAiApiStyle::ChatCompletions
            && previous_response_id.is_some()
        {
            return Err(ProviderError {
                code: ProviderErrorCode::ConversationUnavailable,
                message: "Chat Completions profiles require HomeBot transcript replay".to_owned(),
                retryable: false,
                diagnostic_id: None,
            });
        }
        let model = model.unwrap_or_else(|| self.profile.default_model.clone());
        let (path, mut body) = initial_request(self.profile.api_style, &model, &prompt, &tools);
        if let Some(previous) = previous_response_id {
            body["previous_response_id"] = Value::String(previous);
        }
        let (events_tx, events_rx) = mpsc::channel(EVENT_BUFFER);
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let mut operations = self.operations.lock().await;
        if operations.insert(operation_id, cancel_tx).is_some() {
            return Err(invalid_request("Provider operation is already active"));
        }
        drop(operations);
        let secret = self.secrets.resolve(self.profile.secret_reference).await?;
        let response = async {
            let response = self
                .client
                .post(self.endpoint(path)?)
                .bearer_auth(secret.expose())
                .json(&body)
                .send()
                .await
                .map_err(http_transport_error)?;
            classify_status(response)
        }
        .await;
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                self.operations.lock().await.remove(&operation_id);
                return Err(error);
            }
        };
        let active = Arc::clone(&self.operations);
        let pending_tools = Arc::clone(&self.tool_calls);
        let style = self.profile.api_style;
        let client = self.client.clone();
        let endpoint = self.endpoint(path)?;
        tokio::spawn(async move {
            run_stream(
                response,
                StreamRun {
                    style,
                    operation_id,
                    model,
                    tools,
                    client,
                    endpoint,
                    secret,
                    cancel: cancel_rx,
                    events: events_tx,
                    pending_tools: Arc::clone(&pending_tools),
                    chat_messages: if style == OpenAiApiStyle::ChatCompletions {
                        vec![json!({"role":"user","content":prompt})]
                    } else {
                        Vec::new()
                    },
                },
            )
            .await;
            pending_tools
                .lock()
                .await
                .retain(|_, pending| pending.operation_id != operation_id);
            active.lock().await.remove(&operation_id);
        });
        Ok(ProviderRun {
            operation_id,
            events: events_rx,
        })
    }
}

#[async_trait::async_trait]
impl ProviderAdapter for OpenAiCompatibleAdapter {
    fn id(&self) -> &ProviderAdapterId {
        &self.profile.adapter_id
    }

    async fn discover(&self) -> Result<ProviderDescriptor, ProviderError> {
        let mut supported = [
            ProviderCapability::Streaming,
            ProviderCapability::Cancellation,
            ProviderCapability::Usage,
        ]
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
        supported.insert(ProviderCapability::DynamicTools);
        if self.profile.api_style == OpenAiApiStyle::Responses {
            supported.insert(ProviderCapability::ConversationResume);
            supported.insert(ProviderCapability::Activities);
        }
        Ok(ProviderDescriptor {
            adapter_id: self.profile.adapter_id.clone(),
            display_name: self.profile.display_name.clone(),
            executable: None,
            capabilities: ProviderCapabilities { supported },
        })
    }

    async fn health(&self) -> ProviderHealth {
        let checked_at_unix_ms = unix_ms();
        match self.authorized_get("models").await {
            Ok(_) => ProviderHealth {
                availability: ProviderAvailability::Available,
                message: "Provider API is ready".to_owned(),
                checked_at_unix_ms,
            },
            Err(error) => ProviderHealth {
                availability: if error.code == ProviderErrorCode::AuthenticationRequired {
                    ProviderAvailability::AuthenticationRequired
                } else {
                    ProviderAvailability::Unavailable
                },
                message: error.message,
                checked_at_unix_ms,
            },
        }
    }

    async fn models(&self) -> Result<Vec<ProviderModel>, ProviderError> {
        let response = self.authorized_get("models").await?;
        let body: Value = response
            .json()
            .await
            .map_err(|_| protocol_error("Provider model response was invalid JSON"))?;
        let data = body
            .get("data")
            .and_then(Value::as_array)
            .ok_or_else(|| protocol_error("Provider model response omitted data"))?;
        Ok(data
            .iter()
            .filter_map(|model| model.get("id").and_then(Value::as_str))
            .map(|id| ProviderModel {
                id: id.to_owned(),
                display_name: id.to_owned(),
                context_window_tokens: None,
                supports_reasoning: false,
            })
            .collect())
    }

    async fn start(&self, request: StartRequest) -> Result<ProviderRun, ProviderError> {
        if !request.attachments.is_empty() {
            return Err(invalid_request(
                "OpenAI-compatible attachments are not available from metadata alone",
            ));
        }
        self.run(
            request.operation_id,
            None,
            request.prompt,
            request.model,
            request.mode,
            request.tools,
        )
        .await
    }

    async fn resume(&self, request: ResumeRequest) -> Result<ProviderRun, ProviderError> {
        if !request.attachments.is_empty() {
            return Err(invalid_request(
                "OpenAI-compatible attachments are not available from metadata alone",
            ));
        }
        self.run(
            request.operation_id,
            Some(request.conversation_id),
            request.prompt,
            request.model,
            request.mode,
            request.tools,
        )
        .await
    }

    async fn cancel(&self, operation_id: Uuid) -> Result<(), ProviderError> {
        self.operations
            .lock()
            .await
            .get(&operation_id)
            .ok_or_else(|| invalid_request("Provider operation is not active"))?
            .send(true)
            .map_err(|_| invalid_request("Provider operation is not active"))
    }

    async fn resolve_approval(
        &self,
        _approval_id: Uuid,
        _decision: ApprovalDecision,
    ) -> Result<(), ProviderError> {
        Err(invalid_request(
            "OpenAI-compatible API profile has no pending approval",
        ))
    }

    async fn resolve_tool_call(
        &self,
        call_id: String,
        result: ProviderToolResult,
    ) -> Result<(), ProviderError> {
        self.tool_calls
            .lock()
            .await
            .remove(&call_id)
            .ok_or_else(|| invalid_request("Provider tool call is no longer pending"))?
            .result
            .send(result)
            .map_err(|_| invalid_request("Provider tool call is no longer pending"))
    }

    async fn compact(&self, _request: CompactRequest) -> Result<(), ProviderError> {
        Err(invalid_request(
            "OpenAI-compatible API profile does not expose manual compaction",
        ))
    }

    async fn recover(&self) -> Result<Vec<ProviderRun>, ProviderError> {
        Ok(Vec::new())
    }
}

struct StreamRun {
    style: OpenAiApiStyle,
    operation_id: Uuid,
    model: String,
    tools: Vec<ProviderTool>,
    client: Client,
    endpoint: Url,
    secret: crate::ResolvedSecret,
    cancel: watch::Receiver<bool>,
    events: mpsc::Sender<ProviderEvent>,
    pending_tools: Arc<Mutex<HashMap<String, PendingToolCall>>>,
    chat_messages: Vec<Value>,
}

enum Continuation {
    Responses(String),
    Chat(Vec<Value>),
}

enum StreamOutcome {
    Continue {
        continuation: Continuation,
        calls: Vec<(String, oneshot::Receiver<ProviderToolResult>)>,
    },
    Finished,
}

async fn run_stream(mut response: reqwest::Response, mut run: StreamRun) {
    let mut conversation_started = false;
    loop {
        let StreamOutcome::Continue {
            continuation,
            calls,
        } = consume_sse(response, &mut run, &mut conversation_started).await
        else {
            return;
        };
        let mut results = Vec::with_capacity(calls.len());
        for (call_id, receiver) in calls {
            let result = tokio::select! {
                changed = run.cancel.changed() => {
                    let _ = changed;
                    let _ = run.events.send(ProviderEvent::Cancelled).await;
                    return;
                }
                result = receiver => result,
            };
            let Ok(result) = result else {
                let _ = run
                    .events
                    .send(ProviderEvent::Failed {
                        error: protocol_error("HomeBot tool result channel closed"),
                    })
                    .await;
                return;
            };
            results.push((call_id, result));
        }
        let body = continuation_request(&mut run, continuation, results);
        let request = run
            .client
            .post(run.endpoint.clone())
            .bearer_auth(run.secret.expose())
            .json(&body)
            .send();
        response = tokio::select! {
            changed = run.cancel.changed() => {
                let _ = changed;
                let _ = run.events.send(ProviderEvent::Cancelled).await;
                return;
            }
            response = request => match response.map_err(http_transport_error).and_then(classify_status) {
                Ok(response) => response,
                Err(error) => {
                    let _ = run.events.send(ProviderEvent::Failed { error }).await;
                    return;
                }
            },
        };
    }
}

async fn consume_sse(
    response: reqwest::Response,
    run: &mut StreamRun,
    conversation_started: &mut bool,
) -> StreamOutcome {
    let mut stream = response.bytes_stream();
    let mut buffer = Vec::new();
    let mut response_id = None;
    let mut calls = Vec::new();
    let mut chat_calls = Vec::new();
    loop {
        tokio::select! {
            changed = run.cancel.changed() => {
                if changed.is_err() || *run.cancel.borrow() {
                    let _ = run.events.send(ProviderEvent::Cancelled).await;
                    return StreamOutcome::Finished;
                }
            }
            chunk = stream.next() => {
                match chunk {
                    Some(Ok(bytes)) => {
                        buffer.extend_from_slice(&bytes);
                        if buffer.len() > MAX_SSE_BUFFER {
                            fail(&run.events, "Provider SSE event exceeded the limit").await;
                            return StreamOutcome::Finished;
                        }
                        while let Some(end) = find_event_end(&buffer) {
                            let event = buffer.drain(..end).collect::<Vec<_>>();
                            let separator = if buffer.starts_with(b"\r\n\r\n") { 4 } else { 2 };
                            buffer.drain(..separator);
                            if let Some(data) = sse_data(&event) {
                                if data == b"[DONE]" {
                                    if let Some(response_id) = response_id.filter(|_| !calls.is_empty()) {
                                        return StreamOutcome::Continue {
                                            continuation: Continuation::Responses(response_id),
                                            calls,
                                        };
                                    }
                                    let _ = run.events.send(ProviderEvent::Completed).await;
                                    return StreamOutcome::Finished;
                                }
                                let Ok(value) = serde_json::from_slice::<Value>(&data) else {
                                    fail(&run.events, "Provider SSE data was invalid JSON").await;
                                    return StreamOutcome::Finished;
                                };
                                if run.style == OpenAiApiStyle::ChatCompletions {
                                    match chat_tool_outcome(&value, run, &mut chat_calls, &mut calls).await {
                                        Ok(Some(outcome)) => return outcome,
                                        Ok(None) => {}
                                        Err(error) => {
                                            let _ = run.events.send(ProviderEvent::Failed { error }).await;
                                            return StreamOutcome::Finished;
                                        }
                                    }
                                }
                                for event in normalize_event(run.style, &value) {
                                    if let Some(outcome) = handle_normalized_event(
                                        run,
                                        event,
                                        &mut response_id,
                                        &mut calls,
                                        conversation_started,
                                    )
                                    .await
                                    {
                                        return outcome;
                                    }
                                }
                            }
                        }
                    }
                    Some(Err(_)) => {
                        let _ = run.events.send(ProviderEvent::Failed { error: unavailable("Provider stream disconnected") }).await;
                        return StreamOutcome::Finished;
                    }
                    None => {
                        fail(&run.events, "Provider stream ended without a terminal event").await;
                        return StreamOutcome::Finished;
                    }
                }
            }
        }
    }
}

async fn handle_normalized_event(
    run: &StreamRun,
    event: ProviderEvent,
    response_id: &mut Option<String>,
    calls: &mut Vec<(String, oneshot::Receiver<ProviderToolResult>)>,
    conversation_started: &mut bool,
) -> Option<StreamOutcome> {
    match event {
        ProviderEvent::ConversationStarted { conversation_id } => {
            *response_id = Some(conversation_id.clone());
            if !*conversation_started {
                *conversation_started = true;
                if run
                    .events
                    .send(ProviderEvent::ConversationStarted { conversation_id })
                    .await
                    .is_err()
                {
                    return Some(StreamOutcome::Finished);
                }
            }
        }
        ProviderEvent::ToolCall { call } => {
            if let Err(error) = register_tool_call(run, call, calls).await {
                let _ = run.events.send(ProviderEvent::Failed { error }).await;
                return Some(StreamOutcome::Finished);
            }
        }
        ProviderEvent::Completed if calls.is_empty() => {
            let _ = run.events.send(ProviderEvent::Completed).await;
            return Some(StreamOutcome::Finished);
        }
        ProviderEvent::Completed => {
            let Some(response_id) = response_id.take() else {
                fail(
                    &run.events,
                    "Provider tool response omitted its response identifier",
                )
                .await;
                return Some(StreamOutcome::Finished);
            };
            return Some(StreamOutcome::Continue {
                continuation: Continuation::Responses(response_id),
                calls: std::mem::take(calls),
            });
        }
        ProviderEvent::Cancelled | ProviderEvent::Failed { .. } => {
            let _ = run.events.send(event).await;
            return Some(StreamOutcome::Finished);
        }
        event => {
            if run.events.send(event).await.is_err() {
                return Some(StreamOutcome::Finished);
            }
        }
    }
    None
}

async fn chat_tool_outcome(
    value: &Value,
    run: &StreamRun,
    chat_calls: &mut Vec<ChatCall>,
    calls: &mut Vec<(String, oneshot::Receiver<ProviderToolResult>)>,
) -> Result<Option<StreamOutcome>, ProviderError> {
    accumulate_chat_calls(value, chat_calls)?;
    if chat_finish_reason(value) != Some("tool_calls") {
        return Ok(None);
    }
    let tool_calls = finish_chat_calls(chat_calls)?;
    let assistant_calls = chat_tool_values(chat_calls);
    for call in tool_calls {
        register_tool_call(run, call, calls).await?;
    }
    Ok(Some(StreamOutcome::Continue {
        continuation: Continuation::Chat(assistant_calls),
        calls: std::mem::take(calls),
    }))
}

#[derive(Default)]
struct ChatCall {
    id: String,
    name: String,
    arguments: String,
}

fn accumulate_chat_calls(value: &Value, calls: &mut Vec<ChatCall>) -> Result<(), ProviderError> {
    let Some(deltas) = value
        .pointer("/choices/0/delta/tool_calls")
        .and_then(Value::as_array)
    else {
        return Ok(());
    };
    for delta in deltas {
        let index = delta
            .get("index")
            .and_then(Value::as_u64)
            .and_then(|index| usize::try_from(index).ok())
            .ok_or_else(|| protocol_error("Provider chat tool index was invalid"))?;
        if index >= MAX_TOOL_CALLS_PER_RESPONSE {
            return Err(protocol_error("Provider exceeded the tool-call limit"));
        }
        calls.resize_with(index + 1, ChatCall::default);
        let call = &mut calls[index];
        if let Some(id) = delta.get("id").and_then(Value::as_str) {
            id.clone_into(&mut call.id);
        }
        if let Some(name) = delta.pointer("/function/name").and_then(Value::as_str) {
            name.clone_into(&mut call.name);
        }
        if let Some(arguments) = delta.pointer("/function/arguments").and_then(Value::as_str) {
            if call.arguments.len().saturating_add(arguments.len()) > MAX_SSE_BUFFER {
                return Err(protocol_error(
                    "Provider chat tool arguments exceeded the limit",
                ));
            }
            call.arguments.push_str(arguments);
        }
    }
    Ok(())
}

fn chat_finish_reason(value: &Value) -> Option<&str> {
    value
        .pointer("/choices/0/finish_reason")
        .and_then(Value::as_str)
}

fn finish_chat_calls(calls: &[ChatCall]) -> Result<Vec<crate::ProviderToolCall>, ProviderError> {
    if calls.is_empty() {
        return Err(protocol_error("Provider chat tool call was empty"));
    }
    calls
        .iter()
        .map(|call| {
            if call.id.is_empty() || call.name.is_empty() {
                return Err(protocol_error("Provider chat tool call was malformed"));
            }
            Ok(crate::ProviderToolCall {
                call_id: call.id.clone(),
                name: call.name.clone(),
                arguments: serde_json::from_str(&call.arguments).map_err(|_| {
                    protocol_error("Provider chat tool arguments were invalid JSON")
                })?,
            })
        })
        .collect()
}

fn chat_tool_values(calls: &[ChatCall]) -> Vec<Value> {
    calls
        .iter()
        .map(|call| {
            json!({
                "id": call.id,
                "type": "function",
                "function": {"name":call.name,"arguments":call.arguments},
            })
        })
        .collect()
}

fn continuation_request(
    run: &mut StreamRun,
    continuation: Continuation,
    results: Vec<(String, ProviderToolResult)>,
) -> Value {
    match continuation {
        Continuation::Responses(response_id) => json!({
            "model": run.model,
            "input": results.into_iter().map(|(call_id, result)| json!({
                "type":"function_call_output",
                "call_id":call_id,
                "output":json!({"success":result.success,"content":result.content}).to_string(),
            })).collect::<Vec<_>>(),
            "previous_response_id": response_id,
            "stream": true,
            "tools": responses_tools(&run.tools),
        }),
        Continuation::Chat(tool_calls) => {
            run.chat_messages.push(json!({
                "role":"assistant",
                "content":null,
                "tool_calls":tool_calls,
            }));
            run.chat_messages.extend(results.into_iter().map(|(call_id, result)| {
                json!({
                    "role":"tool",
                    "tool_call_id":call_id,
                    "content":json!({"success":result.success,"content":result.content}).to_string(),
                })
            }));
            json!({
                "model":run.model,
                "messages":run.chat_messages,
                "stream":true,
                "stream_options":{"include_usage":true},
                "tools":chat_tools(&run.tools),
            })
        }
    }
}

async fn register_tool_call(
    run: &StreamRun,
    call: crate::ProviderToolCall,
    calls: &mut Vec<(String, oneshot::Receiver<ProviderToolResult>)>,
) -> Result<(), ProviderError> {
    if calls.len() >= MAX_TOOL_CALLS_PER_RESPONSE {
        return Err(protocol_error("Provider exceeded the tool-call limit"));
    }
    let (result, receiver) = oneshot::channel();
    let mut pending = run.pending_tools.lock().await;
    if pending.contains_key(&call.call_id) {
        return Err(protocol_error("Provider repeated a tool-call identifier"));
    }
    pending.insert(
        call.call_id.clone(),
        PendingToolCall {
            operation_id: run.operation_id,
            result,
        },
    );
    drop(pending);
    calls.push((call.call_id.clone(), receiver));
    run.events
        .send(ProviderEvent::ToolCall { call })
        .await
        .map_err(|_| unavailable("Provider event receiver closed"))
}

async fn fail(events: &mpsc::Sender<ProviderEvent>, message: &str) {
    let _ = events
        .send(ProviderEvent::Failed {
            error: protocol_error(message),
        })
        .await;
}

fn initial_request(
    style: OpenAiApiStyle,
    model: &str,
    prompt: &str,
    tools: &[ProviderTool],
) -> (&'static str, Value) {
    match style {
        OpenAiApiStyle::Responses => (
            "responses",
            json!({"model":model,"input":prompt,"stream":true,"tools":responses_tools(tools)}),
        ),
        OpenAiApiStyle::ChatCompletions => (
            "chat/completions",
            json!({
                "model": model,
                "messages": [{"role": "user", "content": prompt}],
                "stream": true,
                "stream_options": {"include_usage": true},
                "tools": chat_tools(tools),
            }),
        ),
    }
}

fn chat_tools(tools: &[ProviderTool]) -> Value {
    Value::Array(
        tools
            .iter()
            .map(|tool| {
                json!({
                    "type":"function",
                    "function":{
                        "name":tool.name,
                        "description":tool.description,
                        "parameters":tool.input_schema,
                    },
                })
            })
            .collect(),
    )
}

fn responses_tools(tools: &[ProviderTool]) -> Value {
    Value::Array(
        tools
            .iter()
            .map(|tool| {
                json!({
                    "type": "function",
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.input_schema,
                })
            })
            .collect(),
    )
}

fn find_event_end(buffer: &[u8]) -> Option<usize> {
    buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .or_else(|| buffer.windows(2).position(|window| window == b"\n\n"))
}

fn sse_data(event: &[u8]) -> Option<Vec<u8>> {
    let mut data = Vec::new();
    for line in event.split(|byte| *byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if let Some(value) = line.strip_prefix(b"data:") {
            if !data.is_empty() {
                data.push(b'\n');
            }
            data.extend_from_slice(value.strip_prefix(b" ").unwrap_or(value));
        }
    }
    (!data.is_empty()).then_some(data)
}

fn classify_status(response: reqwest::Response) -> Result<reqwest::Response, ProviderError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    Err(ProviderError {
        code: if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            ProviderErrorCode::AuthenticationRequired
        } else if status.is_client_error() {
            ProviderErrorCode::InvalidRequest
        } else {
            ProviderErrorCode::Unavailable
        },
        message: format!("Provider API returned HTTP {}", status.as_u16()),
        retryable: status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error(),
        diagnostic_id: Some(Uuid::now_v7()),
    })
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn http_transport_error(_error: reqwest::Error) -> ProviderError {
    unavailable("Provider API connection failed")
}

fn protocol_error(message: impl Into<String>) -> ProviderError {
    ProviderError {
        code: ProviderErrorCode::ProtocolViolation,
        message: message.into(),
        retryable: false,
        diagnostic_id: Some(Uuid::now_v7()),
    }
}

fn invalid_request(message: impl Into<String>) -> ProviderError {
    ProviderError {
        code: ProviderErrorCode::InvalidRequest,
        message: message.into(),
        retryable: false,
        diagnostic_id: None,
    }
}

fn unavailable(message: impl Into<String>) -> ProviderError {
    ProviderError {
        code: ProviderErrorCode::Unavailable,
        message: message.into(),
        retryable: true,
        diagnostic_id: Some(Uuid::now_v7()),
    }
}

#[cfg(test)]
#[path = "openai_compatible/tests.rs"]
mod tests;
