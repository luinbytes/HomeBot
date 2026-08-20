use homebot_server::{AppState, router};
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
    let app = router(AppState::new(storage, &token));
    let listener = TcpListener::bind(DEFAULT_BIND).await?;
    tracing::info!(
        address = DEFAULT_BIND,
        "HomeBot server listening on loopback"
    );
    axum::serve(listener, app).await?;
    Ok(())
}
