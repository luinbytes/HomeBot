//! Authenticated versioned routine editor, recorder, and manual replay API.

use super::{
    AppState, AuthenticatedIdentity,
    bots::{ApiError, claim},
    persist_event, unix_time_ms,
};
use async_trait::async_trait;
use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
};
use homebot_plugins::PluginAdapter;
use homebot_protocol::{
    AppendRoutineRecordingRequest, BotMutationRequest, CreateRoutineRequest,
    DuplicateRoutineRequest, PluginMutationRequest, RoutineRecordingSummary, RoutineRunSummary,
    RoutineStepStatus, RoutineSummary, RunRoutineRequest, ServerEventBody,
    StartRoutineRecordingRequest, UpdateRoutineRequest,
};
use homebot_routines::{
    RoutineActionExecutor, RoutineError, RoutineStep, definition_from_recording, replay, validate,
};
use homebot_storage::IdempotencyClaim;
use homebot_storage::{
    RoutineJobRecord, RoutineRecord, RoutineRecordingRecord, RoutineRunRecord, RoutineUpdate,
};
use homebot_tools::{CapabilityClass, CapabilityRequest, OperationContext, PolicyEffect};
use serde_json::{Value, json};
use uuid::Uuid;

pub(super) async fn list(
    State(state): State<AppState>,
) -> Result<Json<Vec<RoutineSummary>>, ApiError> {
    Ok(Json(
        state
            .storage
            .list_routines(state.owner_id)
            .await?
            .iter()
            .map(summary)
            .collect(),
    ))
}

pub(super) async fn create(
    State(state): State<AppState>,
    Json(request): Json<CreateRoutineRequest>,
) -> Result<(StatusCode, Json<RoutineSummary>), ApiError> {
    let _ = claim(&state, request.idempotency_key, "create_routine", &request).await?;
    if let Ok(existing) = state
        .storage
        .routine(state.owner_id, request.idempotency_key)
        .await
    {
        return Ok((StatusCode::OK, Json(summary(&existing))));
    }
    validate(&request.definition).map_err(|error| routine_validation(&error))?;
    let now = unix_time_ms();
    let record = RoutineRecord {
        id: request.idempotency_key,
        owner_id: state.owner_id,
        bot_id: request.bot_id,
        name: visible(&request.name, 80, "Routine name")?,
        description: optional_visible(&request.description, 500, "Routine description")?,
        enabled: !request.draft,
        draft: request.draft,
        active_version_id: Uuid::now_v7(),
        version: 1,
        definition: request.definition,
        created_at_ms: now,
        updated_at_ms: now,
    };
    state.storage.create_routine(&record).await?;
    let routine = summary(&record);
    publish_routine(&state, routine.clone()).await?;
    Ok((StatusCode::CREATED, Json(routine)))
}

pub(super) async fn update(
    State(state): State<AppState>,
    Path(routine_id): Path<Uuid>,
    Json(request): Json<UpdateRoutineRequest>,
) -> Result<Json<RoutineSummary>, ApiError> {
    let _ = claim(
        &state,
        request.idempotency_key,
        &format!("update_routine:{routine_id}"),
        &request,
    )
    .await?;
    validate(&request.definition).map_err(|error| routine_validation(&error))?;
    let name = visible(&request.name, 80, "Routine name")?;
    let description = optional_visible(&request.description, 500, "Routine description")?;
    let record = state
        .storage
        .update_routine(
            state.owner_id,
            routine_id,
            RoutineUpdate {
                name: &name,
                description: &description,
                definition: &request.definition,
                draft: request.draft,
                updated_at_ms: unix_time_ms(),
            },
        )
        .await?;
    let routine = summary(&record);
    publish_routine(&state, routine.clone()).await?;
    Ok(Json(routine))
}

pub(super) async fn duplicate(
    State(state): State<AppState>,
    Path(routine_id): Path<Uuid>,
    Json(request): Json<DuplicateRoutineRequest>,
) -> Result<(StatusCode, Json<RoutineSummary>), ApiError> {
    let _ = claim(
        &state,
        request.idempotency_key,
        &format!("duplicate_routine:{routine_id}"),
        &request,
    )
    .await?;
    if let Ok(existing) = state
        .storage
        .routine(state.owner_id, request.idempotency_key)
        .await
    {
        return Ok((StatusCode::OK, Json(summary(&existing))));
    }
    let source = state.storage.routine(state.owner_id, routine_id).await?;
    let now = unix_time_ms();
    let record = RoutineRecord {
        id: request.idempotency_key,
        name: visible(&request.name, 80, "Routine name")?,
        active_version_id: Uuid::now_v7(),
        version: 1,
        enabled: false,
        draft: true,
        created_at_ms: now,
        updated_at_ms: now,
        ..source
    };
    state.storage.create_routine(&record).await?;
    let routine = summary(&record);
    publish_routine(&state, routine.clone()).await?;
    Ok((StatusCode::CREATED, Json(routine)))
}

