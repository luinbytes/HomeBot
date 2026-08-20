//! OS-backed secret storage with opaque references at every non-secret boundary.

use async_trait::async_trait;
pub use homebot_providers::ResolvedSecret;
use homebot_providers::{
    ProviderError, ProviderErrorCode, ProviderSecretResolver, SecretReference,
};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::RwLock;
use zeroize::Zeroizing;

const SERVICE_NAME: &str = "dev.homebot.secret";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SecretStatus {
    Ready,
    Locked,
    Unavailable,
    Missing,
}

#[derive(Debug, thiserror::Error)]
pub enum SecretStoreError {
    #[error("the operating-system credential store is locked")]
    Locked,
    #[error("the operating-system credential store is unavailable")]
    Unavailable,
    #[error("the secret does not exist")]
    NotFound,
    #[error("the secret reference is invalid")]
    InvalidReference,
    #[error("the secret operation could not be completed")]
    OperationFailed,
}

/// An inbound value that is redacted from diagnostics and zeroed on drop.
pub struct SecretInput(Zeroizing<String>);

impl SecretInput {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(Zeroizing::new(value.into()))
    }

    fn expose(&self) -> &str {
        self.0.as_str()
    }
}

impl std::fmt::Debug for SecretInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SecretInput([REDACTED])")
    }
}

#[async_trait]
pub trait SecretVault: Send + Sync {
    async fn put(&self, locator: &str, value: SecretInput) -> Result<(), SecretStoreError>;
    async fn resolve(&self, locator: &str) -> Result<ResolvedSecret, SecretStoreError>;
    async fn delete(&self, locator: &str) -> Result<(), SecretStoreError>;

