//! OpenAI-compatible HTTP provider with secret-reference-only profiles.

#[path = "openai_compatible/protocol.rs"]
mod protocol;

use crate::{
    ApprovalDecision, CompactRequest, ExecutionMode, ProviderAdapter, ProviderAdapterId,
    ProviderAvailability, ProviderCapabilities, ProviderCapability, ProviderDescriptor,
    ProviderError, ProviderErrorCode, ProviderEvent, ProviderHealth, ProviderModel, ProviderRun,
    ProviderSecretResolver, ResumeRequest, SecretReference, StartRequest,
};
use futures_util::StreamExt;
use reqwest::{Client, StatusCode, Url};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{Mutex, mpsc, watch};
use uuid::Uuid;

use protocol::normalize_event;

const EVENT_BUFFER: usize = 128;
const MAX_SSE_BUFFER: usize = 4 * 1024 * 1024;

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
        let (path, mut body) = match self.profile.api_style {
            OpenAiApiStyle::Responses => (
                "responses",
                json!({"model": model, "input": prompt, "stream": true}),
            ),
            OpenAiApiStyle::ChatCompletions => (
                "chat/completions",
                json!({
                    "model": model,
                    "messages": [{"role": "user", "content": prompt}],
                    "stream": true,
                    "stream_options": {"include_usage": true}
                }),
            ),
        };
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
        let response = async {
            let secret = self.secrets.resolve(self.profile.secret_reference).await?;
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
        let style = self.profile.api_style;
        tokio::spawn(async move {
            consume_sse(response, style, cancel_rx, events_tx).await;
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

    async fn compact(&self, _request: CompactRequest) -> Result<(), ProviderError> {
        Err(invalid_request(
            "OpenAI-compatible API profile does not expose manual compaction",
        ))
    }

    async fn recover(&self) -> Result<Vec<ProviderRun>, ProviderError> {
        Ok(Vec::new())
    }
}

async fn consume_sse(
    response: reqwest::Response,
    style: OpenAiApiStyle,
    mut cancel: watch::Receiver<bool>,
    events: mpsc::Sender<ProviderEvent>,
) {
    let mut stream = response.bytes_stream();
    let mut buffer = Vec::new();
    let mut terminal = false;
    let mut conversation_started = false;
    loop {
        tokio::select! {
            changed = cancel.changed() => {
                if changed.is_err() || *cancel.borrow() {
                    let _ = events.send(ProviderEvent::Cancelled).await;
                    return;
                }
            }
            chunk = stream.next() => {
                match chunk {
                    Some(Ok(bytes)) => {
                        buffer.extend_from_slice(&bytes);
                        if buffer.len() > MAX_SSE_BUFFER {
                            let _ = events.send(ProviderEvent::Failed { error: protocol_error("Provider SSE event exceeded the limit") }).await;
                            return;
                        }
                        while let Some(end) = find_event_end(&buffer) {
                            let event = buffer.drain(..end).collect::<Vec<_>>();
                            let separator = if buffer.starts_with(b"\r\n\r\n") { 4 } else { 2 };
                            buffer.drain(..separator);
                            if let Some(data) = sse_data(&event) {
                                if data == b"[DONE]" {
                                    if !terminal {
                                        let _ = events.send(ProviderEvent::Completed).await;
                                    }
                                    return;
                                }
                                let Ok(value) = serde_json::from_slice::<Value>(&data) else {
                                    let _ = events.send(ProviderEvent::Failed { error: protocol_error("Provider SSE data was invalid JSON") }).await;
                                    return;
                                };
                                let mut normalized = normalize_event(style, &value);
                                normalized.retain(|event| {
                                    if matches!(event, ProviderEvent::ConversationStarted { .. }) {
                                        if conversation_started {
                                            return false;
                                        }
                                        conversation_started = true;
                                    }
                                    true
                                });
                                terminal |= normalized.iter().any(is_terminal);
                                for event in normalized {
                                    if events.send(event).await.is_err() {
                                        return;
                                    }
                                }
                                if terminal {
                                    return;
                                }
                            }
                        }
                    }
                    Some(Err(_)) => {
                        let _ = events.send(ProviderEvent::Failed { error: unavailable("Provider stream disconnected") }).await;
                        return;
                    }
                    None => {
                        if !terminal {
                            let _ = events.send(ProviderEvent::Failed { error: protocol_error("Provider stream ended without a terminal event") }).await;
                        }
                        return;
                    }
                }
            }
        }
    }
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

fn is_terminal(event: &ProviderEvent) -> bool {
    matches!(
        event,
        ProviderEvent::Completed | ProviderEvent::Cancelled | ProviderEvent::Failed { .. }
    )
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
