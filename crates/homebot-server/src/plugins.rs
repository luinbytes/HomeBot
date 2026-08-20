//! Authenticated plugin registry and local MCP lifecycle API.

use super::{
    AppState,
    bots::{ApiError, claim},
    persist_event, unix_time_ms,
};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use homebot_plugins::{LocalMcpAdapter, LocalMcpProfile, PluginAdapter};
use homebot_protocol::{
    CreateLocalMcpPluginRequest, PluginAssignmentRequest, PluginAuthState, PluginConnectionState,
    PluginMutationRequest, PluginSummary, PluginToolSummary, ServerEventBody,
};
use homebot_storage::{PluginConnectionUpdate, PluginRecord, PluginToolRecord};
use serde::{Deserialize, Serialize};
use std::{ffi::OsString, path::PathBuf};
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct LocalConfiguration {
    program: String,
    arguments: Vec<String>,
}

pub(super) async fn list(
    State(state): State<AppState>,
) -> Result<Json<Vec<PluginSummary>>, ApiError> {
    let records = state.storage.list_plugins(state.owner_id).await?;
    let mut result = Vec::with_capacity(records.len());
    for record in records {
        result.push(summary(&state, &record).await?);
    }
    Ok(Json(result))
}

pub(super) async fn create(
    State(state): State<AppState>,
    Json(request): Json<CreateLocalMcpPluginRequest>,
) -> Result<(StatusCode, Json<PluginSummary>), ApiError> {
    let _ = claim(
        &state,
        request.idempotency_key,
        "create_local_mcp_plugin",
        &request,
    )
    .await?;
    if let Ok(existing) = state
        .storage
        .plugin(state.owner_id, request.idempotency_key)
        .await
    {
        return Ok((StatusCode::OK, Json(summary(&state, &existing).await?)));
    }
    let name = visible(&request.name, 80, "Plugin name")?;
    let description = optional_visible(&request.description, 500, "Plugin description")?;
    let program = PathBuf::from(&request.program);
    if !program.is_absolute() || request.program.chars().any(char::is_control) {
        return Err(ApiError::validation(
            "Local MCP executable must be an absolute path",
        ));
    }
    if request.arguments.len() > 64
        || request
            .arguments
            .iter()
            .any(|value| value.len() > 4096 || value.chars().any(char::is_control))
    {
        return Err(ApiError::validation(
            "Local MCP arguments exceed safe limits",
        ));
    }
    let now = unix_time_ms();
    let configuration = serde_json::to_value(LocalConfiguration {
        program: request.program,
        arguments: request.arguments,
    })
    .map_err(|_| ApiError::internal())?;
    let record = PluginRecord {
        id: request.idempotency_key,
        owner_id: state.owner_id,
        name,
        description,
        kind: "local_mcp".to_owned(),
        configuration,
        enabled: false,
        connection_id: Uuid::now_v7(),
        transport: "stdio".to_owned(),
        status: "connect".to_owned(),
        auth_status: "not_required".to_owned(),
        error_message: None,
        updated_at_ms: now,
    };
    state.storage.create_plugin(&record).await?;
    let plugin = summary(&state, &record).await?;
    publish(&state, plugin.clone()).await?;
    Ok((StatusCode::CREATED, Json(plugin)))
}

pub(super) async fn connect(
    State(state): State<AppState>,
    Path(plugin_id): Path<Uuid>,
    Json(request): Json<PluginMutationRequest>,
) -> Result<Json<PluginSummary>, ApiError> {
    let _ = claim(
        &state,
        request.idempotency_key,
        &format!("connect_plugin:{plugin_id}"),
        &request,
    )
    .await?;
    let record = state.storage.plugin(state.owner_id, plugin_id).await?;
    let waiting = state
        .storage
        .update_plugin_connection(
            state.owner_id,
            plugin_id,
            PluginConnectionUpdate {
                enabled: false,
                status: "waiting",
                auth_status: "not_required",
                error_message: None,
                tools: &[],
                updated_at_ms: unix_time_ms(),
            },
        )
        .await?;
    publish(&state, summary(&state, &waiting).await?).await?;
    let adapter = adapter_for(&record)?;
    let updated = match adapter.discover_tools().await {
        Ok(discovered) => {
            let tools = discovered
                .into_iter()
                .map(|tool| PluginToolRecord {
                    name: tool.name,
                    title: tool.title,
                    description: tool.description,
                    input_schema: tool.input_schema,
                })
                .collect::<Vec<_>>();
            state
                .storage
                .update_plugin_connection(
                    state.owner_id,
                    plugin_id,
                    PluginConnectionUpdate {
                        enabled: true,
                        status: "connected",
                        auth_status: "not_required",
                        error_message: None,
                        tools: &tools,
                        updated_at_ms: unix_time_ms(),
                    },
                )
                .await?
        }
        Err(error) => {
            let message = error.to_string();
            state
                .storage
                .update_plugin_connection(
                    state.owner_id,
                    plugin_id,
                    PluginConnectionUpdate {
                        enabled: false,
                        status: "error",
                        auth_status: "not_required",
                        error_message: Some(&message),
                        tools: &[],
                        updated_at_ms: unix_time_ms(),
                    },
                )
                .await?
        }
    };
    let plugin = summary(&state, &updated).await?;
    publish(&state, plugin.clone()).await?;
    Ok(Json(plugin))
}

pub(super) fn adapter_for(record: &PluginRecord) -> Result<LocalMcpAdapter, ApiError> {
    let config: LocalConfiguration =
        serde_json::from_value(record.configuration.clone()).map_err(|_| ApiError::internal())?;
    let mut profile = LocalMcpProfile::new(config.program);
    profile.arguments = config.arguments.into_iter().map(OsString::from).collect();
    Ok(LocalMcpAdapter::new(profile))
}

