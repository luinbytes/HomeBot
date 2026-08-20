use homebot_server::{AppState, serve};
use homebot_storage::Storage;
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
    let listener = TcpListener::bind(&bind).await?;
    tracing::info!(address = bind, "HomeBot server listening on loopback");
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = shutdown_tx.send(());
    });
    serve(listener, state, shutdown_rx).await?;
    Ok(())
}
