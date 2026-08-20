//! Durable headless routine schedules, deliveries, retries, cancellation and run history.

use std::{sync::Arc, time::Duration};

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use homebot_protocol::{
    BotMutationRequest, CreateRoutineTriggerRequest, DeliverRoutineTriggerRequest,
    RoutineJobSummary, RoutineTriggerSource, RoutineTriggerSummary, ServerEventBody,
};
use homebot_routines::{RoutineSchedule, due_occurrences, next_occurrence};
use homebot_storage::{OutboxEvent, RoutineJobClaim, RoutineJobRecord, RoutineTriggerRecord};
use serde_json::{Value, json};
use tokio::sync::Notify;
use uuid::Uuid;

use crate::{
    AppState,
    bots::{ApiError, claim},
    persist_event, routines, unix_time_ms,
};

const POLL_INTERVAL: Duration = Duration::from_millis(250);
const MISSED_GRACE_MS: u64 = 30_000;

pub(super) fn start(state: AppState) {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn(async move {
            let _ = state
                .storage
                .recover_interrupted_routine_jobs(state.owner_id, unix_time_ms())
                .await;
            let mut shutdown = state.server_shutdown.subscribe();
            let mut trigger_events = state.trigger_events.subscribe();
            let mut poll = tokio::time::interval(POLL_INTERVAL);
            loop {
                tokio::select! {
                    changed = shutdown.changed() => {
                        if changed.is_err() || *shutdown.borrow() { break; }
                    }
                    event = trigger_events.recv() => {
                        if let Ok((kind, event_id)) = event {
                            dispatch_event(&state, &kind, event_id).await;
                        }
                    }
                    _ = poll.tick() => {
                        evaluate_schedules(&state, unix_time_ms()).await;
                        evaluate_event_triggers(&state).await;
                        run_due_jobs(&state).await;
                    }
                }
            }
        });
    }
}

pub(super) async fn list_triggers(
    State(state): State<AppState>,
    Path(routine_id): Path<Uuid>,
) -> Result<Json<Vec<RoutineTriggerSummary>>, ApiError> {
    let triggers = state
        .storage
        .routine_triggers(state.owner_id, Some(routine_id))
        .await?;
    Ok(Json(triggers.iter().map(trigger_summary).collect()))
}

pub(super) async fn create_trigger(
    State(state): State<AppState>,
    Path(routine_id): Path<Uuid>,
    Json(request): Json<CreateRoutineTriggerRequest>,
) -> Result<(StatusCode, Json<RoutineTriggerSummary>), ApiError> {
    let replayed = matches!(
        claim(
            &state,
            request.idempotency_key,
            &format!("create_routine_trigger:{routine_id}"),
            &request,
        )
        .await?,
        homebot_storage::IdempotencyClaim::Replayed { .. }
    );
    if replayed {
        let trigger = state
            .storage
            .routine_trigger(state.owner_id, request.idempotency_key)
            .await?;
        return Ok((StatusCode::OK, Json(trigger_summary(&trigger))));
    }
    validate_definition(&request.definition)?;
    if let RoutineTriggerSource::Plugin { plugin_id, .. } = &request.definition.source {
        let _ = state.storage.plugin(state.owner_id, *plugin_id).await?;
    }
    let now = unix_time_ms();
    let next_fire_at_ms = schedule(&request.definition)
        .map(|schedule| next_occurrence(schedule, now.saturating_sub(1)))
        .transpose()
        .map_err(|error| ApiError::validation(&error.to_string()))?
        .flatten();
    let record = RoutineTriggerRecord {
        id: request.idempotency_key,
        owner_id: state.owner_id,
        routine_id,
        definition: request.definition,
        enabled: request.enabled,
        last_evaluated_at_ms: None,
        next_fire_at_ms,
        last_event_sequence: state.storage.latest_sequence(state.owner_id).await?,
        created_at_ms: now,
        updated_at_ms: now,
    };
    state.storage.create_routine_trigger(&record).await?;
    let summary = trigger_summary(&record);
    publish_trigger(&state, summary.clone()).await?;
    Ok((StatusCode::CREATED, Json(summary)))
}

