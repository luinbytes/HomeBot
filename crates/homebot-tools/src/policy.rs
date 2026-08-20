use crate::{
    ActivityKind, ActivitySink, ActivityStatus, CapabilityClass, CapabilityRequest, ToolActivity,
    ToolError, contracts::unix_ms,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;

const MAX_PENDING_APPROVALS: usize = 4_096;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyEffect {
    Deny,
    RequireApproval,
    Allow,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PolicyRule {
    pub id: Uuid,
    pub capability: CapabilityClass,
    pub owner_id: Option<Uuid>,
    pub device_id: Option<Uuid>,
    pub bot_id: Option<Uuid>,
    pub chat_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub action_prefix: Option<String>,
    pub effect: PolicyEffect,
}

impl PolicyRule {
    #[must_use]
    pub fn new(capability: CapabilityClass, effect: PolicyEffect) -> Self {
        Self {
            id: Uuid::now_v7(),
            capability,
            owner_id: None,
            device_id: None,
            bot_id: None,
            chat_id: None,
            workspace_id: None,
            action_prefix: None,
            effect,
        }
    }

    #[must_use]
    pub fn workspace(mut self, workspace_id: Uuid) -> Self {
        self.workspace_id = Some(workspace_id);
        self
    }

    #[must_use]
    pub fn owner(mut self, owner_id: Uuid) -> Self {
        self.owner_id = Some(owner_id);
        self
    }

    #[must_use]
    pub fn device(mut self, device_id: Uuid) -> Self {
        self.device_id = Some(device_id);
        self
    }

    #[must_use]
    pub fn bot(mut self, bot_id: Uuid) -> Self {
        self.bot_id = Some(bot_id);
        self
    }

    #[must_use]
    pub fn chat(mut self, chat_id: Uuid) -> Self {
        self.chat_id = Some(chat_id);
        self
    }

    #[must_use]
    pub fn action_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.action_prefix = Some(prefix.into());
        self
    }

    fn matches(&self, request: &CapabilityRequest) -> bool {
        self.capability == request.capability
            && self
                .owner_id
                .is_none_or(|id| id == request.context.owner_id)
            && self
                .device_id
                .is_none_or(|id| id == request.context.device_id)
            && self.bot_id.is_none_or(|id| id == request.context.bot_id)
            && self.chat_id.is_none_or(|id| id == request.context.chat_id)
            && self
                .workspace_id
                .is_none_or(|id| id == request.context.workspace_id)
            && self
                .action_prefix
                .as_ref()
                .is_none_or(|prefix| request.action.starts_with(prefix))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ApprovalTicket {
    pub approval_id: Uuid,
    pub operation_id: Uuid,
    pub capability: CapabilityClass,
    pub action: String,
    pub summary: String,
    pub destructive: bool,
    pub expires_at_unix_ms: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApprovalDecision {
    AllowOnce,
    Deny,
}

#[derive(Clone, Debug)]
struct ApprovalRecord {
    ticket: ApprovalTicket,
    request_digest: [u8; 32],
    decision: Option<ApprovalDecision>,
    policy_revision: u64,
}

/// Proof that the server policy engine authorized exactly one request.
pub struct AuthorizedOperation {
    request_digest: [u8; 32],
}

impl AuthorizedOperation {
    fn new(request_digest: [u8; 32]) -> Self {
        Self { request_digest }
    }
}

impl Drop for AuthorizedOperation {
    fn drop(&mut self) {
        self.request_digest.fill(0);
    }
}

pub struct PolicyEngine {
    rules: RwLock<Vec<PolicyRule>>,
    approvals: Mutex<HashMap<Uuid, ApprovalRecord>>,
    policy_revision: AtomicU64,
    approval_ttl: Duration,
    activities: Arc<dyn ActivitySink>,
}

impl std::fmt::Debug for PolicyEngine {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PolicyEngine")
            .field("approval_ttl", &self.approval_ttl)
            .finish_non_exhaustive()
    }
}

impl PolicyEngine {
    #[must_use]
    pub fn new(approval_ttl: Duration, activities: Arc<dyn ActivitySink>) -> Self {
        Self {
            rules: RwLock::new(Vec::new()),
            approvals: Mutex::new(HashMap::new()),
            policy_revision: AtomicU64::new(0),
            approval_ttl,
            activities,
        }
    }

    pub async fn replace_rules(&self, rules: Vec<PolicyRule>) {
        let mut current = self.rules.write().await;
        *current = rules;
        self.policy_revision.fetch_add(1, Ordering::SeqCst);
    }

    /// Records a server-authenticated decision for a pending approval.
    ///
    /// # Errors
    ///
    /// Rejects missing, expired and already-decided approvals.
    pub async fn decide(
        &self,
        approval_id: Uuid,
        decision: ApprovalDecision,
    ) -> Result<(), ToolError> {
        let mut approvals = self.approvals.lock().await;
        let record = approvals
            .get_mut(&approval_id)
            .ok_or(ToolError::InvalidApproval)?;
        if record.ticket.expires_at_unix_ms <= unix_ms() || record.decision.is_some() {
            approvals.remove(&approval_id);
            return Err(ToolError::InvalidApproval);
        }
        record.decision = Some(decision);
        Ok(())
    }

    /// Authorizes one exact capability request or creates/consumes its structured approval.
    ///
    /// # Errors
    /// Returns denial, an approval ticket, invalid approval, or a bounded policy failure.
    pub async fn authorize(
        &self,
        request: &CapabilityRequest,
        approval_id: Option<Uuid>,
    ) -> Result<AuthorizedOperation, ToolError> {
        let digest = request_digest(request)?;
        if let Some(approval_id) = approval_id {
            return self.consume_approval(request, approval_id, digest).await;
        }
        match self.effect(request).await {
            PolicyEffect::Deny => {
                self.emit_policy_activity(request, ActivityStatus::Denied, "Capability denied")
                    .await;
                Err(ToolError::Denied)
            }
            PolicyEffect::Allow => Ok(AuthorizedOperation::new(digest)),
            PolicyEffect::RequireApproval => {
                let mut approvals = self.approvals.lock().await;
                approvals.retain(|_, record| record.ticket.expires_at_unix_ms > unix_ms());
                if let Some(existing) = approvals.values().find(|record| {
                    record.request_digest == digest
                        && record.ticket.operation_id == request.context.operation_id
                        && record.decision.is_none()
                        && record.policy_revision == self.policy_revision.load(Ordering::SeqCst)
                }) {
                    return Err(ToolError::ApprovalRequired(existing.ticket.clone()));
                }
                let ticket = ApprovalTicket {
                    approval_id: Uuid::now_v7(),
                    operation_id: request.context.operation_id,
                    capability: request.capability,
                    action: request.action.clone(),
                    summary: request.summary.clone(),
                    destructive: request.destructive,
                    expires_at_unix_ms: unix_ms().saturating_add(
                        self.approval_ttl.as_millis().try_into().unwrap_or(u64::MAX),
                    ),
                };
                if approvals.len() >= MAX_PENDING_APPROVALS {
                    return Err(ToolError::LimitExceeded);
                }
                approvals.insert(
                    ticket.approval_id,
                    ApprovalRecord {
                        ticket: ticket.clone(),
                        request_digest: digest,
                        decision: None,
                        policy_revision: self.policy_revision.load(Ordering::SeqCst),
                    },
                );
                self.emit_policy_activity(request, ActivityStatus::Updated, "Approval required")
                    .await;
                Err(ToolError::ApprovalRequired(ticket))
            }
        }
    }

    async fn consume_approval(
        &self,
        request: &CapabilityRequest,
        approval_id: Uuid,
        digest: [u8; 32],
    ) -> Result<AuthorizedOperation, ToolError> {
        let Some(record) = self.approvals.lock().await.remove(&approval_id) else {
            return Err(ToolError::InvalidApproval);
        };
        if record.ticket.expires_at_unix_ms <= unix_ms()
            || record.request_digest != digest
            || record.ticket.operation_id != request.context.operation_id
            || record.policy_revision != self.policy_revision.load(Ordering::SeqCst)
        {
            return Err(ToolError::InvalidApproval);
        }
        match record.decision {
            Some(ApprovalDecision::AllowOnce) => Ok(AuthorizedOperation::new(digest)),
            Some(ApprovalDecision::Deny) => {
                self.emit_policy_activity(request, ActivityStatus::Denied, "Approval denied")
                    .await;
                Err(ToolError::Denied)
            }
            None => Err(ToolError::InvalidApproval),
        }
    }

    async fn effect(&self, request: &CapabilityRequest) -> PolicyEffect {
        let rules = self.rules.read().await;
        let mut matched_allow = false;
        let mut matched_approval = false;
        for rule in rules.iter().filter(|rule| rule.matches(request)) {
            match rule.effect {
                PolicyEffect::Deny => return PolicyEffect::Deny,
                PolicyEffect::RequireApproval => matched_approval = true,
                PolicyEffect::Allow => matched_allow = true,
            }
        }
        if matched_approval {
            PolicyEffect::RequireApproval
        } else if matched_allow {
            PolicyEffect::Allow
        } else {
            PolicyEffect::RequireApproval
        }
    }

    async fn emit_policy_activity(
        &self,
        request: &CapabilityRequest,
        status: ActivityStatus,
        title: &str,
    ) {
        self.activities
            .emit(ToolActivity::new(
                request.context.operation_id,
                ActivityKind::Approval,
                status,
                title,
                Some(request.summary.clone()),
            ))
            .await;
    }
}

fn request_digest(request: &CapabilityRequest) -> Result<[u8; 32], ToolError> {
    let encoded = serde_json::to_vec(request).map_err(|_| ToolError::OperationFailed)?;
    Ok(Sha256::digest(encoded).into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RecordingActivitySink;

    fn request() -> CapabilityRequest {
        CapabilityRequest {
            context: crate::OperationContext {
                operation_id: Uuid::now_v7(),
                owner_id: Uuid::now_v7(),
                device_id: Uuid::now_v7(),
                bot_id: Uuid::now_v7(),
                chat_id: Uuid::now_v7(),
                workspace_id: Uuid::now_v7(),
            },
            capability: CapabilityClass::FilesystemWrite,
            action: "filesystem.write".to_owned(),
            canonical_resource: "/workspace/file".to_owned(),
            summary: "Write file".to_owned(),
            destructive: false,
        }
    }

    #[tokio::test]
    async fn approval_is_digest_bound_and_single_use() {
        let engine = PolicyEngine::new(
            Duration::from_secs(60),
            Arc::new(RecordingActivitySink::default()),
        );
        let request = request();
        let ToolError::ApprovalRequired(ticket) = engine
            .authorize(&request, None)
            .await
            .err()
            .unwrap_or(ToolError::OperationFailed)
        else {
            panic!("expected approval");
        };
        let ToolError::ApprovalRequired(duplicate) = engine
            .authorize(&request, None)
            .await
            .err()
            .unwrap_or(ToolError::OperationFailed)
        else {
            panic!("expected duplicate approval");
        };
        assert_eq!(duplicate.approval_id, ticket.approval_id);
        engine
            .decide(ticket.approval_id, ApprovalDecision::AllowOnce)
            .await
            .unwrap_or_else(|error| panic!("approval failed: {error}"));
        let mut changed = request.clone();
        changed.canonical_resource = "/workspace/other".to_owned();
        assert!(matches!(
            engine.authorize(&changed, Some(ticket.approval_id)).await,
            Err(ToolError::InvalidApproval)
        ));
        assert!(matches!(
            engine.authorize(&request, Some(ticket.approval_id)).await,
            Err(ToolError::InvalidApproval)
        ));
    }

    #[tokio::test]
    async fn approved_request_succeeds_once_and_expired_ticket_fails() {
        let engine = PolicyEngine::new(
            Duration::from_secs(60),
            Arc::new(RecordingActivitySink::default()),
        );
        let request = request();
        let ToolError::ApprovalRequired(ticket) = engine
            .authorize(&request, None)
            .await
            .err()
            .unwrap_or(ToolError::OperationFailed)
        else {
            panic!("expected approval");
        };
        engine
            .decide(ticket.approval_id, ApprovalDecision::AllowOnce)
            .await
            .unwrap_or_else(|error| panic!("approval failed: {error}"));
        assert!(
            engine
                .authorize(&request, Some(ticket.approval_id))
                .await
                .is_ok()
        );
        assert!(matches!(
            engine.authorize(&request, Some(ticket.approval_id)).await,
            Err(ToolError::InvalidApproval)
        ));

        let expiring =
            PolicyEngine::new(Duration::ZERO, Arc::new(RecordingActivitySink::default()));
        let ToolError::ApprovalRequired(expired) = expiring
            .authorize(&request, None)
            .await
            .err()
            .unwrap_or(ToolError::OperationFailed)
        else {
            panic!("expected approval");
        };
        assert!(matches!(
            expiring
                .decide(expired.approval_id, ApprovalDecision::AllowOnce)
                .await,
            Err(ToolError::InvalidApproval)
        ));
    }

    #[tokio::test]
    async fn deny_wins_and_unmatched_requests_require_approval() {
        let engine = PolicyEngine::new(
            Duration::from_secs(60),
            Arc::new(RecordingActivitySink::default()),
        );
        engine
            .replace_rules(vec![
                PolicyRule::new(CapabilityClass::FilesystemWrite, PolicyEffect::Allow),
                PolicyRule::new(CapabilityClass::FilesystemWrite, PolicyEffect::Deny)
                    .action_prefix("filesystem.write"),
            ])
            .await;
        assert!(matches!(
            engine.authorize(&request(), None).await,
            Err(ToolError::Denied)
        ));
        let mut unmatched = request();
        unmatched.capability = CapabilityClass::BrowserObserve;
        assert!(matches!(
            engine.authorize(&unmatched, None).await,
            Err(ToolError::ApprovalRequired(_))
        ));
    }

    #[tokio::test]
    async fn policy_change_invalidates_previously_approved_ticket() {
        let engine = PolicyEngine::new(
            Duration::from_secs(60),
            Arc::new(RecordingActivitySink::default()),
        );
        let request = request();
        let ToolError::ApprovalRequired(ticket) = engine
            .authorize(&request, None)
            .await
            .err()
            .unwrap_or(ToolError::OperationFailed)
        else {
            panic!("expected approval");
        };
        engine
            .decide(ticket.approval_id, ApprovalDecision::AllowOnce)
            .await
            .unwrap_or_else(|error| panic!("approval failed: {error}"));
        engine
            .replace_rules(vec![PolicyRule::new(
                CapabilityClass::FilesystemWrite,
                PolicyEffect::Deny,
            )])
            .await;
        assert!(matches!(
            engine.authorize(&request, Some(ticket.approval_id)).await,
            Err(ToolError::InvalidApproval)
        ));
    }
}
