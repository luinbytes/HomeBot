use axum::{
    Extension, Json,
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, header},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use homebot_protocol::{
    CreatePairingRequest, DeviceSessionSummary, ExchangePairingRequest, PairingEndpointKind,
    PairingExchangeResponse, PairingOffer, RevokeDeviceSessionRequest,
};
use homebot_storage::DeviceSessionRecord;
use sha2::{Digest, Sha256};
use std::net::{IpAddr, SocketAddr};
use url::{Host, Url};
use uuid::Uuid;

use crate::{
    AppState, AuthenticatedIdentity,
    bots::{ApiError, claim},
    unix_time_ms,
};

const PAIRING_TTL_MS: i64 = 5 * 60 * 1_000;
const PAIRING_RATE_WINDOW_MS: i64 = 60 * 1_000;

pub(super) async fn create(
    State(state): State<AppState>,
    Extension(identity): Extension<AuthenticatedIdentity>,
    Json(request): Json<CreatePairingRequest>,
) -> Result<(HeaderMap, Json<PairingOffer>), ApiError> {
    require_owner(&identity)?;
    let endpoint = validate_endpoint(&request.endpoint, request.allow_insecure_private_network)?;
    let token = random_token("hbpair")?;
    let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
    let now = unix_time_ms();
    let expires = now.saturating_add(PAIRING_TTL_MS);
    let id = Uuid::now_v7();
    state
        .storage
        .create_pairing_credential(
            state.owner_id,
            id,
            &digest,
            &endpoint.endpoint,
            &endpoint.origin,
            endpoint_kind_name(endpoint.kind),
            now,
            expires,
        )
        .await?;
    let mut deep_link = Url::parse("homebot://pair").map_err(|_| ApiError::internal())?;
    deep_link
        .query_pairs_mut()
        .append_pair("offer", &id.to_string())
        .append_pair("endpoint", &endpoint.endpoint)
        .append_pair("token", &token);
    let offer = PairingOffer {
        id,
        endpoint: endpoint.endpoint,
        endpoint_kind: endpoint.kind,
        pairing_token: token,
        deep_link: deep_link.to_string(),
        expires_at_unix_ms: timestamp(expires)?,
        warning: endpoint.warning,
    };
    Ok((no_store_headers(), Json(offer)))
}

pub(super) async fn exchange(
    State(state): State<AppState>,
    ConnectInfo(connection): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<ExchangePairingRequest>,
) -> Result<(HeaderMap, Json<PairingExchangeResponse>), ApiError> {
    if !request.pairing_token.starts_with("hbpair_") || request.pairing_token.len() > 128 {
        return Err(homebot_storage::StorageError::PairingNotFound.into());
    }
    let origin = normalized_origin(&headers)?;
    let session = random_token("hbds")?;
    let pairing_digest: [u8; 32] = Sha256::digest(request.pairing_token.as_bytes()).into();
    let source_digest = pairing_source_digest(connection.ip());
    let session_digest: [u8; 32] = Sha256::digest(session.as_bytes()).into();
    let device = state
        .storage
        .exchange_pairing_credential(
            state.owner_id,
            &pairing_digest,
            request.offer_id,
            &request.endpoint,
            origin.as_deref(),
            &source_digest,
            Uuid::now_v7(),
            &request.device_name,
            &session_digest,
            unix_time_ms(),
            PAIRING_RATE_WINDOW_MS,
        )
        .await?;
    Ok((
        no_store_headers(),
        Json(PairingExchangeResponse {
            device: device_summary(device)?,
            device_session: session,
        }),
    ))
}

fn pairing_source_digest(source: IpAddr) -> [u8; 32] {
    // Hash the network identity so durable throttling does not retain client addresses.
    Sha256::digest(source.to_string().as_bytes()).into()
}

pub(super) async fn list_devices(
    State(state): State<AppState>,
    Extension(identity): Extension<AuthenticatedIdentity>,
) -> Result<Json<Vec<DeviceSessionSummary>>, ApiError> {
    require_owner(&identity)?;
    let devices = state
        .storage
        .device_sessions(state.owner_id)
        .await?
        .into_iter()
        .map(device_summary)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Json(devices))
}