pub(super) async fn delete_trigger(
    State(state): State<AppState>,
    Path(trigger_id): Path<Uuid>,
    Json(request): Json<BotMutationRequest>,
) -> Result<StatusCode, ApiError> {
    let replayed = matches!(
        claim(
            &state,
            request.idempotency_key,
            &format!("delete_routine_trigger:{trigger_id}"),
            &request,
        )
        .await?,
        homebot_storage::IdempotencyClaim::Replayed { .. }
    );
    if replayed {
        return Ok(StatusCode::NO_CONTENT);
    }
    state
        .storage
        .delete_routine_trigger(state.owner_id, trigger_id)
        .await?;
    persist_event(
        &state,
        "routine_trigger_removed",
        ServerEventBody::RoutineTriggerRemoved { trigger_id },
    )
    .await
    .map_err(|()| ApiError::internal())?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn deliver_trigger(
    State(state): State<AppState>,
    Path(trigger_id): Path<Uuid>,
    Json(request): Json<DeliverRoutineTriggerRequest>,
) -> Result<(StatusCode, Json<RoutineJobSummary>), ApiError> {
    let trigger = state
        .storage
        .routine_trigger(state.owner_id, trigger_id)
        .await?;
    if !trigger.enabled
        || !matches!(
            trigger.definition.source,
            RoutineTriggerSource::Webhook { .. }
        )
    {
        return Err(ApiError::validation(
            "Only enabled webhook triggers accept external deliveries",
        ));
    }
    if request.delivery_key.is_empty()
        || request.delivery_key.len() > 256
        || !request.inputs.is_object()
    {
        return Err(ApiError::validation("Invalid trigger delivery"));
    }
    let _ = claim(
        &state,
        request.idempotency_key,
        &format!("deliver_routine_trigger:{trigger_id}"),
        &request,
    )
    .await?;
    let (claim, job) = enqueue_delivery(
        &state,
        &trigger,
        &request.delivery_key,
        json!({"kind": "webhook", "trigger_id": trigger.id, "delivery_key": request.delivery_key}),
        request.inputs,
        unix_time_ms(),
    )
    .await?;
    Ok((
        if claim == RoutineJobClaim::Claimed {
            StatusCode::ACCEPTED
        } else {
            StatusCode::OK
        },
        Json(job_summary(&job)),
    ))
}

pub(super) async fn list_jobs(
    State(state): State<AppState>,
    Path(routine_id): Path<Uuid>,
) -> Result<Json<Vec<RoutineJobSummary>>, ApiError> {
    let jobs = state
        .storage
        .routine_jobs(state.owner_id, routine_id)
        .await?;
    Ok(Json(jobs.iter().map(job_summary).collect()))
}

pub(super) async fn cancel_job(
    State(state): State<AppState>,
    Path(job_id): Path<Uuid>,
    Json(request): Json<BotMutationRequest>,
) -> Result<StatusCode, ApiError> {
    let replayed = matches!(
        claim(
            &state,
            request.idempotency_key,
            &format!("cancel_routine_job:{job_id}"),
            &request,
        )
        .await?,
        homebot_storage::IdempotencyClaim::Replayed { .. }
    );
    if replayed {
        return Ok(StatusCode::ACCEPTED);
    }
    state
        .storage
        .cancel_routine_job(state.owner_id, job_id, unix_time_ms())
        .await?;
    if let Some(cancel) = state
        .routine_cancellations
        .lock()
        .await
        .get(&job_id)
        .cloned()
    {
        cancel.notify_one();
    }
    let changed = state.storage.routine_job(state.owner_id, job_id).await?;
    publish_job(&state, job_summary(&changed)).await?;
    Ok(StatusCode::ACCEPTED)
}

async fn evaluate_schedules(state: &AppState, now: i64) {
    let Ok(triggers) = state.storage.routine_triggers(state.owner_id, None).await else {
        return;
    };
    for trigger in triggers.into_iter().filter(|trigger| trigger.enabled) {
        let Some(schedule) = schedule(&trigger.definition) else {
            continue;
        };
        let cursor = trigger
            .last_evaluated_at_ms
            .unwrap_or_else(|| trigger.created_at_ms.saturating_sub(1));
        let Ok(due) = due_occurrences(
            schedule,
            cursor,
            now,
            MISSED_GRACE_MS,
            trigger.definition.missed_run_policy,
            trigger.definition.catch_up_limit,
        ) else {
            continue;
        };
        let mut enqueued_all = true;
        for scheduled_for in due {
            let key = format!("schedule:{scheduled_for}");
            if enqueue_delivery(
                state,
                &trigger,
                &key,
                json!({"kind":"schedule", "trigger_id":trigger.id, "scheduled_for_unix_ms":scheduled_for}),
                json!({}),
                scheduled_for,
            )
            .await
            .is_err()
            {
                enqueued_all = false;
                break;
            }
        }
        if !enqueued_all {
            continue;
        }
        let next = next_occurrence(schedule, now).ok().flatten();
        let _ = state
            .storage
            .advance_routine_trigger(state.owner_id, trigger.id, now, next, now)
            .await;
    }
}

async fn enqueue_delivery(
    state: &AppState,
    trigger: &RoutineTriggerRecord,
    delivery_key: &str,
    source: Value,
    inputs: Value,
    scheduled_for_ms: i64,
) -> Result<(RoutineJobClaim, RoutineJobRecord), ApiError> {
    let routine = state
        .storage
        .routine(state.owner_id, trigger.routine_id)
        .await?;
    if !routine.enabled || routine.draft {
        return Err(ApiError::validation(
            "Routine must be published and enabled",
        ));
    }
    let created = unix_time_ms();
    let record = RoutineJobRecord {
        id: Uuid::now_v7(),
        owner_id: state.owner_id,
        trigger_id: trigger.id,
        routine_id: trigger.routine_id,
        routine_version_id: routine.active_version_id,
        delivery_key: delivery_key.to_owned(),
        trigger: source,
        inputs,
        status: "queued".to_owned(),
        attempt_count: 0,
        scheduled_for_ms,
        next_attempt_at_ms: created.max(scheduled_for_ms),
        cancel_requested: false,
        error_message: None,
        created_at_ms: created,
        started_at_ms: None,
        finished_at_ms: None,
    };
    let claim = state.storage.enqueue_routine_job(&record).await?;
    let record = if claim == RoutineJobClaim::Claimed {
        publish_job(state, job_summary(&record)).await?;
        record
    } else {
        state
            .storage
            .routine_jobs(state.owner_id, trigger.routine_id)
            .await?
            .into_iter()
            .find(|job| job.trigger_id == trigger.id && job.delivery_key == delivery_key)
            .ok_or_else(ApiError::internal)?
    };
    Ok((claim, record))
}

async fn run_due_jobs(state: &AppState) {
    for _ in 0..32 {
        let Ok(Some(job)) = state
            .storage
            .claim_next_routine_job(state.owner_id, unix_time_ms())
            .await
        else {
            break;
        };
        let cancel = Arc::new(Notify::new());
        state
            .routine_cancellations
            .lock()
            .await
            .insert(job.id, Arc::clone(&cancel));
        let state = state.clone();
        tokio::spawn(async move {
            execute_claimed_job(state, job, cancel).await;
        });
    }
}

async fn execute_claimed_job(state: AppState, job: RoutineJobRecord, cancel: Arc<Notify>) {
    let outcome = tokio::select! {
        result = routines::execute_job(&state, &job) => Some(result),
        () = cancel.notified() => None,
    };
    state.routine_cancellations.lock().await.remove(&job.id);
    let now = unix_time_ms();
    match outcome {
        None => {
            let _ = state
                .storage
                .finish_routine_job(state.owner_id, job.id, "cancelled", None, now)
                .await;
        }
        Some(Ok(run)) if run.status == "succeeded" => {
            let _ = state
                .storage
                .finish_routine_job(state.owner_id, job.id, "succeeded", None, now)
                .await;
        }
        Some(Ok(run)) if run.status == "waiting_approval" => {
            let _ = state
                .storage
                .finish_routine_job(
                    state.owner_id,
                    job.id,
                    "failed",
                    Some("Routine requires approval"),
                    now,
                )
                .await;
        }
        Some(Ok(run)) => {
            let message = run
                .error_message
                .as_deref()
                .unwrap_or("Routine execution failed");
            let _ = state
                .storage
                .retry_or_fail_routine_job(state.owner_id, job.id, message, now)
                .await;
        }
        Some(Err(_)) => {
            let _ = state
                .storage
                .retry_or_fail_routine_job(state.owner_id, job.id, "Routine execution failed", now)
                .await;
        }
    }
    if let Ok(jobs) = state
        .storage
        .routine_jobs(state.owner_id, job.routine_id)
        .await
        && let Some(changed) = jobs.into_iter().find(|changed| changed.id == job.id)
    {
        let _ = publish_job(&state, job_summary(&changed)).await;
    }
}

pub(super) async fn dispatch_event(state: &AppState, kind: &str, event_id: Uuid) {
    if kind.starts_with("routine_") {
        return;
    }
    let Ok(triggers) = state.storage.routine_triggers(state.owner_id, None).await else {
        return;
    };
    for trigger in triggers.into_iter().filter(|trigger| trigger.enabled) {
        let RoutineTriggerSource::Event { event_kind } = &trigger.definition.source else {
            continue;
        };
        if event_kind != kind {
            continue;
        }
        let delivery = format!("event:{event_id}");
        let _ = enqueue_delivery(
            state,
            &trigger,
            &delivery,
            json!({"kind":"event", "trigger_id":trigger.id, "event_kind":kind, "event_id":event_id}),
            json!({}),
            unix_time_ms(),
        )
        .await;
    }
}

async fn evaluate_event_triggers(state: &AppState) {
    let Ok(triggers) = state.storage.routine_triggers(state.owner_id, None).await else {
        return;
    };
    for trigger in triggers.into_iter().filter(|trigger| trigger.enabled) {
        let (RoutineTriggerSource::Event { event_kind }
        | RoutineTriggerSource::Plugin { event_kind, .. }) = &trigger.definition.source
        else {
            continue;
        };
        let Ok(events) = state
            .storage
            .events_after(state.owner_id, trigger.last_event_sequence, 1_000)
            .await
        else {
            continue;
        };
        let mut cursor = trigger.last_event_sequence;
        for event in events {
            if !event.event_kind.starts_with("routine_")
                && &event.event_kind == event_kind
                && event_matches_trigger(&trigger, &event)
            {
                let kind = if matches!(
                    trigger.definition.source,
                    RoutineTriggerSource::Plugin { .. }
                ) {
                    "plugin"
                } else {
                    "event"
                };
                let delivery = format!("{kind}:{}", event.event_id);
                if enqueue_delivery(
                    state,
                    &trigger,
                    &delivery,
                    json!({"kind":kind, "trigger_id":trigger.id, "event_kind":event.event_kind, "event_id":event.event_id}),
                    json!({}),
                    event.created_at_ms,
                )
                .await
                .is_err()
                {
                    break;
                }
            }
            cursor = event.sequence;
        }
        if cursor > trigger.last_event_sequence {
            let _ = state
                .storage
                .advance_routine_trigger_event_cursor(
                    state.owner_id,
                    trigger.id,
                    cursor,
                    unix_time_ms(),
                )
                .await;
        }
    }
}

fn event_matches_trigger(trigger: &RoutineTriggerRecord, event: &OutboxEvent) -> bool {
    let RoutineTriggerSource::Plugin { plugin_id, .. } = trigger.definition.source else {
        return true;
    };
    event
        .payload
        .pointer("/plugin/id")
        .or_else(|| event.payload.get("plugin_id"))
        .and_then(Value::as_str)
        .is_some_and(|value| value == plugin_id.to_string())
}

fn validate_definition(
    definition: &homebot_routines::RoutineTriggerDefinition,
) -> Result<(), ApiError> {
    if definition.catch_up_limit > 100
        || definition.retry_policy.maximum_attempts == 0
        || definition.retry_policy.maximum_attempts > 20
        || definition.retry_policy.initial_backoff_seconds == 0
        || definition.retry_policy.maximum_backoff_seconds
            < definition.retry_policy.initial_backoff_seconds
    {
        return Err(ApiError::validation("Invalid trigger execution policy"));
    }
    if matches!(
        definition.overlap_policy,
        homebot_routines::OverlapPolicy::Parallel { maximum: 0 }
    ) {
        return Err(ApiError::validation("Parallel limit must be positive"));
    }
    match &definition.source {
        RoutineTriggerSource::Schedule { schedule } => {
            next_occurrence(schedule, unix_time_ms().saturating_sub(1))
                .map_err(|error| ApiError::validation(&error.to_string()))?;
        }
        RoutineTriggerSource::Webhook { slug } => validate_name(slug)?,
        RoutineTriggerSource::Event { event_kind }
        | RoutineTriggerSource::Plugin { event_kind, .. } => validate_name(event_kind)?,
    }
    Ok(())
}

fn validate_name(value: &str) -> Result<(), ApiError> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return Err(ApiError::validation("Invalid trigger identifier"));
    }
    Ok(())
}

