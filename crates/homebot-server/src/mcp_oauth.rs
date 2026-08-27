//! OAuth 2.1 authorization-code support for remote MCP transports.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use futures_util::StreamExt as _;
use getrandom::fill as fill_random;
use reqwest::{Client, StatusCode, header::WWW_AUTHENTICATE};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};
use url::Url;

const MAX_METADATA_BYTES: usize = 256 * 1024;
const MAX_TOKEN_BYTES: usize = 64 * 1024;
const MAX_AUTHORIZATION_URL_BYTES: usize = 8 * 1024;

#[derive(Debug, thiserror::Error)]
pub(super) enum OAuthError {
    #[error("the MCP authorization metadata is invalid or unavailable")]
    Metadata,
    #[error("the MCP authorization server does not support PKCE S256")]
    PkceRequired,
    #[error("the MCP authorization server does not support dynamic client registration")]
    RegistrationUnsupported,
    #[error("the MCP OAuth redirect URI must use HTTPS or loopback HTTP")]
    RedirectUri,
    #[error("the MCP OAuth request was rejected")]
    Rejected,
    #[error("the MCP OAuth response is too large")]
    ResponseTooLarge,
    #[error("the MCP OAuth token is invalid")]
    Token,
}

#[derive(Clone)]
pub(super) struct PendingFlow {
    pub plugin_id: uuid::Uuid,
    state: String,
    verifier: String,
    redirect_uri: Url,
    token_endpoint: Url,
    client_id: String,
    client_secret: Option<String>,
    token_endpoint_auth_method: String,
    resource: Url,
    scope: Option<String>,
    created_at_ms: u64,
}

impl PendingFlow {
    pub(super) fn state(&self) -> &str {
        &self.state
    }

    pub(super) fn expired(&self, now_ms: u64) -> bool {
        now_ms.saturating_sub(self.created_at_ms) > 10 * 60 * 1_000
    }
}