pub(super) async fn current_device(
    State(state): State<AppState>,
    Extension(identity): Extension<AuthenticatedIdentity>,
) -> Result<Json<DeviceSessionSummary>, ApiError> {
    let AuthenticatedIdentity::Device { id } = identity else {
        return Err(ApiError::forbidden(
            "This endpoint describes the authenticated paired device",
        ));
    };
    let device = state
        .storage
        .device_sessions(state.owner_id)
        .await?
        .into_iter()
        .find(|device| device.id == id)
        .ok_or_else(ApiError::internal)?;
    Ok(Json(device_summary(device)?))
}

pub(super) async fn revoke_current_device(
    State(state): State<AppState>,
    Extension(identity): Extension<AuthenticatedIdentity>,
    Json(request): Json<RevokeDeviceSessionRequest>,
) -> Result<Json<DeviceSessionSummary>, ApiError> {
    let AuthenticatedIdentity::Device { id } = identity else {
        return Err(ApiError::forbidden(
            "This endpoint revokes only the authenticated paired device",
        ));
    };
    let _claim = claim(
        &state,
        request.idempotency_key,
        &format!("revoke_current_device_session:{id}"),
        &request,
    )
    .await?;
    let device = state
        .storage
        .revoke_device_session(state.owner_id, id, unix_time_ms())
        .await?;
    Ok(Json(device_summary(device)?))
}

pub(super) async fn revoke_device(
    State(state): State<AppState>,
    Extension(identity): Extension<AuthenticatedIdentity>,
    Path(device_id): Path<Uuid>,
    Json(request): Json<RevokeDeviceSessionRequest>,
) -> Result<Json<DeviceSessionSummary>, ApiError> {
    require_owner(&identity)?;
    let _claim = claim(
        &state,
        request.idempotency_key,
        &format!("revoke_device_session:{device_id}"),
        &request,
    )
    .await?;
    let device = state
        .storage
        .revoke_device_session(state.owner_id, device_id, unix_time_ms())
        .await?;
    Ok(Json(device_summary(device)?))
}

fn require_owner(identity: &AuthenticatedIdentity) -> Result<(), ApiError> {
    if identity == &AuthenticatedIdentity::Owner {
        Ok(())
    } else {
        Err(ApiError::forbidden(
            "Only the HomeBot owner can manage pairing and device sessions",
        ))
    }
}

fn random_token(prefix: &str) -> Result<String, ApiError> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| ApiError::internal())?;
    Ok(format!("{prefix}_{}", URL_SAFE_NO_PAD.encode(bytes)))
}

#[derive(Debug)]
struct ValidatedEndpoint {
    endpoint: String,
    origin: String,
    kind: PairingEndpointKind,
    warning: Option<String>,
}

fn validate_endpoint(
    raw: &str,
    allow_insecure_private: bool,
) -> Result<ValidatedEndpoint, ApiError> {
    let mut url = Url::parse(raw.trim())
        .map_err(|_| ApiError::validation("Pairing endpoint must be an absolute URL"))?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "" | "/")
    {
        return Err(ApiError::validation(
            "Pairing endpoint cannot include credentials, a path, query, or fragment",
        ));
    }
    let host = url
        .host()
        .ok_or_else(|| ApiError::validation("Pairing endpoint must include a host"))?;
    let kind = classify_host(&host);
    let scheme = url.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(ApiError::validation(
            "Pairing endpoint must use HTTP or HTTPS",
        ));
    }
    let warning = if scheme == "http" && kind != PairingEndpointKind::Loopback {
        if kind == PairingEndpointKind::CustomHttps {
            return Err(ApiError::validation(
                "Public and custom pairing endpoints require HTTPS",
            ));
        }
        if !allow_insecure_private {
            return Err(ApiError::validation(
                "Plain HTTP on LAN or Tailscale requires explicit acknowledgement",
            ));
        }
        Some(
            "This private-network endpoint uses plain HTTP. Prefer HTTPS when the network is not fully trusted."
                .to_owned(),
        )
    } else {
        None
    };
    url.set_path("");
    let origin = url.origin().ascii_serialization();
    let endpoint = url.as_str().trim_end_matches('/').to_owned();
    Ok(ValidatedEndpoint {
        endpoint,
        origin,
        kind,
        warning,
    })
}

