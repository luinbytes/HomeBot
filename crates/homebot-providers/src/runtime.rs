//! Adapter registry and operation routing owned by the `HomeBot` server.

use crate::{
    ApprovalDecision, CompactRequest, ProviderAdapter, ProviderAdapterId, ProviderDescriptor,
    ProviderError, ProviderHealth, ProviderModel, ProviderRun, ProviderToolResult, ResumeRequest,
    StartRequest,
};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::RwLock;
use uuid::Uuid;

#[derive(Default)]
pub struct ProviderRuntime {
    adapters: RwLock<HashMap<ProviderAdapterId, Arc<dyn ProviderAdapter>>>,
    operations: RwLock<HashMap<Uuid, ProviderAdapterId>>,
}

impl std::fmt::Debug for ProviderRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProviderRuntime")
            .field("adapters", &"provider-neutral registry")
            .field("operations", &"operation routing table")
            .finish()
    }
}

impl ProviderRuntime {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers one adapter under its stable ID.
    ///
    /// # Errors
    ///
    /// Rejects duplicate IDs so routing can never depend on registration order.
    pub async fn register(
        &self,
        adapter: Arc<dyn ProviderAdapter>,
    ) -> Result<(), ProviderRuntimeError> {
        let id = adapter.id().clone();
        let mut adapters = self.adapters.write().await;
        if adapters.contains_key(&id) {
            return Err(ProviderRuntimeError::DuplicateAdapter(id));
        }
        adapters.insert(id, adapter);
        Ok(())
    }

    /// Discovers every registered adapter without exposing native payloads.
    ///
    /// # Errors
    ///
    /// Returns the first normalized adapter error.
    pub async fn discover(&self) -> Result<Vec<ProviderDescriptor>, ProviderRuntimeError> {
        let adapters = self
            .adapters
            .read()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let mut descriptors = Vec::with_capacity(adapters.len());
        for adapter in adapters {
            descriptors.push(
                adapter
                    .discover()
                    .await
                    .map_err(ProviderRuntimeError::Provider)?,
            );
        }
        descriptors.sort_by(|left, right| left.adapter_id.as_str().cmp(right.adapter_id.as_str()));
        Ok(descriptors)
    }

    /// Discovers one configured adapter without allowing unrelated providers to block it.
    ///
    /// # Errors
    /// Fails when the adapter is unknown or its normalized discovery fails.
    pub async fn descriptor(
        &self,
        adapter_id: &ProviderAdapterId,
    ) -> Result<ProviderDescriptor, ProviderRuntimeError> {
        self.adapter(adapter_id)
            .await?
            .discover()
            .await
            .map_err(ProviderRuntimeError::Provider)
    }

    /// Checks all adapters independently so one unavailable provider does not hide others.
    pub async fn health(&self) -> Vec<(ProviderAdapterId, ProviderHealth)> {
        let adapters = self
            .adapters
            .read()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let mut health = Vec::with_capacity(adapters.len());
        for adapter in adapters {
            health.push((adapter.id().clone(), adapter.health().await));
        }
        health.sort_by(|left, right| left.0.as_str().cmp(right.0.as_str()));
        health
    }

    /// Lists normalized models for an adapter.
    ///
    /// # Errors
    ///
    /// Fails when the adapter is unknown or reports a normalized failure.
    pub async fn models(
        &self,
        adapter_id: &ProviderAdapterId,
    ) -> Result<Vec<ProviderModel>, ProviderRuntimeError> {
        self.adapter(adapter_id)
            .await?
            .models()
            .await
            .map_err(ProviderRuntimeError::Provider)
    }

    /// Starts an operation and records which adapter owns cancellation.
    ///
    /// # Errors
    ///
    /// Rejects duplicate operation IDs and adapter protocol mismatches.
    pub async fn start(
        &self,
        adapter_id: &ProviderAdapterId,
        request: StartRequest,
    ) -> Result<ProviderRun, ProviderRuntimeError> {
        let adapter = self.adapter(adapter_id).await?;
        self.reserve_operation(request.operation_id, adapter_id)
            .await?;
        match adapter.start(request.clone()).await {
            Ok(run) if run.operation_id == request.operation_id => Ok(run),
            Ok(_) => {
                self.finish(request.operation_id).await;
                Err(ProviderRuntimeError::OperationMismatch)
            }
            Err(error) => {
                self.finish(request.operation_id).await;
                Err(ProviderRuntimeError::Provider(error))
            }
        }
    }