pub(super) struct AuthorizationStart {
    pub authorization_url: Url,
    pub flow: PendingFlow,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OAuthTokenBundle {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at_ms: Option<u64>,
    pub scope: Option<String>,
    pub token_endpoint: String,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub token_endpoint_auth_method: String,
    pub resource: String,
}

impl OAuthTokenBundle {
    pub(super) fn needs_refresh(&self, now_ms: u64) -> bool {
        self.expires_at_ms
            .is_some_and(|expires| expires <= now_ms.saturating_add(30_000))
    }
}

#[derive(Deserialize)]
struct ProtectedResourceMetadata {
    resource: String,
    authorization_servers: Vec<String>,
    #[serde(default)]
    scopes_supported: Vec<String>,
}

#[derive(Deserialize)]
struct AuthorizationServerMetadata {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    #[serde(default)]
    registration_endpoint: Option<String>,
    #[serde(default)]
    code_challenge_methods_supported: Vec<String>,
}

#[derive(Deserialize)]
struct RegistrationResponse {
    client_id: String,
    #[serde(default)]
    client_secret: Option<String>,
    #[serde(default = "default_token_auth_method")]
    token_endpoint_auth_method: String,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    token_type: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    scope: Option<String>,
}

fn default_token_auth_method() -> String {
    "none".to_owned()
}

pub(super) fn client() -> Result<Client, OAuthError> {
    Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("HomeBot/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| OAuthError::Metadata)
}

pub(super) async fn begin(
    client: &Client,
    plugin_id: uuid::Uuid,
    endpoint: Url,
    redirect_uri: Url,
) -> Result<AuthorizationStart, OAuthError> {
    validate_redirect_uri(&redirect_uri)?;
    let challenge = authorization_challenge(client, &endpoint).await?;
    let resource_metadata =
        discover_resource_metadata(client, &endpoint, challenge.resource_metadata.as_deref())
            .await?;
    let resource = validated_url(&resource_metadata.resource, endpoint.scheme() == "http")?;
    if canonical_resource(&resource) != canonical_resource(&endpoint) {
        return Err(OAuthError::Metadata);
    }
    let authorization_server = resource_metadata
        .authorization_servers
        .first()
        .ok_or(OAuthError::Metadata)
        .and_then(|value| validated_url(value, endpoint.scheme() == "http"))?;
    if authorization_server.query().is_some() {
        return Err(OAuthError::Metadata);
    }
    let metadata = discover_authorization_server(client, &authorization_server).await?;
    let discovered_issuer = validated_url(&metadata.issuer, false)?;
    if canonical_resource(&discovered_issuer) != canonical_resource(&authorization_server) {
        return Err(OAuthError::Metadata);
    }
    if !metadata
        .code_challenge_methods_supported
        .iter()
        .any(|method| method == "S256")
    {
        return Err(OAuthError::PkceRequired);
    }
    let authorization_endpoint = validated_url(&metadata.authorization_endpoint, false)?;
    let token_endpoint = validated_url(&metadata.token_endpoint, false)?;
    let registration_endpoint = metadata
        .registration_endpoint
        .as_deref()
        .ok_or(OAuthError::RegistrationUnsupported)
        .and_then(|value| validated_url(value, false))?;
    let registration: RegistrationResponse = bounded_json(
        client
            .post(registration_endpoint)
            .json(&serde_json::json!({
                "client_name": "HomeBot",
                "redirect_uris": [redirect_uri.as_str()],
                "grant_types": ["authorization_code", "refresh_token"],
                "response_types": ["code"],
                "token_endpoint_auth_method": "none"
            }))
            .send()
            .await
            .map_err(|_| OAuthError::Metadata)?,
        MAX_METADATA_BYTES,
    )
    .await?;
    validate_client(&registration)?;

    let state = random_urlsafe(32)?;
    let verifier = random_urlsafe(32)?;
    let challenge_value = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let scope = challenge.scope.or_else(|| {
        (!resource_metadata.scopes_supported.is_empty())
            .then(|| resource_metadata.scopes_supported.join(" "))
    });
    let mut authorization_url = authorization_endpoint;
    {
        let mut query = authorization_url.query_pairs_mut();
        query
            .append_pair("response_type", "code")
            .append_pair("client_id", &registration.client_id)
            .append_pair("redirect_uri", redirect_uri.as_str())
            .append_pair("code_challenge", &challenge_value)
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", &state)
            .append_pair("resource", resource.as_str());
        if let Some(scope) = &scope {
            query.append_pair("scope", scope);
        }
    }
    if authorization_url.as_str().len() > MAX_AUTHORIZATION_URL_BYTES {
        return Err(OAuthError::Metadata);
    }
    Ok(AuthorizationStart {
        authorization_url,
        flow: PendingFlow {
            plugin_id,
            state,
            verifier,
            redirect_uri,
            token_endpoint,
            client_id: registration.client_id,
            client_secret: registration.client_secret,
            token_endpoint_auth_method: registration.token_endpoint_auth_method,
            resource,
            scope,
            created_at_ms: now_ms(),
        },
    })
}

pub(super) async fn finish(
    client: &Client,
    flow: PendingFlow,
    code: &str,
) -> Result<OAuthTokenBundle, OAuthError> {
    validate_short_secret(code)?;
    let mut form = vec![
        ("grant_type", "authorization_code".to_owned()),
        ("code", code.to_owned()),
        ("redirect_uri", flow.redirect_uri.to_string()),
        ("client_id", flow.client_id.clone()),
        ("code_verifier", flow.verifier),
        ("resource", flow.resource.to_string()),
    ];
    add_client_auth(
        &mut form,
        &flow.token_endpoint_auth_method,
        flow.client_secret.as_deref(),
    )?;
    let token: TokenResponse = bounded_json(
        client
            .post(flow.token_endpoint.clone())
            .form(&form)
            .send()
            .await
            .map_err(|_| OAuthError::Rejected)?,
        MAX_TOKEN_BYTES,
    )
    .await?;
    token_bundle(
        token,
        &flow.token_endpoint,
        flow.client_id,
        flow.client_secret,
        flow.token_endpoint_auth_method,
        &flow.resource,
        flow.scope,
    )
}

pub(super) async fn refresh(
    client: &Client,
    bundle: &OAuthTokenBundle,
) -> Result<OAuthTokenBundle, OAuthError> {
    let refresh_token = bundle.refresh_token.as_deref().ok_or(OAuthError::Token)?;
    validate_short_secret(refresh_token)?;
    let mut form = vec![
        ("grant_type", "refresh_token".to_owned()),
        ("refresh_token", refresh_token.to_owned()),
        ("client_id", bundle.client_id.clone()),
        ("resource", bundle.resource.clone()),
    ];
    if let Some(scope) = &bundle.scope {
        form.push(("scope", scope.clone()));
    }
    add_client_auth(
        &mut form,
        &bundle.token_endpoint_auth_method,
        bundle.client_secret.as_deref(),
    )?;
    let token: TokenResponse = bounded_json(
        client
            .post(validated_url(&bundle.token_endpoint, false)?)
            .form(&form)
            .send()
            .await
            .map_err(|_| OAuthError::Rejected)?,
        MAX_TOKEN_BYTES,
    )
    .await?;
    let token_endpoint = validated_url(&bundle.token_endpoint, false)?;
    let resource = validated_url(&bundle.resource, true)?;
    let mut refreshed = token_bundle(
        token,
        &token_endpoint,
        bundle.client_id.clone(),
        bundle.client_secret.clone(),
        bundle.token_endpoint_auth_method.clone(),
        &resource,
        bundle.scope.clone(),
    )?;
    if refreshed.refresh_token.is_none() {
        refreshed.refresh_token.clone_from(&bundle.refresh_token);
    }
    Ok(refreshed)
}

fn token_bundle(
    token: TokenResponse,
    token_endpoint: &Url,
    client_id: String,
    client_secret: Option<String>,
    token_endpoint_auth_method: String,
    resource: &Url,
    fallback_scope: Option<String>,
) -> Result<OAuthTokenBundle, OAuthError> {
    if !token.token_type.eq_ignore_ascii_case("bearer") {
        return Err(OAuthError::Token);
    }
    validate_short_secret(&token.access_token)?;
    if let Some(refresh) = &token.refresh_token {
        validate_short_secret(refresh)?;
    }
    let expires_at_ms = token
        .expires_in
        .map(|seconds| {
            (seconds <= 31_536_000)
                .then(|| now_ms().saturating_add(seconds.saturating_mul(1_000)))
                .ok_or(OAuthError::Token)
        })
        .transpose()?;
    Ok(OAuthTokenBundle {
        access_token: token.access_token,
        refresh_token: token.refresh_token,
        expires_at_ms,
        scope: token.scope.or(fallback_scope),
        token_endpoint: token_endpoint.to_string(),
        client_id,
        client_secret,
        token_endpoint_auth_method,
        resource: resource.to_string(),
    })
}

fn add_client_auth(
    form: &mut Vec<(&'static str, String)>,
    method: &str,
    client_secret: Option<&str>,
) -> Result<(), OAuthError> {
    match (method, client_secret) {
        ("none", None) => Ok(()),
        ("client_secret_post", Some(secret)) => {
            validate_short_secret(secret)?;
            form.push(("client_secret", secret.to_owned()));
            Ok(())
        }
        _ => Err(OAuthError::Metadata),
    }
}

fn validate_client(registration: &RegistrationResponse) -> Result<(), OAuthError> {
    validate_short_secret(&registration.client_id)?;
    if let Some(secret) = &registration.client_secret {
        validate_short_secret(secret)?;
    }
    match (
        registration.token_endpoint_auth_method.as_str(),
        registration.client_secret.is_some(),
    ) {
        ("none", false) | ("client_secret_post", true) => Ok(()),
        _ => Err(OAuthError::Metadata),
    }
}

async fn authorization_challenge(client: &Client, endpoint: &Url) -> Result<Challenge, OAuthError> {
    let response = client
        .post(endpoint.clone())
        .header("accept", "application/json, text/event-stream")
        .header("content-type", "application/json")
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "HomeBot", "version": env!("CARGO_PKG_VERSION")}
            }
        }))
        .send()
        .await
        .map_err(|_| OAuthError::Metadata)?;
    if response.status() != StatusCode::UNAUTHORIZED {
        return Err(OAuthError::Metadata);
    }
    let header = response
        .headers()
        .get(WWW_AUTHENTICATE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    Ok(Challenge {
        resource_metadata: challenge_parameter(header, "resource_metadata"),
        scope: challenge_parameter(header, "scope"),
    })
}

