//! Curated personal-assistant packs installed through existing server-owned primitives.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use homebot_protocol::{
    AssistantPackCadence, AssistantPackInstallationSummary, AssistantPackSchedule,
    AssistantPackSummary, InstallAssistantPackRequest, MissedRunPolicy, OverlapPolicy, RetryPolicy,
    RoutineDefinition, RoutineSchedule, RoutineStep, RoutineTriggerDefinition,
    RoutineTriggerSource, SkillDefinition,
};
use homebot_routines::{next_occurrence, validate as validate_routine};
use homebot_storage::{IdempotencyClaim, RoutineRecord, RoutineTriggerRecord, SkillRecord};
use uuid::Uuid;

use crate::{
    AppState,
    bots::{ApiError, claim},
    routines, scheduler, skills, unix_time_ms,
};

struct AssistantPack {
    id: &'static str,
    name: &'static str,
    description: &'static str,
    skill_name: &'static str,
    routine_name: &'static str,
    instructions: &'static str,
    prompt: &'static str,
    schedule: AssistantPackSchedule,
}

const PACKS: [AssistantPack; 3] = [
    AssistantPack {
        id: "morning-brief",
        name: "Morning Brief",
        description: "Start the day with priorities, commitments, and anything needing attention.",
        skill_name: "Morning brief",
        routine_name: "Morning brief",
        instructions: "Prepare concise personal morning briefs. Use only available HomeBot context and connected tools. Separate confirmed facts from missing sources, prioritize what needs attention today, and finish with practical next actions.",
        prompt: "Prepare today's morning brief. Include priorities, upcoming commitments, blockers, and useful next actions. Clearly say when a source such as a calendar or inbox is unavailable.",
        schedule: AssistantPackSchedule {
            cadence: AssistantPackCadence::Daily,
            weekday: None,
            default_hour: 8,
            default_minute: 0,
        },
    },
    AssistantPack {
        id: "weekly-rundown",
        name: "Weekly Rundown",
        description: "Wrap up the week with progress, loose ends, and next-week priorities.",
        skill_name: "Weekly rundown",
        routine_name: "Weekly rundown",
        instructions: "Prepare concise weekly reviews from available HomeBot context and connected tools. Distinguish completed work, unresolved items, and inferred suggestions. Never invent activity that is not present in the available sources.",
        prompt: "Prepare this week's rundown. Summarize progress, decisions, unfinished work, and the most useful priorities for next week. Clearly note unavailable sources.",
        schedule: AssistantPackSchedule {
            cadence: AssistantPackCadence::Weekly,
            weekday: Some(5),
            default_hour: 17,
            default_minute: 0,
        },
    },
    AssistantPack {
        id: "end-of-day-review",
        name: "End-of-Day Review",
        description: "Close the day with completed work, open loops, and tomorrow's first move.",
        skill_name: "End-of-day review",
        routine_name: "End-of-day review",
        instructions: "Prepare short end-of-day reviews from available HomeBot context. Capture completed work, unresolved items, and tomorrow's first useful action. Do not claim access to unavailable services or invent events.",
        prompt: "Prepare today's end-of-day review. Capture what was completed, open loops, and the best first action for tomorrow. Clearly note unavailable sources.",
        schedule: AssistantPackSchedule {
            cadence: AssistantPackCadence::Daily,
            weekday: None,
            default_hour: 18,
            default_minute: 0,
        },
    },
];

pub(super) async fn list() -> Json<Vec<AssistantPackSummary>> {
    Json(PACKS.iter().map(summary).collect())
}

pub(super) async fn install(
    State(state): State<AppState>,
    Path(pack_id): Path<String>,
    Json(request): Json<InstallAssistantPackRequest>,
) -> Result<(StatusCode, Json<AssistantPackInstallationSummary>), ApiError> {
    let pack = PACKS
        .iter()
        .find(|pack| pack.id == pack_id)
        .ok_or_else(|| ApiError::validation("Assistant Pack was not found"))?;
    let bot = state
        .storage
        .get_bot(state.owner_id, request.bot_id)
        .await?;
    let install_id = Uuid::new_v5(
        &state.owner_id,
        format!("{}:{}", pack.id, request.bot_id).as_bytes(),
    );
    let skill_id = Uuid::new_v5(&install_id, b"skill");
    let routine_id = Uuid::new_v5(&install_id, b"routine");
    let trigger_id = Uuid::new_v5(&install_id, b"trigger");
    if let (Ok(skill), Ok(routine), Ok(trigger)) = (
        state.storage.skill(state.owner_id, skill_id).await,
        state.storage.routine(state.owner_id, routine_id).await,
        state
            .storage
            .routine_trigger(state.owner_id, trigger_id)
            .await,
    ) {
        let expected = schedule(pack, &request)?;
        let expected_source = RoutineTriggerSource::Schedule { schedule: expected };
        if trigger.definition.source != expected_source {
            return Err(ApiError::conflict(
                "This Assistant Pack is already installed for the Bot with a different schedule",
            ));
        }
        return Ok((
            StatusCode::OK,
            Json(installation(pack, &skill, &routine, &trigger)),
        ));
    }
    let (skill, routine, trigger) = new_records(
        &state,
        pack,
        &request,
        &bot.name,
        (skill_id, routine_id, trigger_id),
    )
    .await?;
    let replayed = matches!(
        claim(
            &state,
            request.idempotency_key,
            &format!("install_assistant_pack:{}:{}", pack.id, request.bot_id),
            &request,
        )
        .await?,
        IdempotencyClaim::Replayed { .. }
    );
    if replayed {
        return Err(ApiError::conflict(
            "The original Assistant Pack installation is not reflected in current state",
        ));
    }

    if let Err(error) = state
        .storage
        .install_assistant_pack(&skill, &routine, &trigger)
        .await
    {
        state
            .storage
            .release_idempotency(request.idempotency_key)
            .await?;
        return Err(error.into());
    }
    let installed = installation(pack, &skill, &routine, &trigger);
    skills::publish(&state, installed.skill.clone()).await?;
    routines::publish_routine(&state, installed.routine.clone()).await?;
    scheduler::publish_trigger(&state, installed.trigger.clone()).await?;
    Ok((StatusCode::CREATED, Json(installed)))
}

