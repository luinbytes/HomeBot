use axum::{Json, Router, routing::get};
use serde_json::{Value, json};
use tokio::net::TcpListener;

const DEFAULT_BIND: &str = "127.0.0.1:7123";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let app = Router::new().route("/health", get(health));
    let listener = TcpListener::bind(DEFAULT_BIND).await?;
    tracing::info!(
        address = DEFAULT_BIND,
        "HomeBot server listening on loopback"
    );
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "service": "homebot-server",
        "protocol_version": homebot_protocol::PROTOCOL_VERSION
    }))
}
