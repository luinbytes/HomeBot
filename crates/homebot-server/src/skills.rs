//! Authenticated, owner-scoped, versioned Skill library API.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use homebot_protocol::{
    BotMutationRequest, CreateSkillRequest, DuplicateSkillRequest, ImportSkillRequest,
    ServerEventBody, SkillAssignmentRequest, SkillBundle, SkillImportConflictPolicy, SkillSummary,
    SkillTestSummary, UpdateSkillRequest,
};
use homebot_routines::{RecordedAction, RecordedActor, RoutineStep};
use homebot_skills::{AppliedSkill, SkillContext, SkillDefinition};
use homebot_storage::{IdempotencyClaim, SkillRecord};
use std::fmt::Write;
use uuid::Uuid;

use crate::{
    AppState,
    bots::{ApiError, claim},
    persist_event, unix_time_ms,
};

const SKILL_BUNDLE_FORMAT_VERSION: u16 = 1;

pub(super) async fn list(
    State(state): State<AppState>,
) -> Result<Json<Vec<SkillSummary>>, ApiError> {
    Ok(Json(
        state
            .storage
            .list_skills(state.owner_id)
            .await?
            .iter()
            .map(summary)
            .collect(),
    ))
}

pub(super) async fn create(
    State(state): State<AppState>,
    Json(request): Json<CreateSkillRequest>,
) -> Result<(StatusCode, Json<SkillSummary>), ApiError> {
    let replayed = matches!(
        claim(&state, request.idempotency_key, "create_skill", &request).await?,
        IdempotencyClaim::Replayed { .. }
    );
    if replayed {
        let existing = match state
            .storage
            .skill(state.owner_id, request.idempotency_key)
            .await
        {
            Ok(skill) => skill,
            Err(_) => {
                state
                    .storage
                    .skill_version(state.owner_id, request.idempotency_key)
                    .await?
            }
        };
        return Ok((StatusCode::OK, Json(summary(&existing))));
    }
    let now = unix_time_ms();
    let record = SkillRecord {
        id: request.idempotency_key,
        owner_id: state.owner_id,
        name: visible(&request.name, 80, "Skill name")?,
        description: optional_visible(&request.description, 500, "Skill description")?,
        active_version_id: Uuid::now_v7(),
        version: 1,
        definition: request.definition,
        bot_ids: Vec::new(),
        created_at_ms: now,
        updated_at_ms: now,
    };
    homebot_skills::validate(&record.definition)
        .map_err(|error| ApiError::validation(&error.to_string()))?;
    state.storage.create_skill(&record).await?;
    let skill = summary(&record);
    publish(&state, skill.clone()).await?;
    Ok((StatusCode::CREATED, Json(skill)))
}

pub(super) async fn finish_recording(
    State(state): State<AppState>,
    Path(recording_id): Path<Uuid>,
    Json(request): Json<BotMutationRequest>,
) -> Result<(StatusCode, Json<SkillSummary>), ApiError> {
    let replayed = matches!(
        claim(
            &state,
            request.idempotency_key,
            &format!("finish_skill_recording:{recording_id}"),
            &request,
        )
        .await?,
        IdempotencyClaim::Replayed { .. }
    );
    if replayed {
        return Ok((
            StatusCode::OK,
            Json(summary(
                &state
                    .storage
                    .skill(state.owner_id, request.idempotency_key)
                    .await?,
            )),
        ));
    }
    let recording = state
        .storage
        .routine_recording(state.owner_id, recording_id)
        .await?;
    let definition = definition_from_demonstration(&recording.actions)?;
    let now = unix_time_ms();
    let record = SkillRecord {
        id: request.idempotency_key,
        owner_id: state.owner_id,
        name: visible(&recording.name, 80, "Skill name")?,
        description: optional_visible(&recording.description, 500, "Skill description")?,
        active_version_id: Uuid::now_v7(),
        version: 1,
        definition,
        bot_ids: Vec::new(),
        created_at_ms: now,
        updated_at_ms: now,
    };
    homebot_skills::validate(&record.definition)
        .map_err(|error| ApiError::validation(&error.to_string()))?;
    state.storage.create_skill(&record).await?;
    let _ = state
        .storage
        .close_routine_recording(state.owner_id, recording_id, true, now)
        .await?;
    let skill = summary(&record);
    publish(&state, skill.clone()).await?;
    Ok((StatusCode::CREATED, Json(skill)))
}