async fn new_records(
    state: &AppState,
    pack: &AssistantPack,
    request: &InstallAssistantPackRequest,
    bot_name: &str,
    (skill_id, routine_id, trigger_id): (Uuid, Uuid, Uuid),
) -> Result<(SkillRecord, RoutineRecord, RoutineTriggerRecord), ApiError> {
    let now = unix_time_ms();
    let schedule = schedule(pack, request)?;
    let skill = SkillRecord {
        id: skill_id,
        owner_id: state.owner_id,
        name: format!("{} · {bot_name}", pack.skill_name),
        description: pack.description.to_owned(),
        active_version_id: Uuid::now_v7(),
        version: 1,
        definition: SkillDefinition {
            instructions: pack.instructions.to_owned(),
            context: Vec::new(),
            tools: Vec::new(),
        },
        bot_ids: vec![request.bot_id],
        created_at_ms: now,
        updated_at_ms: now,
    };
    let routine = RoutineRecord {
        id: routine_id,
        owner_id: state.owner_id,
        bot_id: request.bot_id,
        name: format!("{} · {bot_name}", pack.routine_name),
        description: pack.description.to_owned(),
        enabled: true,
        draft: false,
        active_version_id: Uuid::now_v7(),
        version: 1,
        definition: RoutineDefinition {
            inputs: Vec::new(),
            steps: vec![RoutineStep::BotPrompt {
                bot_id: request.bot_id,
                prompt_template: pack.prompt.to_owned(),
                requires_approval: false,
            }],
            expected_outputs: Vec::new(),
        },
        created_at_ms: now,
        updated_at_ms: now,
    };
    homebot_skills::validate(&skill.definition)
        .map_err(|error| ApiError::validation(&error.to_string()))?;
    validate_routine(&routine.definition)
        .map_err(|error| ApiError::validation(&error.to_string()))?;
    let trigger = RoutineTriggerRecord {
        id: trigger_id,
        owner_id: state.owner_id,
        routine_id,
        definition: RoutineTriggerDefinition {
            source: RoutineTriggerSource::Schedule {
                schedule: schedule.clone(),
            },
            missed_run_policy: MissedRunPolicy::RunOnce,
            overlap_policy: OverlapPolicy::Queue,
            retry_policy: RetryPolicy::default(),
            catch_up_limit: 1,
        },
        enabled: true,
        last_evaluated_at_ms: None,
        next_fire_at_ms: next_occurrence(&schedule, now.saturating_sub(1))
            .map_err(|error| ApiError::validation(&error.to_string()))?,
        last_event_sequence: state.storage.latest_sequence(state.owner_id).await?,
        created_at_ms: now,
        updated_at_ms: now,
    };
    Ok((skill, routine, trigger))
}

fn summary(pack: &AssistantPack) -> AssistantPackSummary {
    AssistantPackSummary {
        id: pack.id.to_owned(),
        name: pack.name.to_owned(),
        description: pack.description.to_owned(),
        skill_name: pack.skill_name.to_owned(),
        routine_name: pack.routine_name.to_owned(),
        schedule: pack.schedule.clone(),
    }
}

fn schedule(
    pack: &AssistantPack,
    request: &InstallAssistantPackRequest,
) -> Result<RoutineSchedule, ApiError> {
    let schedule = match pack.schedule.cadence {
        AssistantPackCadence::Daily => RoutineSchedule::DailyLocal {
            timezone: request.timezone.clone(),
            hour: request.hour,
            minute: request.minute,
        },
        AssistantPackCadence::Weekly => RoutineSchedule::WeeklyLocal {
            timezone: request.timezone.clone(),
            weekday: pack.schedule.weekday.unwrap_or(5),
            hour: request.hour,
            minute: request.minute,
        },
    };
    next_occurrence(&schedule, unix_time_ms().saturating_sub(1))
        .map_err(|error| ApiError::validation(&error.to_string()))?;
    Ok(schedule)
}

fn installation(
    pack: &AssistantPack,
    skill: &SkillRecord,
    routine: &RoutineRecord,
    trigger: &RoutineTriggerRecord,
) -> AssistantPackInstallationSummary {
    AssistantPackInstallationSummary {
        pack_id: pack.id.to_owned(),
        skill: skills::summary(skill),
        routine: routines::summary(routine),
        trigger: scheduler::trigger_summary(trigger),
    }
}