pub(super) async fn enable(
    State(state): State<AppState>,
    Path(plugin_id): Path<Uuid>,
    Json(request): Json<PluginMutationRequest>,
) -> Result<Json<PluginSummary>, ApiError> {
    set_enabled(&state, plugin_id, request, true)
        .await
        .map(Json)
}

pub(super) async fn disable(
    State(state): State<AppState>,
    Path(plugin_id): Path<Uuid>,
    Json(request): Json<PluginMutationRequest>,
) -> Result<Json<PluginSummary>, ApiError> {
    set_enabled(&state, plugin_id, request, false)
        .await
        .map(Json)
}

async fn set_enabled(
    state: &AppState,
    plugin_id: Uuid,
    request: PluginMutationRequest,
    enabled: bool,
) -> Result<PluginSummary, ApiError> {
    let operation = if enabled {
        "enable_plugin"
    } else {
        "disable_plugin"
    };
    let _ = claim(
        state,
        request.idempotency_key,
        &format!("{operation}:{plugin_id}"),
        &request,
    )
    .await?;
    let record = state.storage.plugin(state.owner_id, plugin_id).await?;
    if enabled && record.status != "connected" {
        return Err(ApiError::conflict("Connect the plugin before enabling it"));
    }
    let tools = state
        .storage
        .plugin_tools(state.owner_id, plugin_id)
        .await?;
    let state_name = if enabled { "connected" } else { "reopen" };
    let updated = state
        .storage
        .update_plugin_connection(
            state.owner_id,
            plugin_id,
            PluginConnectionUpdate {
                enabled,
                status: state_name,
                auth_status: &record.auth_status,
                error_message: None,
                tools: &tools,
                updated_at_ms: unix_time_ms(),
            },
        )
        .await?;
    let plugin = summary(state, &updated).await?;
    publish(state, plugin.clone()).await?;
    Ok(plugin)
}

pub(super) async fn assign(
    State(state): State<AppState>,
    Path(plugin_id): Path<Uuid>,
    Json(request): Json<PluginAssignmentRequest>,
) -> Result<Json<PluginSummary>, ApiError> {
    let _ = claim(
        &state,
        request.idempotency_key,
        &format!("assign_plugin:{plugin_id}:{}", request.bot_id),
        &request,
    )
    .await?;
    state
        .storage
        .set_plugin_assignment(state.owner_id, plugin_id, request.bot_id, request.enabled)
        .await?;
    let plugin = summary(
        &state,
        &state.storage.plugin(state.owner_id, plugin_id).await?,
    )
    .await?;
    publish(&state, plugin.clone()).await?;
    Ok(Json(plugin))
}

pub(super) async fn delete(
    State(state): State<AppState>,
    Path(plugin_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    state
        .storage
        .delete_plugin(state.owner_id, plugin_id)
        .await?;
    persist_event(
        &state,
        "plugin_removed",
        ServerEventBody::PluginRemoved { plugin_id },
    )
    .await
    .map_err(|()| ApiError::internal())?;
    Ok(StatusCode::NO_CONTENT)
}

async fn summary(state: &AppState, record: &PluginRecord) -> Result<PluginSummary, ApiError> {
    let tools = state
        .storage
        .plugin_tools(state.owner_id, record.id)
        .await?
        .into_iter()
        .map(|tool| PluginToolSummary {
            name: tool.name,
            title: tool.title,
            description: tool.description,
            input_schema: tool.input_schema,
        })
        .collect();
    Ok(PluginSummary {
        id: record.id,
        name: record.name.clone(),
        description: record.description.clone(),
        kind: record.kind.clone(),
        enabled: record.enabled,
        connection_state: match record.status.as_str() {
            "connect" => PluginConnectionState::Connect,
            "waiting" => PluginConnectionState::Waiting,
            "reopen" => PluginConnectionState::Reopen,
            "connected" => PluginConnectionState::Connected,
            _ => PluginConnectionState::Error,
        },
        auth_state: match record.auth_status.as_str() {
            "not_required" => PluginAuthState::NotRequired,
            "required" => PluginAuthState::Required,
            "waiting" => PluginAuthState::Waiting,
            "connected" => PluginAuthState::Connected,
            _ => PluginAuthState::Error,
        },
        error_message: record.error_message.clone(),
        tools,
        bot_ids: state
            .storage
            .plugin_bot_ids(state.owner_id, record.id)
            .await?,
        updated_at_unix_ms: u64::try_from(record.updated_at_ms).unwrap_or_default(),
    })
}

async fn publish(state: &AppState, plugin: PluginSummary) -> Result<(), ApiError> {
    persist_event(
        state,
        "plugin_changed",
        ServerEventBody::PluginChanged { plugin },
    )
    .await
    .map(|_| ())
    .map_err(|()| ApiError::internal())
}

fn visible(value: &str, max: usize, label: &str) -> Result<String, ApiError> {
    let value = value.trim();
    if value.is_empty() || value.len() > max || value.chars().any(char::is_control) {
        return Err(ApiError::validation(&format!("{label} is invalid")));
    }
    Ok(value.to_owned())
}

fn optional_visible(value: &str, max: usize, label: &str) -> Result<String, ApiError> {
    if value.len() > max || value.chars().any(char::is_control) {
        return Err(ApiError::validation(&format!("{label} is invalid")));
    }
    Ok(value.trim().to_owned())
}
