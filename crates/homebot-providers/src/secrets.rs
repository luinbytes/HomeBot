//! Opaque secret references at the provider boundary.

use crate::ProviderError;
use uuid::Uuid;
use zeroize::Zeroizing;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SecretReference(Uuid);

impl SecretReference {
    #[must_use]
    pub const fn new(id: Uuid) -> Self {
        Self(id)
    }

    #[must_use]
    pub const fn id(self) -> Uuid {
        self.0
    }
}

/// A short-lived resolved value that redacts debug output and zeroes memory on drop.
pub struct ResolvedSecret(Zeroizing<String>);

impl ResolvedSecret {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(Zeroizing::new(value.into()))
    }

    pub(crate) fn expose(&self) -> &str {
        self.0.as_str()
    }
}

impl std::fmt::Debug for ResolvedSecret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ResolvedSecret([REDACTED])")
    }
}

#[async_trait::async_trait]
pub trait ProviderSecretResolver: Send + Sync {
    async fn resolve(&self, reference: SecretReference) -> Result<ResolvedSecret, ProviderError>;
}