pub(super) async fn enable(
    State(state): State<AppState>,
    Path(routine_id): Path<Uuid>,
    Json(request): Json<PluginMutationRequest>,
) -> Result<Json<RoutineSummary>, ApiError> {
    set_enabled(&state, routine_id, request, true)
        .await
        .map(Json)
}
pub(super) async fn disable(
    State(state): State<AppState>,
    Path(routine_id): Path<Uuid>,
    Json(request): Json<PluginMutationRequest>,
) -> Result<Json<RoutineSummary>, ApiError> {
    set_enabled(&state, routine_id, request, false)
        .await
        .map(Json)
}
async fn set_enabled(
    state: &AppState,
    id: Uuid,
    request: PluginMutationRequest,
    enabled: bool,
) -> Result<RoutineSummary, ApiError> {
    let _ = claim(
        state,
        request.idempotency_key,
        &format!("routine_enabled:{id}:{enabled}"),
        &request,
    )
    .await?;
    let current = state.storage.routine(state.owner_id, id).await?;
    if enabled && current.draft {
        return Err(ApiError::conflict(
            "Finish editing the routine draft before enabling it",
        ));
    }
    let routine = summary(
        &state
            .storage
            .set_routine_enabled(state.owner_id, id, enabled, unix_time_ms())
            .await?,
    );
    publish_routine(state, routine.clone()).await?;
    Ok(routine)
}