struct Challenge {
    resource_metadata: Option<String>,
    scope: Option<String>,
}

fn challenge_parameter(header: &str, name: &str) -> Option<String> {
    if header.len() > 8_192 || header.chars().any(char::is_control) {
        return None;
    }
    header.split(',').find_map(|part| {
        let (key, value) = part.trim().split_once('=')?;
        (key.trim().trim_start_matches("Bearer ") == name).then(|| {
            value
                .trim()
                .strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
                .unwrap_or(value.trim())
                .to_owned()
        })
    })
}

async fn discover_resource_metadata(
    client: &Client,
    endpoint: &Url,
    advertised: Option<&str>,
) -> Result<ProtectedResourceMetadata, OAuthError> {
    let mut candidates = Vec::new();
    if let Some(advertised) = advertised {
        candidates.push(validated_url(advertised, endpoint.scheme() == "http")?);
    } else {
        let mut path = endpoint.clone();
        path.set_path(&format!(
            "/.well-known/oauth-protected-resource{}",
            endpoint.path()
        ));
        candidates.push(path);
        let mut root = endpoint.clone();
        root.set_path("/.well-known/oauth-protected-resource");
        if root != candidates[0] {
            candidates.push(root);
        }
    }
    discover_json(client, candidates).await
}

async fn discover_authorization_server(
    client: &Client,
    issuer: &Url,
) -> Result<AuthorizationServerMetadata, OAuthError> {
    let path = issuer.path().trim_matches('/');
    let mut candidates = Vec::new();
    for well_known in ["oauth-authorization-server", "openid-configuration"] {
        let mut candidate = issuer.clone();
        let candidate_path = if path.is_empty() {
            if well_known == "oauth-authorization-server" {
                "/.well-known/oauth-authorization-server".to_owned()
            } else {
                "/.well-known/openid-configuration".to_owned()
            }
        } else {
            format!("/.well-known/{well_known}/{path}")
        };
        candidate.set_path(&candidate_path);
        candidates.push(candidate);
    }
    if !path.is_empty() {
        let mut appended = issuer.clone();
        appended.set_path(&format!("/{path}/.well-known/openid-configuration"));
        candidates.push(appended);
    }
    discover_json(client, candidates).await
}