fn classify_host(host: &Host<&str>) -> PairingEndpointKind {
    match host {
        Host::Ipv4(ip) if ip.is_loopback() => PairingEndpointKind::Loopback,
        Host::Ipv4(ip) if ip.octets()[0] == 100 && (64..=127).contains(&ip.octets()[1]) => {
            PairingEndpointKind::Tailscale
        }
        Host::Ipv4(ip) if ip.is_private() || ip.is_link_local() => PairingEndpointKind::Lan,
        Host::Ipv6(ip) if ip.is_loopback() => PairingEndpointKind::Loopback,
        Host::Ipv6(ip) if ip.segments()[0..3] == [0xfd7a, 0x115c, 0xa1e0] => {
            PairingEndpointKind::Tailscale
        }
        Host::Ipv6(ip) if ip.segments()[0] & 0xfe00 == 0xfc00 => PairingEndpointKind::Lan,
        Host::Domain(domain) if domain.eq_ignore_ascii_case("localhost") => {
            PairingEndpointKind::Loopback
        }
        Host::Domain(domain) if domain.to_ascii_lowercase().ends_with(".ts.net") => {
            PairingEndpointKind::Tailscale
        }
        Host::Domain(domain) if domain.to_ascii_lowercase().ends_with(".local") => {
            PairingEndpointKind::Lan
        }
        _ => PairingEndpointKind::CustomHttps,
    }
}

fn normalized_origin(headers: &HeaderMap) -> Result<Option<String>, ApiError> {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return Ok(None);
    };
    let origin = origin
        .to_str()
        .map_err(|_| ApiError::forbidden("Pairing request origin is invalid"))?;
    let url =
        Url::parse(origin).map_err(|_| ApiError::forbidden("Pairing request origin is invalid"))?;
    if !matches!(url.scheme(), "http" | "https") || !url.origin().is_tuple() {
        return Err(ApiError::forbidden("Pairing request origin is invalid"));
    }
    Ok(Some(url.origin().ascii_serialization()))
}

fn device_summary(record: DeviceSessionRecord) -> Result<DeviceSessionSummary, ApiError> {
    Ok(DeviceSessionSummary {
        id: record.id,
        name: record.name,
        endpoint_kind: endpoint_kind(&record.endpoint_kind)?,
        created_at_unix_ms: timestamp(record.created_at_ms)?,
        last_seen_at_unix_ms: record.last_seen_at_ms.map(timestamp).transpose()?,
        revoked_at_unix_ms: record.revoked_at_ms.map(timestamp).transpose()?,
    })
}

fn endpoint_kind(value: &str) -> Result<PairingEndpointKind, ApiError> {
    match value {
        "loopback" => Ok(PairingEndpointKind::Loopback),
        "lan" => Ok(PairingEndpointKind::Lan),
        "tailscale" => Ok(PairingEndpointKind::Tailscale),
        "custom_https" => Ok(PairingEndpointKind::CustomHttps),
        _ => Err(ApiError::internal()),
    }
}

const fn endpoint_kind_name(kind: PairingEndpointKind) -> &'static str {
    match kind {
        PairingEndpointKind::Loopback => "loopback",
        PairingEndpointKind::Lan => "lan",
        PairingEndpointKind::Tailscale => "tailscale",
        PairingEndpointKind::CustomHttps => "custom_https",
    }
}

fn timestamp(value: i64) -> Result<u64, ApiError> {
    u64::try_from(value).map_err(|_| ApiError::internal())
}

fn no_store_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    headers
}
