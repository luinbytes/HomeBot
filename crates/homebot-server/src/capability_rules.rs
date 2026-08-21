use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
};
use homebot_protocol::{
    BotMutationRequest, CapabilityRuleAuditSummary, CapabilityRuleSummary, ServerEventBody,
    UpsertCapabilityRuleRequest,
};
use homebot_storage::{CapabilityRuleAuditRecord, CapabilityRuleRecord, IdempotencyClaim};
use homebot_tools::{CapabilityClass, PolicyEffect, PolicyRule};
use uuid::Uuid;

use crate::{
    AppState, AuthenticatedIdentity,
    bots::{ApiError, claim},
    chats::publish,
    unix_time_ms,
};

pub(super) async fn list(
    State(state): State<AppState>,
    Extension(identity): Extension<AuthenticatedIdentity>,
) -> Result<Json<Vec<CapabilityRuleSummary>>, ApiError> {
    require_owner(&identity)?;
    Ok(Json(
        state
            .storage
            .capability_rules(state.owner_id)
            .await?
            .into_iter()
            .map(summary)
            .collect::<Result<_, _>>()?,
    ))
}

pub(super) async fn audit(
    State(state): State<AppState>,
    Extension(identity): Extension<AuthenticatedIdentity>,
) -> Result<Json<Vec<CapabilityRuleAuditSummary>>, ApiError> {
    require_owner(&identity)?;
    Ok(Json(
        state
            .storage
            .capability_rule_audit(state.owner_id)
            .await?
            .into_iter()
            .map(audit_summary)
            .collect(),
    ))
}

pub(super) async fn upsert(
    State(state): State<AppState>,
    Extension(identity): Extension<AuthenticatedIdentity>,
    Path(rule_id): Path<Uuid>,
    Json(request): Json<UpsertCapabilityRuleRequest>,
) -> Result<Json<CapabilityRuleSummary>, ApiError> {
    require_owner(&identity)?;
    let replayed = matches!(
        claim(
            &state,
            request.idempotency_key,
            &format!("upsert_capability_rule:{rule_id}"),
            &request,
        )
        .await?,
        IdempotencyClaim::Replayed { .. }
    );
    let rule = if replayed {
        state
            .storage
            .capability_rules(state.owner_id)
            .await?
            .into_iter()
            .find(|rule| rule.id == rule_id)
            .ok_or_else(|| {
                ApiError::conflict("The original capability rule is no longer present")
            })?
    } else {
        let now = unix_time_ms();
        state
            .storage
            .upsert_capability_rule(&CapabilityRuleRecord {
                id: rule_id,
                owner_id: state.owner_id,
                capability: enum_name(request.capability)?,
                effect: enum_name(request.effect)?,
                device_id: request.device_id,
                bot_id: request.bot_id,
                chat_id: request.chat_id,
                workspace_id: request.workspace_id,
                action_prefix: request.action_prefix,
                created_at_ms: now,
                updated_at_ms: now,
            })
            .await?
    };
    reload_policy(&state).await?;
    let rule = summary(rule)?;
    if !replayed {
        publish(
            &state,
            "capability_rule_changed",
            ServerEventBody::CapabilityRuleChanged { rule: rule.clone() },
        )
        .await?;
    }
    Ok(Json(rule))
}

pub(super) async fn delete(
    State(state): State<AppState>,
    Extension(identity): Extension<AuthenticatedIdentity>,
    Path(rule_id): Path<Uuid>,
    Json(request): Json<BotMutationRequest>,
) -> Result<StatusCode, ApiError> {
    require_owner(&identity)?;
    let replayed = matches!(
        claim(
            &state,
            request.idempotency_key,
            &format!("delete_capability_rule:{rule_id}"),
            &request,
        )
        .await?,
        IdempotencyClaim::Replayed { .. }
    );
    if !replayed {
        state
            .storage
            .delete_capability_rule(state.owner_id, rule_id, unix_time_ms())
            .await?;
        reload_policy(&state).await?;
        publish(
            &state,
            "capability_rule_removed",
            ServerEventBody::CapabilityRuleRemoved { rule_id },
        )
        .await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn reload_policy(state: &AppState) -> Result<(), homebot_storage::StorageError> {
    let rules = state
        .storage
        .capability_rules(state.owner_id)
        .await?
        .into_iter()
        .map(policy_rule)
        .collect::<Result<Vec<_>, _>>()?;
    state.policy_engine.replace_rules(rules).await;
    state
        .policy_loaded
        .store(true, std::sync::atomic::Ordering::Release);
    Ok(())
}

fn require_owner(identity: &AuthenticatedIdentity) -> Result<(), ApiError> {
    if identity == &AuthenticatedIdentity::Owner {
        Ok(())
    } else {
        Err(ApiError::forbidden(
            "Only the HomeBot owner can manage capability rules",
        ))
    }
}

pub(super) fn summary(rule: CapabilityRuleRecord) -> Result<CapabilityRuleSummary, ApiError> {
    Ok(CapabilityRuleSummary {
        id: rule.id,
        capability: parse_enum(&rule.capability)?,
        effect: parse_enum(&rule.effect)?,
        device_id: rule.device_id,
        bot_id: rule.bot_id,
        chat_id: rule.chat_id,
        workspace_id: rule.workspace_id,
        action_prefix: rule.action_prefix,
        created_at_ms: rule.created_at_ms,
        updated_at_ms: rule.updated_at_ms,
    })
}

fn audit_summary(record: CapabilityRuleAuditRecord) -> CapabilityRuleAuditSummary {
    CapabilityRuleAuditSummary {
        id: record.id,
        rule_id: record.rule_id,
        action: record.action,
        snapshot: record.snapshot,
        created_at_ms: record.created_at_ms,
    }
}

fn policy_rule(rule: CapabilityRuleRecord) -> Result<PolicyRule, homebot_storage::StorageError> {
    let capability: CapabilityClass =
        serde_json::from_value(serde_json::Value::String(rule.capability.clone()))
            .map_err(|error| homebot_storage::StorageError::Integrity(error.to_string()))?;
    let effect: PolicyEffect = serde_json::from_value(serde_json::Value::String(rule.effect))
        .map_err(|error| homebot_storage::StorageError::Integrity(error.to_string()))?;
    Ok(PolicyRule {
        id: rule.id,
        capability,
        owner_id: Some(rule.owner_id),
        device_id: rule.device_id,
        bot_id: rule.bot_id,
        chat_id: rule.chat_id,
        workspace_id: rule.workspace_id,
        action_prefix: rule.action_prefix,
        effect,
    })
}

fn enum_name<T: serde::Serialize>(value: T) -> Result<String, ApiError> {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .ok_or_else(ApiError::internal)
}

fn parse_enum<T: serde::de::DeserializeOwned>(value: &str) -> Result<T, ApiError> {
    serde_json::from_value(serde_json::Value::String(value.to_owned()))
        .map_err(|_| ApiError::internal())
}