pub(super) async fn test(
    State(state): State<AppState>,
    Path(skill_id): Path<Uuid>,
    Json(request): Json<BotMutationRequest>,
) -> Result<Json<SkillTestSummary>, ApiError> {
    let _ = claim(
        &state,
        request.idempotency_key,
        &format!("test_skill:{skill_id}"),
        &request,
    )
    .await?;
    let record = state.storage.skill(state.owner_id, skill_id).await?;
    let prompt_preview = homebot_skills::assemble(&[AppliedSkill {
        skill_id: record.id,
        version_id: record.active_version_id,
        name: record.name,
        version: record.version,
        definition: record.definition,
    }])
    .map_err(|error| ApiError::validation(&error.to_string()))?;
    Ok(Json(SkillTestSummary {
        skill_id: record.id,
        skill_version_id: record.active_version_id,
        version: record.version,
        prompt_preview,
        capability_policy_enforced: true,
    }))
}

fn definition_from_demonstration(actions: &[RecordedAction]) -> Result<SkillDefinition, ApiError> {
    if actions.is_empty() {
        return Err(ApiError::validation(
            "A demonstrated Skill requires at least one recorded action",
        ));
    }
    let mut instructions = String::from("Follow this demonstrated workflow in order:\n");
    for (index, action) in actions.iter().enumerate() {
        let actor = match action.actor {
            RecordedActor::User => "User",
            RecordedActor::Bot => "Bot",
        };
        let step = match &action.step {
            RoutineStep::BotPrompt {
                bot_id,
                prompt_template,
                requires_approval,
            } => format!(
                "ask Bot {bot_id}: {prompt_template}{}",
                if *requires_approval {
                    " (approval is required)"
                } else {
                    ""
                }
            ),
            RoutineStep::PluginTool {
                plugin_id,
                tool_name,
                arguments,
                requires_approval,
            } => format!(
                "request plugin {plugin_id} tool {tool_name} with arguments {arguments}{}",
                if *requires_approval {
                    " (approval is required)"
                } else {
                    ""
                }
            ),
            RoutineStep::RecordOutput {
                output_key,
                value_template,
            } => format!("record {output_key} as {value_template}"),
        };
        let _ = writeln!(instructions, "{}. [{actor}] {step}", index + 1);
    }
    let recorded = serde_json::to_string_pretty(actions).map_err(|_| ApiError::internal())?;
    Ok(SkillDefinition {
        instructions,
        context: vec![SkillContext {
            label: "Recorded demonstration".to_owned(),
            content: recorded,
        }],
        tools: Vec::new(),
    })
}

pub(super) async fn update(
    State(state): State<AppState>,
    Path(skill_id): Path<Uuid>,
    Json(request): Json<UpdateSkillRequest>,
) -> Result<Json<SkillSummary>, ApiError> {
    let replayed = matches!(
        claim(
            &state,
            request.idempotency_key,
            &format!("update_skill:{skill_id}"),
            &request,
        )
        .await?,
        IdempotencyClaim::Replayed { .. }
    );
    if replayed {
        return Ok(Json(summary(
            &state
                .storage
                .skill_version(state.owner_id, request.idempotency_key)
                .await?,
        )));
    }
    homebot_skills::validate(&request.definition)
        .map_err(|error| ApiError::validation(&error.to_string()))?;
    let record = state
        .storage
        .update_skill(
            state.owner_id,
            skill_id,
            &visible(&request.name, 80, "Skill name")?,
            &optional_visible(&request.description, 500, "Skill description")?,
            &request.definition,
            request.idempotency_key,
            unix_time_ms(),
        )
        .await?;
    let skill = summary(&record);
    publish(&state, skill.clone()).await?;
    Ok(Json(skill))
}