async fn discover_json<T: DeserializeOwned>(
    client: &Client,
    candidates: Vec<Url>,
) -> Result<T, OAuthError> {
    for candidate in candidates {
        let Ok(response) = client.get(candidate).send().await else {
            continue;
        };
        if !response.status().is_success() {
            continue;
        }
        if let Ok(value) = bounded_json(response, MAX_METADATA_BYTES).await {
            return Ok(value);
        }
    }
    Err(OAuthError::Metadata)
}

async fn bounded_json<T: DeserializeOwned>(
    response: reqwest::Response,
    limit: usize,
) -> Result<T, OAuthError> {
    if !response.status().is_success() {
        return Err(OAuthError::Rejected);
    }
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(OAuthError::ResponseTooLarge);
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| OAuthError::Rejected)?;
        if bytes.len().saturating_add(chunk.len()) > limit {
            return Err(OAuthError::ResponseTooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| OAuthError::Metadata)
}

fn validate_redirect_uri(uri: &Url) -> Result<(), OAuthError> {
    let loopback = uri.host_str().is_some_and(is_loopback_host);
    if (uri.scheme() != "https" && !(uri.scheme() == "http" && loopback))
        || !uri.username().is_empty()
        || uri.password().is_some()
        || uri.query().is_some()
        || uri.fragment().is_some()
    {
        return Err(OAuthError::RedirectUri);
    }
    Ok(())
}