    /// Resumes a provider conversation while preserving the `HomeBot` operation ID.
    ///
    /// # Errors
    ///
    /// Rejects duplicate operation IDs and adapter protocol mismatches.
    pub async fn resume(
        &self,
        adapter_id: &ProviderAdapterId,
        request: ResumeRequest,
    ) -> Result<ProviderRun, ProviderRuntimeError> {
        let adapter = self.adapter(adapter_id).await?;
        self.reserve_operation(request.operation_id, adapter_id)
            .await?;
        match adapter.resume(request.clone()).await {
            Ok(run) if run.operation_id == request.operation_id => Ok(run),
            Ok(_) => {
                self.finish(request.operation_id).await;
                Err(ProviderRuntimeError::OperationMismatch)
            }
            Err(error) => {
                self.finish(request.operation_id).await;
                Err(ProviderRuntimeError::Provider(error))
            }
        }
    }

    /// Routes cancellation to the adapter that owns an operation.
    ///
    /// # Errors
    ///
    /// Fails safely for unknown operations or normalized adapter failures.
    pub async fn cancel(&self, operation_id: Uuid) -> Result<(), ProviderRuntimeError> {
        let adapter_id = self
            .operations
            .read()
            .await
            .get(&operation_id)
            .cloned()
            .ok_or(ProviderRuntimeError::OperationNotFound(operation_id))?;
        self.adapter(&adapter_id)
            .await?
            .cancel(operation_id)
            .await
            .map_err(ProviderRuntimeError::Provider)
    }

    /// Resolves a provider approval without exposing provider-native decision payloads.
    ///
    /// # Errors
    ///
    /// Fails when the adapter is unknown or the approval is no longer pending.
    pub async fn resolve_approval(
        &self,
        adapter_id: &ProviderAdapterId,
        approval_id: Uuid,
        decision: ApprovalDecision,
    ) -> Result<(), ProviderRuntimeError> {
        self.adapter(adapter_id)
            .await?
            .resolve_approval(approval_id, decision)
            .await
            .map_err(ProviderRuntimeError::Provider)
    }

    /// Returns a client-executed tool result to the provider turn that requested it.
    ///
    /// # Errors
    ///
    /// Fails when the adapter is unknown or the tool call is no longer pending.
    pub async fn resolve_tool_call(
        &self,
        adapter_id: &ProviderAdapterId,
        call_id: String,
        result: ProviderToolResult,
    ) -> Result<(), ProviderRuntimeError> {
        self.adapter(adapter_id)
            .await?
            .resolve_tool_call(call_id, result)
            .await
            .map_err(ProviderRuntimeError::Provider)
    }

    /// Marks routing state complete after the terminal provider event is durable.
    pub async fn finish(&self, operation_id: Uuid) {
        self.operations.write().await.remove(&operation_id);
    }

    /// Requests provider-native compaction behind the normalized contract.
    ///
    /// # Errors
    ///
    /// Fails for unknown adapters or normalized provider errors.
    pub async fn compact(
        &self,
        adapter_id: &ProviderAdapterId,
        request: CompactRequest,
    ) -> Result<(), ProviderRuntimeError> {
        self.adapter(adapter_id)
            .await?
            .compact(request)
            .await
            .map_err(ProviderRuntimeError::Provider)
    }

    /// Queries all adapters for interrupted operation IDs they can recover.
    ///
    /// # Errors
    ///
    /// Returns a normalized failure rather than provider-native diagnostics.
    pub async fn recover(&self) -> Result<Vec<ProviderRun>, ProviderRuntimeError> {
        let adapters = self
            .adapters
            .read()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let mut recovered = Vec::new();
        for adapter in adapters {
            let runs = adapter
                .recover()
                .await
                .map_err(ProviderRuntimeError::Provider)?;
            for run in runs {
                self.reserve_operation(run.operation_id, adapter.id())
                    .await?;
                recovered.push(run);
            }
        }
        recovered.sort_by_key(|run| run.operation_id);
        Ok(recovered)
    }

    async fn adapter(
        &self,
        adapter_id: &ProviderAdapterId,
    ) -> Result<Arc<dyn ProviderAdapter>, ProviderRuntimeError> {
        self.adapters
            .read()
            .await
            .get(adapter_id)
            .cloned()
            .ok_or_else(|| ProviderRuntimeError::AdapterNotFound(adapter_id.clone()))
    }

    async fn reserve_operation(
        &self,
        operation_id: Uuid,
        adapter_id: &ProviderAdapterId,
    ) -> Result<(), ProviderRuntimeError> {
        let mut operations = self.operations.write().await;
        if operations.contains_key(&operation_id) {
            return Err(ProviderRuntimeError::DuplicateOperation(operation_id));
        }
        operations.insert(operation_id, adapter_id.clone());
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderRuntimeError {
    #[error("provider adapter {0} is already registered")]
    DuplicateAdapter(ProviderAdapterId),
    #[error("provider adapter {0} is not registered")]
    AdapterNotFound(ProviderAdapterId),
    #[error("provider operation {0} is already active")]
    DuplicateOperation(Uuid),
    #[error("provider operation {0} is not active")]
    OperationNotFound(Uuid),
    #[error("provider adapter returned a mismatched operation ID")]
    OperationMismatch,
    #[error("provider operation failed: {0:?}")]
    Provider(ProviderError),
}