pub(super) async fn duplicate(
    State(state): State<AppState>,
    Path(skill_id): Path<Uuid>,
    Json(request): Json<DuplicateSkillRequest>,
) -> Result<(StatusCode, Json<SkillSummary>), ApiError> {
    let replayed = matches!(
        claim(
            &state,
            request.idempotency_key,
            &format!("duplicate_skill:{skill_id}"),
            &request,
        )
        .await?,
        IdempotencyClaim::Replayed { .. }
    );
    if replayed {
        let existing = match state
            .storage
            .skill(state.owner_id, request.idempotency_key)
            .await
        {
            Ok(skill) => skill,
            Err(_) => {
                state
                    .storage
                    .skill_version(state.owner_id, request.idempotency_key)
                    .await?
            }
        };
        return Ok((StatusCode::OK, Json(summary(&existing))));
    }
    let source = state.storage.skill(state.owner_id, skill_id).await?;
    let now = unix_time_ms();
    let record = SkillRecord {
        id: request.idempotency_key,
        owner_id: state.owner_id,
        name: visible(&request.name, 80, "Skill name")?,
        description: source.description,
        active_version_id: Uuid::now_v7(),
        version: 1,
        definition: source.definition,
        bot_ids: Vec::new(),
        created_at_ms: now,
        updated_at_ms: now,
    };
    state.storage.create_skill(&record).await?;
    let skill = summary(&record);
    publish(&state, skill.clone()).await?;
    Ok((StatusCode::CREATED, Json(skill)))
}