fn validated_url(value: &str, allow_loopback_http: bool) -> Result<Url, OAuthError> {
    let url = Url::parse(value).map_err(|_| OAuthError::Metadata)?;
    let loopback = url.host_str().is_some_and(is_loopback_host);
    if (url.scheme() != "https" && !(allow_loopback_http && url.scheme() == "http" && loopback))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(OAuthError::Metadata);
    }
    Ok(url)
}

fn is_loopback_host(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "[::1]" | "::1")
}

fn canonical_resource(url: &Url) -> String {
    let mut canonical = url.clone();
    canonical.set_query(None);
    canonical.set_fragment(None);
    if canonical.path() == "/" {
        canonical.set_path("");
    }
    canonical.to_string()
}

fn validate_short_secret(value: &str) -> Result<(), OAuthError> {
    if value.is_empty() || value.len() > 16_384 || value.chars().any(char::is_control) {
        Err(OAuthError::Token)
    } else {
        Ok(())
    }
}

fn random_urlsafe(length: usize) -> Result<String, OAuthError> {
    let mut bytes = vec![0_u8; length];
    fill_random(&mut bytes).map_err(|_| OAuthError::Rejected)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

pub(super) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenge_parser_and_redirect_policy_are_strict() -> Result<(), url::ParseError> {
        let challenge = r#"Bearer resource_metadata="https://mcp.example/.well-known/oauth-protected-resource", scope="memory:read memory:write""#;
        assert_eq!(
            challenge_parameter(challenge, "resource_metadata").as_deref(),
            Some("https://mcp.example/.well-known/oauth-protected-resource")
        );
        assert_eq!(
            challenge_parameter(challenge, "scope").as_deref(),
            Some("memory:read memory:write")
        );
        assert!(validate_redirect_uri(&Url::parse("http://127.0.0.1:7123/callback")?).is_ok());
        assert!(validate_redirect_uri(&Url::parse("http://192.168.1.2/callback")?).is_err());
        assert!(validate_redirect_uri(&Url::parse("https://homebot.example/callback")?).is_ok());
        Ok(())
    }

    #[test]
    fn expiring_tokens_refresh_before_use() {
        let bundle = OAuthTokenBundle {
            access_token: "access".to_owned(),
            refresh_token: Some("refresh".to_owned()),
            expires_at_ms: Some(60_000),
            scope: None,
            token_endpoint: "https://auth.example/token".to_owned(),
            client_id: "homebot".to_owned(),
            client_secret: None,
            token_endpoint_auth_method: "none".to_owned(),
            resource: "https://mcp.example/mcp".to_owned(),
        };
        assert!(!bundle.needs_refresh(29_999));
        assert!(bundle.needs_refresh(30_000));
    }

    #[test]
    fn token_boundary_rejects_non_bearer_and_excessive_lifetime() -> Result<(), url::ParseError> {
        let endpoint = Url::parse("https://auth.example/token")?;
        let resource = Url::parse("https://mcp.example/mcp")?;
        let rejected_type = token_bundle(
            TokenResponse {
                access_token: "access".to_owned(),
                token_type: "MAC".to_owned(),
                refresh_token: None,
                expires_in: Some(60),
                scope: None,
            },
            &endpoint,
            "homebot".to_owned(),
            None,
            "none".to_owned(),
            &resource,
            None,
        );
        assert!(matches!(rejected_type, Err(OAuthError::Token)));
        let rejected_lifetime = token_bundle(
            TokenResponse {
                access_token: "access".to_owned(),
                token_type: "Bearer".to_owned(),
                refresh_token: None,
                expires_in: Some(31_536_001),
                scope: None,
            },
            &endpoint,
            "homebot".to_owned(),
            None,
            "none".to_owned(),
            &resource,
            None,
        );
        assert!(matches!(rejected_lifetime, Err(OAuthError::Token)));
        Ok(())
    }
}