fn schedule(definition: &homebot_routines::RoutineTriggerDefinition) -> Option<&RoutineSchedule> {
    match &definition.source {
        RoutineTriggerSource::Schedule { schedule } => Some(schedule),
        _ => None,
    }
}

fn trigger_summary(record: &RoutineTriggerRecord) -> RoutineTriggerSummary {
    RoutineTriggerSummary {
        id: record.id,
        routine_id: record.routine_id,
        definition: record.definition.clone(),
        enabled: record.enabled,
        last_evaluated_at_unix_ms: record.last_evaluated_at_ms.map(millis),
        next_fire_at_unix_ms: record.next_fire_at_ms.map(millis),
        created_at_unix_ms: millis(record.created_at_ms),
        updated_at_unix_ms: millis(record.updated_at_ms),
    }
}

fn job_summary(record: &RoutineJobRecord) -> RoutineJobSummary {
    RoutineJobSummary {
        id: record.id,
        trigger_id: record.trigger_id,
        routine_id: record.routine_id,
        routine_version_id: record.routine_version_id,
        delivery_key: record.delivery_key.clone(),
        trigger: record.trigger.clone(),
        input_metadata: input_metadata(&record.inputs),
        status: record.status.clone(),
        attempt_count: record.attempt_count,
        scheduled_for_unix_ms: millis(record.scheduled_for_ms),
        next_attempt_at_unix_ms: millis(record.next_attempt_at_ms),
        cancel_requested: record.cancel_requested,
        error_message: record.error_message.clone(),
        created_at_unix_ms: millis(record.created_at_ms),
        started_at_unix_ms: record.started_at_ms.map(millis),
        finished_at_unix_ms: record.finished_at_ms.map(millis),
    }
}

fn input_metadata(inputs: &Value) -> Value {
    let Some(object) = inputs.as_object() else {
        return json!({});
    };
    Value::Object(
        object
            .iter()
            .map(|(key, value)| {
                let kind = if value.is_string() {
                    "string"
                } else if value.is_number() {
                    "number"
                } else if value.is_boolean() {
                    "boolean"
                } else if value.is_null() {
                    "null"
                } else {
                    "structured"
                };
                (key.clone(), json!({"kind":kind}))
            })
            .collect(),
    )
}

async fn publish_trigger(state: &AppState, trigger: RoutineTriggerSummary) -> Result<(), ApiError> {
    persist_event(
        state,
        "routine_trigger_changed",
        ServerEventBody::RoutineTriggerChanged { trigger },
    )
    .await
    .map_err(|()| ApiError::internal())?;
    Ok(())
}

async fn publish_job(state: &AppState, job: RoutineJobSummary) -> Result<(), ApiError> {
    persist_event(
        state,
        "routine_job_changed",
        ServerEventBody::RoutineJobChanged { job },
    )
    .await
    .map_err(|()| ApiError::internal())?;
    Ok(())
}

fn millis(value: i64) -> u64 {
    u64::try_from(value).unwrap_or_default()
}