    async fn status(&self, locator: &str) -> SecretStatus {
        match self.resolve(locator).await {
            Ok(_) => SecretStatus::Ready,
            Err(SecretStoreError::Locked) => SecretStatus::Locked,
            Err(SecretStoreError::NotFound) => SecretStatus::Missing,
            Err(_) => SecretStatus::Unavailable,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct OsSecretVault;

impl OsSecretVault {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SecretVault for OsSecretVault {
    async fn put(&self, locator: &str, value: SecretInput) -> Result<(), SecretStoreError> {
        validate_locator(locator)?;
        let locator = locator.to_owned();
        tokio::task::spawn_blocking(move || {
            let entry = keyring::Entry::new(SERVICE_NAME, &locator).map_err(map_keyring_error)?;
            entry
                .set_password(value.expose())
                .map_err(map_keyring_error)
        })
        .await
        .map_err(|_| SecretStoreError::OperationFailed)?
    }

    async fn resolve(&self, locator: &str) -> Result<ResolvedSecret, SecretStoreError> {
        validate_locator(locator)?;
        let locator = locator.to_owned();
        tokio::task::spawn_blocking(move || {
            let entry = keyring::Entry::new(SERVICE_NAME, &locator).map_err(map_keyring_error)?;
            entry
                .get_password()
                .map(ResolvedSecret::new)
                .map_err(map_keyring_error)
        })
        .await
        .map_err(|_| SecretStoreError::OperationFailed)?
    }

    async fn delete(&self, locator: &str) -> Result<(), SecretStoreError> {
        validate_locator(locator)?;
        let locator = locator.to_owned();
        tokio::task::spawn_blocking(move || {
            let entry = keyring::Entry::new(SERVICE_NAME, &locator).map_err(map_keyring_error)?;
            entry.delete_credential().map_err(map_keyring_error)
        })
        .await
        .map_err(|_| SecretStoreError::OperationFailed)?
    }
}

fn validate_locator(locator: &str) -> Result<(), SecretStoreError> {
    if locator.starts_with("homebot:")
        && locator.len() <= 160
        && locator
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'-'))
    {
        Ok(())
    } else {
        Err(SecretStoreError::InvalidReference)
    }
}

fn map_keyring_error(error: keyring::Error) -> SecretStoreError {
    match error {
        keyring::Error::NoStorageAccess(source) => {
            drop(source);
            SecretStoreError::Locked
        }
        keyring::Error::NoEntry => SecretStoreError::NotFound,
        keyring::Error::Invalid(attribute, reason) => {
            drop((attribute, reason));
            SecretStoreError::InvalidReference
        }
        keyring::Error::TooLong(attribute, limit) => {
            drop((attribute, limit));
            SecretStoreError::InvalidReference
        }
        keyring::Error::PlatformFailure(source) => {
            drop(source);
            SecretStoreError::Unavailable
        }
        _ => SecretStoreError::OperationFailed,
    }
}

/// Test and headless-fixture vault. It never writes values to `SQLite` or disk.
#[derive(Clone, Debug, Default)]
pub struct MemorySecretVault {
    values: Arc<RwLock<HashMap<String, Zeroizing<String>>>>,
    forced_status: Arc<RwLock<Option<SecretStatus>>>,
}

impl MemorySecretVault {
    pub async fn force_status(&self, status: Option<SecretStatus>) {
        *self.forced_status.write().await = status;
    }

    async fn ensure_available(&self) -> Result<(), SecretStoreError> {
        match *self.forced_status.read().await {
            Some(SecretStatus::Locked) => Err(SecretStoreError::Locked),
            Some(SecretStatus::Unavailable) => Err(SecretStoreError::Unavailable),
            _ => Ok(()),
        }
    }
}

#[async_trait]
impl SecretVault for MemorySecretVault {
    async fn put(&self, locator: &str, value: SecretInput) -> Result<(), SecretStoreError> {
        validate_locator(locator)?;
        self.ensure_available().await?;
        self.values
            .write()
            .await
            .insert(locator.to_owned(), value.0);
        Ok(())
    }

    async fn resolve(&self, locator: &str) -> Result<ResolvedSecret, SecretStoreError> {
        validate_locator(locator)?;
        self.ensure_available().await?;
        self.values
            .read()
            .await
            .get(locator)
            .map(|value| ResolvedSecret::new(value.as_str()))
            .ok_or(SecretStoreError::NotFound)
    }

    async fn delete(&self, locator: &str) -> Result<(), SecretStoreError> {
        validate_locator(locator)?;
        self.ensure_available().await?;
        self.values
            .write()
            .await
            .remove(locator)
            .map(|_| ())
            .ok_or(SecretStoreError::NotFound)
    }
}

/// Explicit provider-only bridge: normal chat and routine contexts receive references, not values.
#[derive(Clone)]
pub struct VaultProviderSecretResolver {
    vault: Arc<dyn SecretVault>,
}

impl VaultProviderSecretResolver {
    #[must_use]
    pub fn new(vault: Arc<dyn SecretVault>) -> Self {
        Self { vault }
    }
}

#[async_trait]
impl ProviderSecretResolver for VaultProviderSecretResolver {
    async fn resolve(&self, reference: SecretReference) -> Result<ResolvedSecret, ProviderError> {
        self.vault
            .resolve(&locator_for(reference.id()))
            .await
            .map_err(|_| ProviderError {
                code: ProviderErrorCode::AuthenticationRequired,
                message: "provider credential is unavailable".to_owned(),
                retryable: false,
                diagnostic_id: Some(uuid::Uuid::now_v7()),
            })
    }
}

#[must_use]
pub fn locator_for(reference_id: uuid::Uuid) -> String {
    format!("homebot:{reference_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn values_are_redacted_and_crud_is_reference_only()
    -> Result<(), Box<dyn std::error::Error>> {
        let vault = MemorySecretVault::default();
        let locator = locator_for(uuid::Uuid::now_v7());
        let input = SecretInput::new("canary-do-not-leak");
        assert_eq!(format!("{input:?}"), "SecretInput([REDACTED])");
        vault.put(&locator, input).await?;
        let resolved = vault.resolve(&locator).await?;
        assert_eq!(format!("{resolved:?}"), "ResolvedSecret([REDACTED])");
        vault.put(&locator, SecretInput::new("replacement")).await?;
        assert_eq!(vault.status(&locator).await, SecretStatus::Ready);
        vault.delete(&locator).await?;
        assert_eq!(vault.status(&locator).await, SecretStatus::Missing);
        Ok(())
    }

    #[tokio::test]
    async fn locked_and_unavailable_states_fail_closed() {
        let vault = MemorySecretVault::default();
        let locator = locator_for(uuid::Uuid::now_v7());
        vault.force_status(Some(SecretStatus::Locked)).await;
        assert_eq!(vault.status(&locator).await, SecretStatus::Locked);
        assert!(matches!(
            vault.put(&locator, SecretInput::new("hidden")).await,
            Err(SecretStoreError::Locked)
        ));
        vault.force_status(Some(SecretStatus::Unavailable)).await;
        assert_eq!(vault.status(&locator).await, SecretStatus::Unavailable);
    }

    #[test]
    fn locators_are_strict_and_opaque() {
        assert!(validate_locator("homebot:018f-abc").is_ok());
        assert!(validate_locator("other:018f-abc").is_err());
        assert!(validate_locator("homebot:../secret").is_err());
    }
}