pub(super) async fn delete(
    State(state): State<AppState>,
    Path(routine_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    state
        .storage
        .delete_routine(state.owner_id, routine_id)
        .await?;
    persist_event(
        &state,
        "routine_removed",
        ServerEventBody::RoutineRemoved { routine_id },
    )
    .await
    .map_err(|()| ApiError::internal())?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn start_recording(
    State(state): State<AppState>,
    Json(request): Json<StartRoutineRecordingRequest>,
) -> Result<(StatusCode, Json<RoutineRecordingSummary>), ApiError> {
    let _ = claim(
        &state,
        request.idempotency_key,
        "start_routine_recording",
        &request,
    )
    .await?;
    if let Ok(existing) = state
        .storage
        .routine_recording(state.owner_id, request.idempotency_key)
        .await
    {
        return Ok((StatusCode::OK, Json(recording_summary(&existing))));
    }
    let now = unix_time_ms();
    let record = RoutineRecordingRecord {
        id: request.idempotency_key,
        owner_id: state.owner_id,
        bot_id: request.bot_id,
        name: visible(&request.name, 80, "Routine name")?,
        description: optional_visible(&request.description, 500, "Routine description")?,
        actions: Vec::new(),
        created_at_ms: now,
        updated_at_ms: now,
    };
    state.storage.create_routine_recording(&record).await?;
    let recording = recording_summary(&record);
    publish_recording(&state, recording.clone()).await?;
    Ok((StatusCode::CREATED, Json(recording)))
}

pub(super) async fn append_recording(
    State(state): State<AppState>,
    Path(recording_id): Path<Uuid>,
    Json(request): Json<AppendRoutineRecordingRequest>,
) -> Result<Json<RoutineRecordingSummary>, ApiError> {
    let _ = claim(
        &state,
        request.idempotency_key,
        &format!("append_recording:{recording_id}"),
        &request,
    )
    .await?;
    let record = state
        .storage
        .append_routine_recording_action(
            state.owner_id,
            recording_id,
            &request.action,
            unix_time_ms(),
        )
        .await?;
    let recording = recording_summary(&record);
    publish_recording(&state, recording.clone()).await?;
    Ok(Json(recording))
}

pub(super) async fn finish_recording(
    State(state): State<AppState>,
    Path(recording_id): Path<Uuid>,
    Json(request): Json<BotMutationRequest>,
) -> Result<(StatusCode, Json<RoutineSummary>), ApiError> {
    let _ = claim(
        &state,
        request.idempotency_key,
        &format!("finish_recording:{recording_id}"),
        &request,
    )
    .await?;
    if let Ok(existing) = state
        .storage
        .routine(state.owner_id, request.idempotency_key)
        .await
    {
        return Ok((StatusCode::OK, Json(summary(&existing))));
    }
    let recording = state
        .storage
        .routine_recording(state.owner_id, recording_id)
        .await?;
    let definition =
        definition_from_recording(recording.actions).map_err(|error| routine_validation(&error))?;
    let now = unix_time_ms();
    let record = RoutineRecord {
        id: request.idempotency_key,
        owner_id: state.owner_id,
        bot_id: recording.bot_id,
        name: recording.name,
        description: recording.description,
        enabled: false,
        draft: true,
        active_version_id: Uuid::now_v7(),
        version: 1,
        definition,
        created_at_ms: now,
        updated_at_ms: now,
    };
    state.storage.create_routine(&record).await?;
    let _ = state
        .storage
        .close_routine_recording(state.owner_id, recording_id, true, unix_time_ms())
        .await?;
    let routine = summary(&record);
    publish_routine(&state, routine.clone()).await?;
    Ok((StatusCode::CREATED, Json(routine)))
}

pub(super) async fn cancel_recording(
    State(state): State<AppState>,
    Path(recording_id): Path<Uuid>,
    Json(request): Json<BotMutationRequest>,
) -> Result<StatusCode, ApiError> {
    let _ = claim(
        &state,
        request.idempotency_key,
        &format!("cancel_recording:{recording_id}"),
        &request,
    )
    .await?;
    let _ = state
        .storage
        .close_routine_recording(state.owner_id, recording_id, false, unix_time_ms())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn run_now(
    State(state): State<AppState>,
    Extension(identity): Extension<AuthenticatedIdentity>,
    Path(routine_id): Path<Uuid>,
    Json(request): Json<RunRoutineRequest>,
) -> Result<Json<RoutineRunSummary>, ApiError> {
    execute(&state, routine_id, request, false, identity.device_id())
        .await
        .map(Json)
}
pub(super) async fn dry_run(
    State(state): State<AppState>,
    Extension(identity): Extension<AuthenticatedIdentity>,
    Path(routine_id): Path<Uuid>,
    Json(request): Json<RunRoutineRequest>,
) -> Result<Json<RoutineRunSummary>, ApiError> {
    execute(&state, routine_id, request, true, identity.device_id())
        .await
        .map(Json)
}

async fn execute(
    state: &AppState,
    routine_id: Uuid,
    request: RunRoutineRequest,
    dry_run: bool,
    device_id: Uuid,
) -> Result<RoutineRunSummary, ApiError> {
    let claim = claim(
        state,
        request.idempotency_key,
        &format!("run_routine:{routine_id}:{dry_run}"),
        &request,
    )
    .await?;
    if matches!(claim, IdempotencyClaim::Replayed { .. }) {
        let prior = state
            .storage
            .routine_runs(state.owner_id, routine_id)
            .await?
            .into_iter()
            .find(|run| run.id == request.idempotency_key)
            .ok_or_else(ApiError::internal)?;
        return Ok(run_summary(&prior));
    }
    let routine = state.storage.routine(state.owner_id, routine_id).await?;
    if routine.draft || (!dry_run && !routine.enabled) {
        return Err(ApiError::conflict(
            "Routine must be published and enabled before it can run",
        ));
    }
    let started = unix_time_ms();
    let executor = ServerExecutor {
        state,
        routine_bot_id: routine.bot_id,
        operation_id: request.request_id,
        device_id,
    };
    let (results, status, error_message) =
        match replay(&executor, &routine.definition, &request.inputs, dry_run).await {
            Ok(results) => {
                let status = if results
                    .iter()
                    .any(|step| step.status == RoutineStepStatus::ApprovalRequired)
                {
                    "waiting_approval"
                } else if dry_run {
                    "dry_run_succeeded"
                } else {
                    "succeeded"
                };
                (results, status, None)
            }
            Err(error) => (Vec::new(), "failed", Some(routine_error_message(&error))),
        };
    let record = RoutineRunRecord {
        id: request.idempotency_key,
        owner_id: state.owner_id,
        routine_id,
        routine_version_id: routine.active_version_id,
        bot_id: routine.bot_id,
        status: status.to_owned(),
        trigger: serde_json::json!({"kind": "manual"}),
        dry_run,
        inputs: redacted_input_metadata(&routine.definition, &request.inputs),
        results: Some(results),
        error_message,
        attempt_count: 1,
        scheduled_for_ms: None,
        started_at_ms: started,
        finished_at_ms: Some(unix_time_ms()),
    };
    state.storage.create_routine_run(&record).await?;
    let run = run_summary(&record);
    publish_run(state, run.clone()).await?;
    Ok(run)
}

pub(super) async fn execute_job(
    state: &AppState,
    job: &RoutineJobRecord,
) -> Result<RoutineRunSummary, ApiError> {
    let current = state
        .storage
        .routine(state.owner_id, job.routine_id)
        .await?;
    if !current.enabled || current.draft {
        return Err(ApiError::validation("Routine is disabled or still a draft"));
    }
    let routine = state
        .storage
        .routine_version(state.owner_id, job.routine_id, job.routine_version_id)
        .await?;
    let started = unix_time_ms();
    let executor = ServerExecutor {
        state,
        routine_bot_id: routine.bot_id,
        operation_id: job.id,
        device_id: Uuid::nil(),
    };
    let (results, status, error_message) =
        match replay(&executor, &routine.definition, &job.inputs, false).await {
            Ok(results) => {
                let status = if results
                    .iter()
                    .any(|step| step.status == RoutineStepStatus::ApprovalRequired)
                {
                    "waiting_approval"
                } else {
                    "succeeded"
                };
                (results, status, None)
            }
            Err(error) => (Vec::new(), "failed", Some(routine_error_message(&error))),
        };
    let record = RoutineRunRecord {
        id: Uuid::now_v7(),
        owner_id: state.owner_id,
        routine_id: job.routine_id,
        routine_version_id: job.routine_version_id,
        bot_id: routine.bot_id,
        status: status.to_owned(),
        trigger: job.trigger.clone(),
        dry_run: false,
        inputs: redacted_input_metadata(&routine.definition, &job.inputs),
        results: Some(results),
        error_message,
        attempt_count: job.attempt_count,
        scheduled_for_ms: Some(job.scheduled_for_ms),
        started_at_ms: started,
        finished_at_ms: Some(unix_time_ms()),
    };
    state.storage.create_routine_run(&record).await?;
    let run = run_summary(&record);
    publish_run(state, run.clone()).await?;
    Ok(run)
}

fn redacted_input_metadata(
    definition: &homebot_routines::RoutineDefinition,
    inputs: &Value,
) -> Value {
    let fields = definition
        .inputs
        .iter()
        .map(|input| {
            (
                input.key.clone(),
                json!({"kind":input.kind,"present":inputs.get(&input.key).is_some()}),
            )
        })
        .collect();
    Value::Object(fields)
}

pub(super) async fn runs(
    State(state): State<AppState>,
    Path(routine_id): Path<Uuid>,
) -> Result<Json<Vec<RoutineRunSummary>>, ApiError> {
    Ok(Json(
        state
            .storage
            .routine_runs(state.owner_id, routine_id)
            .await?
            .iter()
            .map(run_summary)
            .collect(),
    ))
}

struct ServerExecutor<'a> {
    state: &'a AppState,
    routine_bot_id: Uuid,
    operation_id: Uuid,
    device_id: Uuid,
}
#[async_trait]
impl RoutineActionExecutor for ServerExecutor<'_> {
    async fn validate_step(&self, step: &RoutineStep, _inputs: &Value) -> Result<(), RoutineError> {
        match step {
            RoutineStep::BotPrompt { bot_id, .. } => {
                if *bot_id != self.routine_bot_id {
                    return Err(RoutineError::Invalid("step Bot differs from routine Bot"));
                }
                self.state
                    .storage
                    .get_bot(self.state.owner_id, *bot_id)
                    .await
                    .map_err(|_| RoutineError::Invalid("Bot unavailable"))?;
            }
            RoutineStep::PluginTool {
                plugin_id,
                tool_name,
                ..
            } => {
                let plugin = self
                    .state
                    .storage
                    .plugin(self.state.owner_id, *plugin_id)
                    .await
                    .map_err(|_| RoutineError::Invalid("plugin unavailable"))?;
                let bots = self
                    .state
                    .storage
                    .plugin_bot_ids(self.state.owner_id, *plugin_id)
                    .await
                    .map_err(|_| RoutineError::Invalid("plugin assignment unavailable"))?;
                let tools = self
                    .state
                    .storage
                    .plugin_tools(self.state.owner_id, *plugin_id)
                    .await
                    .map_err(|_| RoutineError::Invalid("plugin tools unavailable"))?;
                if !plugin.enabled
                    || !bots.contains(&self.routine_bot_id)
                    || !tools.iter().any(|tool| tool.name == *tool_name)
                {
                    return Err(RoutineError::Invalid(
                        "plugin tool is not available to this Bot",
                    ));
                }
            }
            RoutineStep::RecordOutput { .. } => {}
        }
        Ok(())
    }

    async fn approval_required(
        &self,
        step: &RoutineStep,
        _inputs: &Value,
    ) -> Result<bool, RoutineError> {
        let RoutineStep::PluginTool {
            plugin_id,
            tool_name,
            requires_approval,
            ..
        } = step
        else {
            return Ok(step.requires_approval());
        };
        self.state
            .ensure_policy_loaded()
            .await
            .map_err(|_| RoutineError::StepFailed)?;
        let request = CapabilityRequest {
            context: OperationContext {
                operation_id: self.operation_id,
                owner_id: self.state.owner_id,
                device_id: self.device_id,
                bot_id: self.routine_bot_id,
                chat_id: Uuid::nil(),
                workspace_id: Uuid::nil(),
            },
            capability: CapabilityClass::PluginWrite,
            action: format!("plugin.tool.call.{tool_name}"),
            canonical_resource: format!("plugin:{plugin_id}:tool:{tool_name}"),
            summary: format!("Run plugin tool {tool_name} from a routine"),
            destructive: true,
        };
        match self.state.policy_engine.effect_for(&request).await {
            PolicyEffect::Deny => Err(RoutineError::StepFailed),
            PolicyEffect::RequireApproval => Ok(true),
            PolicyEffect::Allow => Ok(*requires_approval),
        }
    }

    async fn execute_step(
        &self,
        step: &RoutineStep,
        inputs: &Value,
    ) -> Result<Value, RoutineError> {
        Ok(match step {
            RoutineStep::BotPrompt {
                bot_id,
                prompt_template,
                ..
            } => {
                let prompt = render_template(prompt_template, inputs);
                let applied_skills = self
                    .state
                    .storage
                    .resolve_applied_skills(self.state.owner_id, *bot_id, &[])
                    .await
                    .map_err(|_| RoutineError::StepFailed)?;
                let provider_prompt = super::chats::prompt_with_skills(&prompt, &applied_skills)
                    .map_err(|_| RoutineError::StepFailed)?;
                let chat = self
                    .state
                    .storage
                    .create_direct_chat(
                        self.state.owner_id,
                        *bot_id,
                        Uuid::now_v7(),
                        unix_time_ms(),
                    )
                    .await
                    .map_err(|_| RoutineError::StepFailed)?;
                let message = self
                    .state
                    .storage
                    .append_user_message(
                        self.state.owner_id,
                        chat.id,
                        Uuid::now_v7(),
                        &prompt,
                        &[],
                        None,
                        Vec::new(),
                        &applied_skills,
                        &[],
                        unix_time_ms(),
                    )
                    .await
                    .map_err(|_| RoutineError::StepFailed)?;
                super::chats::publish(
                    self.state,
                    "chat_changed",
                    ServerEventBody::ChatChanged {
                        chat: super::chats::chat_summary(chat.clone()),
                    },
                )
                .await
                .map_err(|_| RoutineError::StepFailed)?;
                let message_id = message.id;
                let message = super::chats::message_summary(self.state, message)
                    .await
                    .map_err(|_| RoutineError::StepFailed)?;
                super::chats::publish(
                    self.state,
                    "message_changed",
                    ServerEventBody::MessageChanged { message },
                )
                .await
                .map_err(|_| RoutineError::StepFailed)?;
                let provider_started = super::provider_turn::start_if_configured(
                    self.state,
                    &chat,
                    &provider_prompt,
                    &[],
                )
                .await
                .map_err(|_| RoutineError::StepFailed)?;
                json!({"kind":"bot_prompt","bot_id":bot_id,"chat_id":chat.id,"message_id":message_id,"provider_started":provider_started})
            }
            RoutineStep::PluginTool {
                plugin_id,
                tool_name,
                arguments,
                ..
            } => {
                let plugin = self
                    .state
                    .storage
                    .plugin(self.state.owner_id, *plugin_id)
                    .await
                    .map_err(|_| RoutineError::StepFailed)?;
                let adapter =
                    super::plugins::adapter_for(&plugin).map_err(|_| RoutineError::StepFailed)?;
                let rendered_arguments = render_value_templates(arguments, inputs);
                let _untrusted = adapter
                    .call_tool(*plugin_id, tool_name, &rendered_arguments)
                    .await
                    .map_err(|_| RoutineError::StepFailed)?;
                json!({"kind":"plugin_tool","plugin_id":plugin_id,"tool_name":tool_name,"status":"succeeded","trust":"untrusted"})
            }
            RoutineStep::RecordOutput {
                output_key,
                value_template,
            } => json!({"kind":"output","key":output_key,"value":value_template}),
        })
    }
}

fn render_value_templates(value: &Value, inputs: &Value) -> Value {
    match value {
        Value::String(text) => Value::String(render_template(text, inputs)),
        Value::Array(values) => Value::Array(
            values
                .iter()
                .map(|value| render_value_templates(value, inputs))
                .collect(),
        ),
        Value::Object(values) => Value::Object(
            values
                .iter()
                .map(|(key, value)| (key.clone(), render_value_templates(value, inputs)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn render_template(template: &str, inputs: &Value) -> String {
    let mut rendered = template.to_owned();
    if let Some(object) = inputs.as_object() {
        for (key, value) in object {
            let replacement = value
                .as_str()
                .map_or_else(|| value.to_string(), str::to_owned);
            rendered = rendered.replace(&format!("{{{{{key}}}}}"), &replacement);
        }
    }
    rendered
}

fn summary(record: &RoutineRecord) -> RoutineSummary {
    RoutineSummary {
        id: record.id,
        bot_id: record.bot_id,
        name: record.name.clone(),
        description: record.description.clone(),
        enabled: record.enabled,
        draft: record.draft,
        active_version_id: record.active_version_id,
        version: record.version,
        definition: record.definition.clone(),
        created_at_unix_ms: millis(record.created_at_ms),
        updated_at_unix_ms: millis(record.updated_at_ms),
    }
}
fn recording_summary(record: &RoutineRecordingRecord) -> RoutineRecordingSummary {
    RoutineRecordingSummary {
        id: record.id,
        bot_id: record.bot_id,
        name: record.name.clone(),
        description: record.description.clone(),
        actions: record.actions.clone(),
        created_at_unix_ms: millis(record.created_at_ms),
        updated_at_unix_ms: millis(record.updated_at_ms),
    }
}
fn run_summary(record: &RoutineRunRecord) -> RoutineRunSummary {
    RoutineRunSummary {
        id: record.id,
        routine_id: record.routine_id,
        routine_version_id: record.routine_version_id,
        bot_id: record.bot_id,
        status: record.status.clone(),
        trigger: record.trigger.clone(),
        input_metadata: record.inputs.clone(),
        dry_run: record.dry_run,
        results: record.results.clone().unwrap_or_default(),
        error_message: record.error_message.clone(),
        attempt_count: record.attempt_count,
        scheduled_for_unix_ms: record.scheduled_for_ms.map(millis),
        started_at_unix_ms: millis(record.started_at_ms),
        finished_at_unix_ms: record.finished_at_ms.map(millis),
    }
}
fn millis(value: i64) -> u64 {
    u64::try_from(value).unwrap_or_default()
}
async fn publish_routine(state: &AppState, routine: RoutineSummary) -> Result<(), ApiError> {
    persist_event(
        state,
        "routine_changed",
        ServerEventBody::RoutineChanged { routine },
    )
    .await
    .map(|_| ())
    .map_err(|()| ApiError::internal())
}
async fn publish_recording(
    state: &AppState,
    recording: RoutineRecordingSummary,
) -> Result<(), ApiError> {
    persist_event(
        state,
        "routine_recording_changed",
        ServerEventBody::RoutineRecordingChanged { recording },
    )
    .await
    .map(|_| ())
    .map_err(|()| ApiError::internal())
}
async fn publish_run(state: &AppState, run: RoutineRunSummary) -> Result<(), ApiError> {
    persist_event(
        state,
        "routine_run_changed",
        ServerEventBody::RoutineRunChanged { run },
    )
    .await
    .map(|_| ())
    .map_err(|()| ApiError::internal())
}
fn routine_validation(error: &RoutineError) -> ApiError {
    ApiError::validation(&routine_error_message(error))
}

fn routine_error_message(error: &RoutineError) -> String {
    match error {
        RoutineError::Empty => "routine must contain at least one structured step".to_owned(),
        RoutineError::Invalid(detail) => format!("routine definition is invalid: {detail}"),
        RoutineError::StepFailed => "routine step failed".to_owned(),
    }
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
