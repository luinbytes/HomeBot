use crate::{
    ActivityKind, ActivitySink, ActivityStatus, CapabilityClass, CapabilityRequest,
    OperationContext, PolicyEngine, ToolActivity, ToolError,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use futures_util::{SinkExt, StreamExt};
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use uuid::Uuid;

const MAX_CDP_MESSAGE_BYTES: usize = 4 * 1024 * 1024;
const MAX_CDP_MESSAGES: usize = 512;
const MAX_BROWSER_SESSIONS: usize = 64;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BrowserSessionProfile {
    pub profile_id: Uuid,
    pub display_name: String,
    /// Relative directory below the server-owned browser state root.
    pub profile_directory: PathBuf,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BrowserAction {
    Navigate { url: String },
    Evaluate { expression: String },
    CaptureScreenshot,
    CurrentUrl,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BrowserResult {
    SessionCreated { session_id: Uuid },
    NavigationAccepted,
    Evaluation { value: Value },
    ScreenshotPng { bytes: Vec<u8> },
    Url { url: String },
    SessionClosed,
}

#[derive(Clone, Debug)]
struct BrowserSession {
    target_id: String,
    websocket_url: Url,
    profile_id: Uuid,
    browser_context_id: String,
    _slot: Arc<OwnedSemaphorePermit>,
}

pub struct BrowserService {
    endpoint: Url,
    profile_root: PathBuf,
    client: Client,
    policy: Arc<PolicyEngine>,
    activities: Arc<dyn ActivitySink>,
    sessions: Mutex<HashMap<Uuid, BrowserSession>>,
    profile_contexts: Mutex<HashMap<Uuid, String>>,
    session_slots: Arc<Semaphore>,
    request_timeout: Duration,
}

impl std::fmt::Debug for BrowserService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BrowserService")
            .field("endpoint", &self.endpoint)
            .field("profile_root", &self.profile_root)
            .field("request_timeout", &self.request_timeout)
            .finish_non_exhaustive()
    }
}

impl BrowserService {
    /// Creates a controller for a local Chrome `DevTools` Protocol endpoint.
    ///
    /// The endpoint must be loopback. Browser cookies and other authentication
    /// state remain in `profile_root` on the `HomeBot` server and are never
    /// returned through this API.
    ///
    /// # Errors
    ///
    /// Rejects remote endpoints and profile roots that cannot be canonicalized.
    pub fn new(
        mut endpoint: Url,
        profile_root: impl AsRef<Path>,
        policy: Arc<PolicyEngine>,
        activities: Arc<dyn ActivitySink>,
    ) -> Result<Self, ToolError> {
        if endpoint.scheme() != "http" || !is_loopback(&endpoint) {
            return Err(ToolError::InvalidRequest(
                "browser control endpoint must use loopback HTTP".to_owned(),
            ));
        }
        if !endpoint.path().ends_with('/') {
            endpoint.set_path(&format!("{}/", endpoint.path()));
        }
        let profile_root =
            std::fs::canonicalize(profile_root).map_err(|_| ToolError::Unavailable)?;
        if !profile_root.is_dir() {
            return Err(ToolError::InvalidRequest(
                "browser profile root is not a directory".to_owned(),
            ));
        }
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| ToolError::Unavailable)?;
        Ok(Self {
            endpoint,
            profile_root,
            client,
            policy,
            activities,
            sessions: Mutex::new(HashMap::new()),
            profile_contexts: Mutex::new(HashMap::new()),
            session_slots: Arc::new(Semaphore::new(MAX_BROWSER_SESSIONS)),
            request_timeout: Duration::from_secs(15),
        })
    }

    /// Creates a single server-owned profile directory without exposing its path to clients.
    ///
    /// # Errors
    /// Rejects nested, parent, absolute, or symlink-backed directory references.
    pub fn ensure_profile_directory(
        &self,
        profile: &BrowserSessionProfile,
    ) -> Result<(), ToolError> {
        let mut components = profile.profile_directory.components();
        if profile.profile_directory.is_absolute()
            || !matches!(components.next(), Some(Component::Normal(_)))
            || components.next().is_some()
        {
            return Err(ToolError::PathOutsideWorkspace);
        }
        let path = self.profile_root.join(&profile.profile_directory);
        if path.exists() {
            reject_profile_symlinks(&self.profile_root, &path)?;
            if !path.is_dir() {
                return Err(ToolError::PathOutsideWorkspace);
            }
            return Ok(());
        }
        std::fs::create_dir(&path).map_err(|_| ToolError::OperationFailed)?;
        reject_profile_symlinks(&self.profile_root, &path)
    }

    /// Opens a new page target using a server-local profile reference.
    ///
    /// # Errors
    ///
    /// Requires policy authorization and a valid local profile and CDP target.
    pub async fn create_session(
        &self,
        context: OperationContext,
        profile: &BrowserSessionProfile,
        approval_id: Option<Uuid>,
    ) -> Result<BrowserResult, ToolError> {
        let profile_path = self.validate_profile(profile)?;
        let request = CapabilityRequest {
            context: context.clone(),
            capability: CapabilityClass::BrowserAct,
            action: "browser.session.create".to_owned(),
            canonical_resource: format!(
                "profile:{}:{}",
                profile.profile_id,
                profile_path.display()
            ),
            summary: format!("Open browser profile {}", profile.display_name),
            destructive: false,
        };
        let _authorization = self.policy.authorize(&request, approval_id).await?;
        self.emit(
            &context,
            ActivityStatus::Started,
            "Opening browser session",
            None,
        )
        .await;
        let slot = match Arc::clone(&self.session_slots).try_acquire_owned() {
            Ok(slot) => Arc::new(slot),
            Err(_) => {
                return self
                    .browser_failure(&context, ToolError::LimitExceeded)
                    .await;
            }
        };
        let (target_id, websocket_url, browser_context_id) =
            match self.open_target(profile.profile_id).await {
                Ok(target) => target,
                Err(error) => return self.browser_failure(&context, error).await,
            };
        let session_id = Uuid::now_v7();
        self.sessions.lock().await.insert(
            session_id,
            BrowserSession {
                target_id,
                websocket_url,
                profile_id: profile.profile_id,
                browser_context_id,
                _slot: slot,
            },
        );
        self.emit(
            &context,
            ActivityStatus::Completed,
            "Opened browser session",
            Some(session_id.to_string()),
        )
        .await;
        Ok(BrowserResult::SessionCreated { session_id })
    }

    /// Executes one normalized browser action.
    ///
    /// # Errors
    ///
    /// Requires policy authorization and fails closed on invalid or oversized CDP data.
    pub async fn execute(
        &self,
        context: OperationContext,
        session_id: Uuid,
        action: BrowserAction,
        approval_id: Option<Uuid>,
    ) -> Result<BrowserResult, ToolError> {
        validate_action(&action)?;
        let session = self
            .sessions
            .lock()
            .await
            .get(&session_id)
            .cloned()
            .ok_or_else(|| ToolError::InvalidRequest("browser session was not found".to_owned()))?;
        let capability = match action {
            BrowserAction::CaptureScreenshot | BrowserAction::CurrentUrl => {
                CapabilityClass::BrowserObserve
            }
            BrowserAction::Navigate { .. } | BrowserAction::Evaluate { .. } => {
                CapabilityClass::BrowserAct
            }
        };
        let action_name = action_name(&action);
        self.verify_target_membership(&session).await?;
        let request = CapabilityRequest {
            context: context.clone(),
            capability,
            action: action_name.to_owned(),
            canonical_resource: format!(
                "browser-session:{session_id}:{action_name}:sha256:{:x}",
                Sha256::digest(
                    serde_json::to_vec(&action).map_err(|_| ToolError::OperationFailed)?
                )
            ),
            summary: action_summary(&action),
            destructive: capability == CapabilityClass::BrowserAct,
        };
        let _authorization = self.policy.authorize(&request, approval_id).await?;
        self.emit(
            &context,
            ActivityStatus::Started,
            "Running browser action",
            Some(action_name.to_owned()),
        )
        .await;
        let result = execute_cdp(&session.websocket_url, &action, self.request_timeout).await;
        match &result {
            Ok(_) => {
                self.emit(
                    &context,
                    ActivityStatus::Completed,
                    "Browser action completed",
                    Some(action_name.to_owned()),
                )
                .await;
            }
            Err(_) => {
                self.emit(
                    &context,
                    ActivityStatus::Failed,
                    "Browser action failed",
                    Some(action_name.to_owned()),
                )
                .await;
            }
        }
        result
    }

    /// Closes a page target and removes it from the active session registry.
    ///
    /// # Errors
    ///
    /// Requires policy authorization and a live local browser target.
    pub async fn close_session(
        &self,
        context: OperationContext,
        session_id: Uuid,
        approval_id: Option<Uuid>,
    ) -> Result<BrowserResult, ToolError> {
        let session = self
            .sessions
            .lock()
            .await
            .get(&session_id)
            .cloned()
            .ok_or_else(|| ToolError::InvalidRequest("browser session was not found".to_owned()))?;
        self.verify_target_membership(&session).await?;
        let request = CapabilityRequest {
            context: context.clone(),
            capability: CapabilityClass::BrowserAct,
            action: "browser.session.close".to_owned(),
            canonical_resource: format!("browser-session:{session_id}"),
            summary: "Close browser session".to_owned(),
            destructive: false,
        };
        let _authorization = self.policy.authorize(&request, approval_id).await?;
        self.emit(
            &context,
            ActivityStatus::Started,
            "Closing browser session",
            Some(session.profile_id.to_string()),
        )
        .await;
        let endpoint = self
            .endpoint
            .join(&format!("json/close/{}", session.target_id))
            .map_err(|_| ToolError::BrowserProtocol);
        let response = match endpoint {
            Ok(endpoint) => self
                .client
                .get(endpoint)
                .send()
                .await
                .map_err(|_| ToolError::Unavailable),
            Err(error) => Err(error),
        };
        let response = match response {
            Ok(response) => response,
            Err(error) => return self.browser_failure(&context, error).await,
        };
        if !response.status().is_success() {
            return self
                .browser_failure(&context, ToolError::BrowserProtocol)
                .await;
        }
        self.sessions.lock().await.remove(&session_id);
        self.emit(
            &context,
            ActivityStatus::Completed,
            "Closed browser session",
            Some(session.profile_id.to_string()),
        )
        .await;
        Ok(BrowserResult::SessionClosed)
    }

    fn validate_profile(&self, profile: &BrowserSessionProfile) -> Result<PathBuf, ToolError> {
        if profile.profile_directory.is_absolute()
            || profile
                .profile_directory
                .components()
                .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
        {
            return Err(ToolError::PathOutsideWorkspace);
        }
        let unresolved = self.profile_root.join(&profile.profile_directory);
        reject_profile_symlinks(&self.profile_root, &unresolved)?;
        let path =
            std::fs::canonicalize(unresolved).map_err(|_| ToolError::PathOutsideWorkspace)?;
        if !path.starts_with(&self.profile_root) || !path.is_dir() {
            return Err(ToolError::PathOutsideWorkspace);
        }
        Ok(path)
    }

    async fn open_target(&self, profile_id: Uuid) -> Result<(String, Url, String), ToolError> {
        let browser_websocket = self.browser_websocket_url().await?;
        let browser_context_id = {
            let mut contexts = self.profile_contexts.lock().await;
            if let Some(existing) = contexts.get(&profile_id) {
                existing.clone()
            } else {
                let response = send_browser_command(
                    &browser_websocket,
                    "Target.createBrowserContext",
                    json!({"disposeOnDetach": false}),
                    self.request_timeout,
                )
                .await?;
                let context_id = response
                    .pointer("/result/browserContextId")
                    .and_then(Value::as_str)
                    .ok_or(ToolError::BrowserProtocol)?
                    .to_owned();
                contexts.insert(profile_id, context_id.clone());
                context_id
            }
        };
        let response = send_browser_command(
            &browser_websocket,
            "Target.createTarget",
            json!({"url": "about:blank", "browserContextId": browser_context_id}),
            self.request_timeout,
        )
        .await?;
        let target_id = response
            .pointer("/result/targetId")
            .and_then(Value::as_str)
            .ok_or(ToolError::BrowserProtocol)?
            .to_owned();
        let target = self
            .targets()
            .await?
            .into_iter()
            .find(|target| target.id == target_id)
            .ok_or(ToolError::BrowserProtocol)?;
        if target.browser_context_id.as_deref() != Some(browser_context_id.as_str()) {
            return Err(ToolError::BrowserProtocol);
        }
        let websocket_url =
            Url::parse(&target.web_socket_debugger_url).map_err(|_| ToolError::BrowserProtocol)?;
        if !matches!(websocket_url.scheme(), "ws" | "wss") || !is_loopback(&websocket_url) {
            return Err(ToolError::BrowserProtocol);
        }
        Ok((target_id, websocket_url, browser_context_id))
    }

    async fn browser_websocket_url(&self) -> Result<Url, ToolError> {
        let endpoint = self
            .endpoint
            .join("json/version")
            .map_err(|_| ToolError::BrowserProtocol)?;
        let response = self
            .client
            .get(endpoint)
            .send()
            .await
            .map_err(|_| ToolError::Unavailable)?;
        if !response.status().is_success() {
            return Err(ToolError::BrowserProtocol);
        }
        let version: CdpBrowserVersion = response
            .json()
            .await
            .map_err(|_| ToolError::BrowserProtocol)?;
        let websocket_url =
            Url::parse(&version.web_socket_debugger_url).map_err(|_| ToolError::BrowserProtocol)?;
        if !matches!(websocket_url.scheme(), "ws" | "wss") || !is_loopback(&websocket_url) {
            return Err(ToolError::BrowserProtocol);
        }
        Ok(websocket_url)
    }

    async fn targets(&self) -> Result<Vec<CdpTarget>, ToolError> {
        let endpoint = self
            .endpoint
            .join("json/list")
            .map_err(|_| ToolError::BrowserProtocol)?;
        let response = self
            .client
            .get(endpoint)
            .send()
            .await
            .map_err(|_| ToolError::Unavailable)?;
        if !response.status().is_success() {
            return Err(ToolError::BrowserProtocol);
        }
        response
            .json()
            .await
            .map_err(|_| ToolError::BrowserProtocol)
    }

    async fn verify_target_membership(&self, session: &BrowserSession) -> Result<(), ToolError> {
        let target = self
            .targets()
            .await?
            .into_iter()
            .find(|target| target.id == session.target_id)
            .ok_or(ToolError::BrowserProtocol)?;
        if target.browser_context_id.as_deref() != Some(session.browser_context_id.as_str())
            || target.web_socket_debugger_url != session.websocket_url.as_str()
        {
            return Err(ToolError::BrowserProtocol);
        }
        Ok(())
    }

    async fn emit(
        &self,
        context: &OperationContext,
        status: ActivityStatus,
        title: &str,
        detail: Option<String>,
    ) {
        self.activities
            .emit(ToolActivity::new(
                context.operation_id,
                ActivityKind::Browser,
                status,
                title,
                detail,
            ))
            .await;
    }

    async fn browser_failure<T>(
        &self,
        context: &OperationContext,
        error: ToolError,
    ) -> Result<T, ToolError> {
        self.emit(
            context,
            ActivityStatus::Failed,
            "Browser session failed",
            None,
        )
        .await;
        Err(error)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CdpTarget {
    id: String,
    web_socket_debugger_url: String,
    #[serde(default)]
    browser_context_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CdpBrowserVersion {
    web_socket_debugger_url: String,
}

async fn send_browser_command(
    websocket_url: &Url,
    method: &str,
    params: Value,
    timeout: Duration,
) -> Result<Value, ToolError> {
    let (mut socket, _) = connect_async(websocket_url.as_str())
        .await
        .map_err(|_| ToolError::BrowserProtocol)?;
    socket
        .send(Message::Text(
            json!({"id": 1, "method": method, "params": params})
                .to_string()
                .into(),
        ))
        .await
        .map_err(|_| ToolError::BrowserProtocol)?;
    let response = tokio::time::timeout(timeout, async {
        for _ in 0..MAX_CDP_MESSAGES {
            let message = socket
                .next()
                .await
                .ok_or(ToolError::BrowserProtocol)?
                .map_err(|_| ToolError::BrowserProtocol)?;
            let data = match message {
                Message::Text(text) => text.as_bytes().to_vec(),
                Message::Binary(bytes) => bytes.to_vec(),
                Message::Close(_) => return Err(ToolError::BrowserProtocol),
                Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => continue,
            };
            if data.len() > MAX_CDP_MESSAGE_BYTES {
                return Err(ToolError::LimitExceeded);
            }
            let value: Value =
                serde_json::from_slice(&data).map_err(|_| ToolError::BrowserProtocol)?;
            if value.get("id").and_then(Value::as_u64) != Some(1) {
                continue;
            }
            if value.get("error").is_some() {
                return Err(ToolError::BrowserProtocol);
            }
            return Ok(value);
        }
        Err(ToolError::LimitExceeded)
    })
    .await
    .map_err(|_| ToolError::TimedOut)??;
    let _ = socket.close(None).await;
    Ok(response)
}

async fn execute_cdp(
    websocket_url: &Url,
    action: &BrowserAction,
    timeout: Duration,
) -> Result<BrowserResult, ToolError> {
    let (mut socket, _) = connect_async(websocket_url.as_str())
        .await
        .map_err(|_| ToolError::BrowserProtocol)?;
    let (method, params) = match action {
        BrowserAction::Navigate { url } => ("Page.navigate", json!({"url": url})),
        BrowserAction::Evaluate { expression } => (
            "Runtime.evaluate",
            json!({"expression": expression, "returnByValue": true, "awaitPromise": true}),
        ),
        BrowserAction::CaptureScreenshot => (
            "Page.captureScreenshot",
            json!({"format": "png", "fromSurface": true}),
        ),
        BrowserAction::CurrentUrl => (
            "Runtime.evaluate",
            json!({"expression": "location.href", "returnByValue": true}),
        ),
    };
    let request_id = 1_u64;
    socket
        .send(Message::Text(
            json!({"id": request_id, "method": method, "params": params})
                .to_string()
                .into(),
        ))
        .await
        .map_err(|_| ToolError::BrowserProtocol)?;
    let response = tokio::time::timeout(timeout, async {
        for _ in 0..MAX_CDP_MESSAGES {
            let message = socket
                .next()
                .await
                .ok_or(ToolError::BrowserProtocol)?
                .map_err(|_| ToolError::BrowserProtocol)?;
            let data = match message {
                Message::Text(text) => text.as_bytes().to_vec(),
                Message::Binary(bytes) => bytes.to_vec(),
                Message::Close(_) => return Err(ToolError::BrowserProtocol),
                Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => continue,
            };
            if data.len() > MAX_CDP_MESSAGE_BYTES {
                return Err(ToolError::LimitExceeded);
            }
            let value: Value =
                serde_json::from_slice(&data).map_err(|_| ToolError::BrowserProtocol)?;
            if value.get("id").and_then(Value::as_u64) != Some(request_id) {
                continue;
            }
            if value.get("error").is_some() {
                return Err(ToolError::BrowserProtocol);
            }
            return normalize_cdp_result(action, &value);
        }
        Err(ToolError::LimitExceeded)
    })
    .await
    .map_err(|_| ToolError::TimedOut)??;
    let _ = socket.close(None).await;
    Ok(response)
}

fn normalize_cdp_result(action: &BrowserAction, value: &Value) -> Result<BrowserResult, ToolError> {
    match action {
        BrowserAction::Navigate { .. } => Ok(BrowserResult::NavigationAccepted),
        BrowserAction::Evaluate { .. } => Ok(BrowserResult::Evaluation {
            value: value
                .pointer("/result/result/value")
                .cloned()
                .unwrap_or(Value::Null),
        }),
        BrowserAction::CaptureScreenshot => {
            let encoded = value
                .pointer("/result/data")
                .and_then(Value::as_str)
                .ok_or(ToolError::BrowserProtocol)?;
            let bytes = STANDARD
                .decode(encoded)
                .map_err(|_| ToolError::BrowserProtocol)?;
            if bytes.len() > MAX_CDP_MESSAGE_BYTES {
                return Err(ToolError::LimitExceeded);
            }
            Ok(BrowserResult::ScreenshotPng { bytes })
        }
        BrowserAction::CurrentUrl => Ok(BrowserResult::Url {
            url: value
                .pointer("/result/result/value")
                .and_then(Value::as_str)
                .ok_or(ToolError::BrowserProtocol)?
                .to_owned(),
        }),
    }
}

fn validate_action(action: &BrowserAction) -> Result<(), ToolError> {
    if let BrowserAction::Navigate { url } = action {
        let url =
            Url::parse(url).map_err(|_| ToolError::InvalidRequest("invalid URL".to_owned()))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(ToolError::InvalidRequest(
                "browser navigation requires HTTP or HTTPS".to_owned(),
            ));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(ToolError::InvalidRequest(
                "browser navigation URL must not contain credentials".to_owned(),
            ));
        }
    }
    if let BrowserAction::Evaluate { expression } = action
        && expression.len() > 256 * 1024
    {
        return Err(ToolError::LimitExceeded);
    }
    Ok(())
}

fn action_name(action: &BrowserAction) -> &'static str {
    match action {
        BrowserAction::Navigate { .. } => "browser.navigate",
        BrowserAction::Evaluate { .. } => "browser.evaluate",
        BrowserAction::CaptureScreenshot => "browser.screenshot",
        BrowserAction::CurrentUrl => "browser.current_url",
    }
}

fn action_summary(action: &BrowserAction) -> String {
    match action {
        BrowserAction::Navigate { url } => Url::parse(url).map_or_else(
            |_| "Navigate browser".to_owned(),
            |url| {
                format!(
                    "Navigate browser to {}://{}{}",
                    url.scheme(),
                    url.host_str().unwrap_or("site"),
                    url.path()
                )
            },
        ),
        BrowserAction::Evaluate { .. } => "Evaluate browser expression".to_owned(),
        BrowserAction::CaptureScreenshot => "Capture browser screenshot".to_owned(),
        BrowserAction::CurrentUrl => "Read current browser URL".to_owned(),
    }
}

fn is_loopback(url: &Url) -> bool {
    url.host_str()
        .is_some_and(|host| matches!(host, "localhost" | "127.0.0.1" | "[::1]" | "::1"))
}

fn reject_profile_symlinks(root: &Path, target: &Path) -> Result<(), ToolError> {
    let relative = target
        .strip_prefix(root)
        .map_err(|_| ToolError::PathOutsideWorkspace)?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        if std::fs::symlink_metadata(&current)
            .map_err(|_| ToolError::OperationFailed)?
            .file_type()
            .is_symlink()
        {
            return Err(ToolError::SymlinkRejected);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PolicyEffect, PolicyRule, RecordingActivitySink};
    use axum::{Json, Router, extract::State, routing::get};
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        time::Duration,
    };
    use tokio::net::TcpListener;
    use tokio_tungstenite::accept_async;

    #[derive(Default)]
    struct FakeCdpState {
        next_context: AtomicUsize,
        targets: Mutex<Vec<(String, String)>>,
    }

    async fn fake_cdp_socket(listener: TcpListener, state: Arc<FakeCdpState>) {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let state = state.clone();
            tokio::spawn(async move {
                let Ok(mut socket) = accept_async(stream).await else {
                    return;
                };
                let Some(Ok(Message::Text(request))) = socket.next().await else {
                    return;
                };
                let Ok(request): Result<Value, _> = serde_json::from_str(request.as_str()) else {
                    return;
                };
                let method = request.get("method").and_then(Value::as_str).unwrap_or("");
                let result = match method {
                    "Target.createBrowserContext" => {
                        let number = state.next_context.fetch_add(1, Ordering::SeqCst) + 1;
                        json!({"browserContextId": format!("context-{number}")})
                    }
                    "Target.createTarget" => {
                        let context = request
                            .pointer("/params/browserContextId")
                            .and_then(Value::as_str)
                            .unwrap_or("missing")
                            .to_owned();
                        let target = format!("target-{context}");
                        state.targets.lock().await.push((target.clone(), context));
                        json!({"targetId": target})
                    }
                    "Page.navigate" => json!({"frameId": "frame"}),
                    "Page.captureScreenshot" => json!({"data": STANDARD.encode(b"png")}),
                    "Runtime.evaluate" => json!({"result": {"value": "https://example.test/"}}),
                    _ => json!({}),
                };
                let _ = socket
                    .send(Message::Text(
                        json!({"id": 1, "result": result}).to_string().into(),
                    ))
                    .await;
            });
        }
    }

    fn context() -> OperationContext {
        OperationContext {
            operation_id: Uuid::now_v7(),
            owner_id: Uuid::now_v7(),
            device_id: Uuid::now_v7(),
            bot_id: Uuid::now_v7(),
            chat_id: Uuid::now_v7(),
            workspace_id: Uuid::now_v7(),
        }
    }

    async fn browser_fixture()
    -> Result<(BrowserService, tempfile::TempDir), Box<dyn std::error::Error>> {
        let websocket_listener = TcpListener::bind("127.0.0.1:0").await?;
        let websocket_address = websocket_listener.local_addr()?;
        let cdp_state = Arc::new(FakeCdpState::default());
        tokio::spawn(fake_cdp_socket(websocket_listener, cdp_state.clone()));
        let websocket_url = format!("ws://{websocket_address}/devtools/page/fixture");
        let app = Router::new()
            .route(
                "/json/version",
                get(
                    |State((url, _)): State<(String, Arc<FakeCdpState>)>| async move {
                        Json(json!({"webSocketDebuggerUrl": url}))
                    },
                ),
            )
            .route(
                "/json/list",
                get(
                    |State((url, state)): State<(String, Arc<FakeCdpState>)>| async move {
                        let targets = state.targets.lock().await;
                        Json(Value::Array(
                            targets
                                .iter()
                                .map(|(id, context)| {
                                    json!({
                                        "id": id,
                                        "webSocketDebuggerUrl": url,
                                        "browserContextId": context,
                                    })
                                })
                                .collect(),
                        ))
                    },
                ),
            )
            .route("/json/close/{id}", get(|| async { "Target is closing" }))
            .with_state((websocket_url, cdp_state));
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        let profile_root = tempfile::tempdir()?;
        let activities = Arc::new(RecordingActivitySink::default());
        let policy = Arc::new(PolicyEngine::new(
            Duration::from_secs(60),
            activities.clone(),
        ));
        policy
            .replace_rules(vec![
                PolicyRule::new(CapabilityClass::BrowserAct, PolicyEffect::Allow),
                PolicyRule::new(CapabilityClass::BrowserObserve, PolicyEffect::Allow),
            ])
            .await;
        let service = BrowserService::new(
            Url::parse(&format!("http://{address}/"))?,
            profile_root.path(),
            policy,
            activities,
        )?;
        Ok((service, profile_root))
    }

    #[tokio::test]
    async fn local_cdp_session_executes_normalized_actions()
    -> Result<(), Box<dyn std::error::Error>> {
        let (service, profile_root) = browser_fixture().await?;
        std::fs::create_dir(profile_root.path().join("default"))?;
        let BrowserResult::SessionCreated { session_id } = service
            .create_session(
                context(),
                &BrowserSessionProfile {
                    profile_id: Uuid::now_v7(),
                    display_name: "Default".to_owned(),
                    profile_directory: PathBuf::from("default"),
                },
                None,
            )
            .await?
        else {
            return Err("session was not created".into());
        };
        assert_eq!(
            service
                .execute(
                    context(),
                    session_id,
                    BrowserAction::Navigate {
                        url: "https://example.test/".to_owned(),
                    },
                    None,
                )
                .await?,
            BrowserResult::NavigationAccepted
        );
        std::fs::create_dir(profile_root.path().join("second"))?;
        let BrowserResult::SessionCreated {
            session_id: second_session,
        } = service
            .create_session(
                context(),
                &BrowserSessionProfile {
                    profile_id: Uuid::now_v7(),
                    display_name: "Second".to_owned(),
                    profile_directory: PathBuf::from("second"),
                },
                None,
            )
            .await?
        else {
            return Err("second session was not created".into());
        };
        let sessions = service.sessions.lock().await;
        assert_ne!(
            sessions[&session_id].browser_context_id,
            sessions[&second_session].browser_context_id,
        );
        drop(sessions);
        assert_eq!(
            service
                .execute(
                    context(),
                    session_id,
                    BrowserAction::CaptureScreenshot,
                    None,
                )
                .await?,
            BrowserResult::ScreenshotPng {
                bytes: b"png".to_vec()
            }
        );
        assert_eq!(
            service.close_session(context(), session_id, None).await?,
            BrowserResult::SessionClosed
        );
        Ok(())
    }

    #[test]
    fn remote_browser_control_and_unsafe_navigation_are_rejected()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let activities = Arc::new(RecordingActivitySink::default());
        let policy = Arc::new(PolicyEngine::new(
            Duration::from_secs(60),
            activities.clone(),
        ));
        assert!(matches!(
            BrowserService::new(
                Url::parse("http://example.com:9222/")?,
                root.path(),
                policy,
                activities,
            ),
            Err(ToolError::InvalidRequest(_))
        ));
        assert!(matches!(
            validate_action(&BrowserAction::Navigate {
                url: "file:///etc/passwd".to_owned()
            }),
            Err(ToolError::InvalidRequest(_))
        ));
        assert!(matches!(
            validate_action(&BrowserAction::Navigate {
                url: "https://user:password@example.com/".to_owned()
            }),
            Err(ToolError::InvalidRequest(_))
        ));
        Ok(())
    }
}
