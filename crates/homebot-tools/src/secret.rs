//! Explicit, policy-gated secret resolution for secret-aware tools.

use crate::{CapabilityClass, CapabilityRequest, OperationContext, PolicyEngine, ToolError};
use homebot_secrets::{ResolvedSecret, SecretStoreError, SecretVault, locator_for};
use std::sync::Arc;
use uuid::Uuid;

#[derive(Clone)]
pub struct SecretToolService {
    policy: Arc<PolicyEngine>,
    vault: Arc<dyn SecretVault>,
}

impl std::fmt::Debug for SecretToolService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SecretToolService([REDACTED])")
    }
}

impl SecretToolService {
    #[must_use]
    pub fn new(policy: Arc<PolicyEngine>, vault: Arc<dyn SecretVault>) -> Self {
        Self { policy, vault }
    }

    /// Resolves one secret only after the server policy authorizes the exact tool purpose.
    ///
    /// # Errors
    ///
    /// Returns an approval/denial error, rejects an invalid purpose, or fails closed when the
    /// operating-system credential store cannot resolve the reference.
    pub async fn resolve_for_tool(
        &self,
        context: OperationContext,
        reference_id: Uuid,
        purpose: &str,
        approval_id: Option<Uuid>,
    ) -> Result<ResolvedSecret, ToolError> {
        let purpose = purpose.trim();
        if purpose.is_empty()
            || purpose.len() > 64
            || !purpose
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        {
            return Err(ToolError::InvalidRequest(
                "secret purpose must be a bounded identifier".to_owned(),
            ));
        }
        let request = CapabilityRequest {
            context,
            capability: CapabilityClass::SecretUse,
            action: format!("secret.use.{purpose}"),
            canonical_resource: format!("secret:{reference_id}"),
            summary: format!("Use a stored secret for {purpose}"),
            destructive: false,
        };
        let _authorization = self.policy.authorize(&request, approval_id).await?;
        self.vault
            .resolve(&locator_for(reference_id))
            .await
            .map_err(|error| map_store_error(&error))
    }
}

fn map_store_error(error: &SecretStoreError) -> ToolError {
    match error {
        SecretStoreError::InvalidReference => {
            ToolError::InvalidRequest("secret reference is invalid".to_owned())
        }
        SecretStoreError::Locked
        | SecretStoreError::Unavailable
        | SecretStoreError::NotFound
        | SecretStoreError::OperationFailed => ToolError::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ApprovalDecision, NoopActivitySink, PolicyEffect, PolicyRule};
    use homebot_secrets::{MemorySecretVault, SecretInput};
    use std::time::Duration;

    fn context() -> OperationContext {
        OperationContext {
            operation_id: Uuid::now_v7(),
            owner_id: Uuid::now_v7(),
            device_id: Uuid::now_v7(),
            bot_id: Uuid::now_v7(),
            chat_id: Uuid::now_v7(),
            workspace_id: Uuid::now_v7(),
        }
    }

    #[tokio::test]
    async fn ordinary_callers_need_exact_secret_use_approval()
    -> Result<(), Box<dyn std::error::Error>> {
        let policy = Arc::new(PolicyEngine::new(
            Duration::from_secs(30),
            Arc::new(NoopActivitySink),
        ));
        let vault = Arc::new(MemorySecretVault::default());
        let reference_id = Uuid::now_v7();
        vault
            .put(
                &locator_for(reference_id),
                SecretInput::new("tool-canary-secret"),
            )
            .await?;
        let service = SecretToolService::new(policy.clone(), vault);
        let operation = context();
        let Err(ToolError::ApprovalRequired(ticket)) = service
            .resolve_for_tool(operation.clone(), reference_id, "github_api", None)
            .await
        else {
            return Err("secret use did not require approval".into());
        };
        assert_eq!(ticket.capability, CapabilityClass::SecretUse);
        assert!(!format!("{ticket:?}").contains("tool-canary-secret"));
        policy
            .decide(ticket.approval_id, ApprovalDecision::AllowOnce)
            .await?;
        let resolved = service
            .resolve_for_tool(
                operation,
                reference_id,
                "github_api",
                Some(ticket.approval_id),
            )
            .await?;
        assert_eq!(format!("{resolved:?}"), "ResolvedSecret([REDACTED])");
        assert!(matches!(
            service
                .resolve_for_tool(
                    context(),
                    reference_id,
                    "github_api",
                    Some(ticket.approval_id)
                )
                .await,
            Err(ToolError::InvalidApproval)
        ));
        Ok(())
    }

    #[tokio::test]
    async fn only_matching_secret_use_rules_can_bypass_approval()
    -> Result<(), Box<dyn std::error::Error>> {
        let policy = Arc::new(PolicyEngine::new(
            Duration::from_secs(30),
            Arc::new(NoopActivitySink),
        ));
        policy
            .replace_rules(vec![
                PolicyRule::new(CapabilityClass::SecretUse, PolicyEffect::Allow)
                    .action_prefix("secret.use.github_api"),
            ])
            .await;
        let vault = Arc::new(MemorySecretVault::default());
        let reference_id = Uuid::now_v7();
        vault
            .put(&locator_for(reference_id), SecretInput::new("hidden"))
            .await?;
        let service = SecretToolService::new(policy, vault);
        assert!(
            service
                .resolve_for_tool(context(), reference_id, "github_api", None)
                .await
                .is_ok()
        );
        assert!(matches!(
            service
                .resolve_for_tool(context(), reference_id, "other", None)
                .await,
            Err(ToolError::ApprovalRequired(_))
        ));
        Ok(())
    }
}