pub(super) async fn delete(
    State(state): State<AppState>,
    Path(skill_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    state
        .storage
        .delete_skill(state.owner_id, skill_id, unix_time_ms())
        .await?;
    persist_event(
        &state,
        "skill_removed",
        ServerEventBody::SkillRemoved { skill_id },
    )
    .await
    .map_err(|()| ApiError::internal())?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn assign(
    State(state): State<AppState>,
    Path(skill_id): Path<Uuid>,
    Json(request): Json<SkillAssignmentRequest>,
) -> Result<Json<SkillSummary>, ApiError> {
    let _ = claim(
        &state,
        request.idempotency_key,
        &format!(
            "skill_assignment:{skill_id}:{}:{}",
            request.bot_id, request.enabled
        ),
        &request,
    )
    .await?;
    state
        .storage
        .set_skill_assignment(
            state.owner_id,
            skill_id,
            request.bot_id,
            request.enabled,
            unix_time_ms(),
        )
        .await?;
    let record = state.storage.skill(state.owner_id, skill_id).await?;
    let skill = summary(&record);
    publish(&state, skill.clone()).await?;
    Ok(Json(skill))
}

pub(super) async fn export(
    State(state): State<AppState>,
    Path(skill_id): Path<Uuid>,
) -> Result<Json<SkillBundle>, ApiError> {
    let record = state.storage.skill(state.owner_id, skill_id).await?;
    Ok(Json(SkillBundle {
        format_version: SKILL_BUNDLE_FORMAT_VERSION,
        name: record.name,
        description: record.description,
        definition: record.definition,
    }))
}

pub(super) async fn import(
    State(state): State<AppState>,
    Json(request): Json<ImportSkillRequest>,
) -> Result<(StatusCode, Json<SkillSummary>), ApiError> {
    let replayed = matches!(
        claim(&state, request.idempotency_key, "import_skill", &request).await?,
        IdempotencyClaim::Replayed { .. }
    );
    if replayed {
        let existing = match state
            .storage
            .skill(state.owner_id, request.idempotency_key)
            .await
        {
            Ok(skill) => skill,
            Err(_) => {
                state
                    .storage
                    .skill_version(state.owner_id, request.idempotency_key)
                    .await?
            }
        };
        return Ok((StatusCode::OK, Json(summary(&existing))));
    }
    if request.bundle.format_version != SKILL_BUNDLE_FORMAT_VERSION {
        return Err(ApiError::validation(
            "Unsupported Skill bundle format version",
        ));
    }
    homebot_skills::validate(&request.bundle.definition)
        .map_err(|error| ApiError::validation(&error.to_string()))?;
    let name = visible(&request.bundle.name, 80, "Skill name")?;
    let description = optional_visible(&request.bundle.description, 500, "Skill description")?;
    let conflict = state
        .storage
        .list_skills(state.owner_id)
        .await?
        .into_iter()
        .find(|skill| skill.name.trim().eq_ignore_ascii_case(name.trim()));
    if let Some(ref existing) = conflict {
        match request.conflict_policy {
            SkillImportConflictPolicy::Reject => {
                return Err(ApiError::conflict("A Skill with that name already exists"));
            }
            SkillImportConflictPolicy::CreateVersion => {
                let record = state
                    .storage
                    .update_skill(
                        state.owner_id,
                        existing.id,
                        &name,
                        &description,
                        &request.bundle.definition,
                        request.idempotency_key,
                        unix_time_ms(),
                    )
                    .await?;
                let skill = summary(&record);
                publish(&state, skill.clone()).await?;
                return Ok((StatusCode::OK, Json(skill)));
            }
            SkillImportConflictPolicy::Rename => {}
        }
    }
    let name = if conflict.is_some() {
        available_import_name(&state, &name).await?
    } else {
        name
    };
    let now = unix_time_ms();
    let record = SkillRecord {
        id: request.idempotency_key,
        owner_id: state.owner_id,
        name,
        description,
        active_version_id: Uuid::now_v7(),
        version: 1,
        definition: request.bundle.definition,
        bot_ids: Vec::new(),
        created_at_ms: now,
        updated_at_ms: now,
    };
    state.storage.create_skill(&record).await?;
    let skill = summary(&record);
    publish(&state, skill.clone()).await?;
    Ok((StatusCode::CREATED, Json(skill)))
}

async fn available_import_name(state: &AppState, source: &str) -> Result<String, ApiError> {
    let existing = state.storage.list_skills(state.owner_id).await?;
    for suffix in 1..=1_000_u16 {
        let marker = if suffix == 1 {
            " (imported)".to_owned()
        } else {
            format!(" (imported {suffix})")
        };
        let keep = 80_usize.saturating_sub(marker.len());
        let base = truncate_utf8(source.trim(), keep);
        let candidate = format!("{base}{marker}");
        if !existing
            .iter()
            .any(|skill| skill.name.trim().eq_ignore_ascii_case(&candidate))
        {
            return Ok(candidate);
        }
    }
    Err(ApiError::conflict("No available imported Skill name"))
}

fn truncate_utf8(value: &str, maximum: usize) -> &str {
    let mut end = value.len().min(maximum);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

pub(super) fn summary(record: &SkillRecord) -> SkillSummary {
    SkillSummary {
        id: record.id,
        name: record.name.clone(),
        description: record.description.clone(),
        active_version_id: record.active_version_id,
        version: record.version,
        definition: record.definition.clone(),
        bot_ids: record.bot_ids.clone(),
        created_at_unix_ms: u64::try_from(record.created_at_ms).unwrap_or_default(),
        updated_at_unix_ms: u64::try_from(record.updated_at_ms).unwrap_or_default(),
    }
}

async fn publish(state: &AppState, skill: SkillSummary) -> Result<(), ApiError> {
    persist_event(
        state,
        "skill_changed",
        ServerEventBody::SkillChanged { skill },
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
    let value = value.trim();
    if value.len() > max || value.chars().any(char::is_control) {
        return Err(ApiError::validation(&format!("{label} is invalid")));
    }
    Ok(value.to_owned())
}
