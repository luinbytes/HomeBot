//! Provider adapter boundary. Provider payloads terminate in this crate.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderHealth {
    Available,
    AuthenticationRequired,
    Unavailable,
}

#[async_trait::async_trait]
pub trait ProviderAdapter: Send + Sync {
    async fn health(&self) -> ProviderHealth;
}
