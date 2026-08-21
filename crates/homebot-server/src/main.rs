use homebot_server::{provider_bootstrap, serve};
use homebot_storage::Storage;
use std::net::SocketAddr;
use tokio::net::TcpListener;

const DEFAULT_BIND: &str = "127.0.0.1:7123";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let data_path = std::env::var_os("HOMEBOT_DATABASE").map_or_else(
        || std::path::PathBuf::from("homebot.db"),
        std::path::PathBuf::from,
    );
    let token = load_device_token(
        std::env::var_os("HOMEBOT_DEVICE_TOKEN"),
        std::env::var_os("HOMEBOT_DEVICE_TOKEN_FILE").map(std::path::PathBuf::from),
    )?;
    let storage = Storage::open(&data_path).await?;
    let provider_config = std::env::var_os("HOMEBOT_PROVIDER_CONFIG").map(std::path::PathBuf::from);
    let artifact_root = data_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("artifacts");
    let state = provider_bootstrap::compose_app_state(
        storage,
        &token,
        artifact_root,
        provider_bootstrap::load_config(provider_config.as_deref())?,
    )
    .await?;
    let bind = std::env::var("HOMEBOT_BIND").unwrap_or_else(|_| DEFAULT_BIND.to_owned());
    let allow_remote = std::env::var("HOMEBOT_ALLOW_REMOTE")
        .is_ok_and(|value| matches!(value.as_str(), "1" | "true" | "yes"));
    let address = validated_bind(&bind, allow_remote)?;
    let listener = TcpListener::bind(address).await?;
    if address.ip().is_loopback() {
        tracing::info!(%address, "HomeBot server listening on loopback");
    } else {
        tracing::warn!(
            %address,
            "HomeBot remote listener enabled; use a private network or an HTTPS reverse proxy"
        );
    }
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = shutdown_tx.send(());
    });
    serve(listener, state, shutdown_rx).await?;
    Ok(())
}

fn load_device_token(
    direct: Option<std::ffi::OsString>,
    credential_file: Option<std::path::PathBuf>,
) -> anyhow::Result<String> {
    let raw = match (direct, credential_file) {
        (Some(_), Some(_)) => {
            anyhow::bail!("set only one of HOMEBOT_DEVICE_TOKEN or HOMEBOT_DEVICE_TOKEN_FILE")
        }
        (Some(value), None) => value
            .into_string()
            .map_err(|_| anyhow::anyhow!("HOMEBOT_DEVICE_TOKEN is not valid UTF-8"))?,
        (None, Some(path)) => {
            let metadata = std::fs::metadata(&path)
                .map_err(|_| anyhow::anyhow!("HomeBot owner credential file is unavailable"))?;
            if !metadata.is_file() || metadata.len() > 4_096 {
                anyhow::bail!("HomeBot owner credential file is invalid");
            }
            std::fs::read_to_string(path)
                .map_err(|_| anyhow::anyhow!("HomeBot owner credential file is unreadable"))?
        }
        (None, None) => {
            anyhow::bail!("HOMEBOT_DEVICE_TOKEN or HOMEBOT_DEVICE_TOKEN_FILE must be set")
        }
    };
    let token = raw.trim().to_owned();
    if token.is_empty() {
        anyhow::bail!("HomeBot owner credential must not be empty");
    }
    Ok(token)
}

fn validated_bind(raw: &str, allow_remote: bool) -> anyhow::Result<SocketAddr> {
    let address: SocketAddr = raw
        .parse()
        .map_err(|_| anyhow::anyhow!("HOMEBOT_BIND must be an IP socket address"))?;
    if !address.ip().is_loopback() && !allow_remote {
        anyhow::bail!("refusing non-loopback HOMEBOT_BIND without HOMEBOT_ALLOW_REMOTE=1");
    }
    Ok(address)
}

#[cfg(test)]
mod tests {
    use super::{load_device_token, validated_bind};
    use std::ffi::OsString;

    #[test]
    fn remote_bind_requires_explicit_acknowledgement() {
        assert!(validated_bind("127.0.0.1:7123", false).is_ok());
        assert!(validated_bind("[::1]:7123", false).is_ok());
        assert!(validated_bind("192.168.1.20:7123", false).is_err());
        assert!(validated_bind("100.64.1.2:7123", false).is_err());
        assert!(validated_bind("0.0.0.0:7123", false).is_err());
        assert!(validated_bind("192.168.1.20:7123", true).is_ok());
        assert!(validated_bind("hostname:7123", true).is_err());
    }

    #[test]
    fn credential_file_is_bounded_trimmed_and_mutually_exclusive() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let credential = directory.path().join("owner-token");
        std::fs::write(&credential, "owner-secret\n")?;
        assert_eq!(
            load_device_token(None, Some(credential.clone()))?,
            "owner-secret"
        );
        assert!(
            load_device_token(Some(OsString::from("direct")), Some(credential.clone())).is_err()
        );
        std::fs::write(&credential, vec![b'x'; 4_097])?;
        assert!(load_device_token(None, Some(credential)).is_err());
        assert!(load_device_token(None, None).is_err());
        Ok(())
    }
}
