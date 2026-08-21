use homebot_server::{AppState, serve};
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
    let token = std::env::var("HOMEBOT_DEVICE_TOKEN")
        .map_err(|_| anyhow::anyhow!("HOMEBOT_DEVICE_TOKEN must be set"))?;
    let storage = Storage::open(&data_path).await?;
    let artifact_root = data_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("artifacts");
    let state = AppState::new(storage, &token).with_artifact_root(artifact_root);
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
    use super::validated_bind;

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
}
