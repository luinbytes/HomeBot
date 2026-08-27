//! Timezone-safe, deterministic routine schedule and trigger contracts.

use chrono::{Datelike, LocalResult, TimeZone, Utc};
use chrono_tz::Tz;
use croner::parser::{CronParser, Seconds};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const MAX_OCCURRENCE_SCAN: usize = 10_000;
const MAX_DAILY_SCAN_DAYS: i64 = 3_670;

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RoutineSchedule {
    OneShot {
        at_unix_ms: i64,
    },
    Interval {
        anchor_unix_ms: i64,
        every_seconds: u32,
    },
    DailyLocal {
        timezone: String,
        hour: u8,
        minute: u8,
    },
    WeeklyLocal {
        timezone: String,
        weekday: u8,
        hour: u8,
        minute: u8,
    },
    Cron {
        expression: String,
        timezone: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RoutineTriggerSource {
    Schedule { schedule: RoutineSchedule },
    Webhook { slug: String },
    Event { event_kind: String },
    Plugin { plugin_id: Uuid, event_kind: String },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MissedRunPolicy {
    Skip,
    RunOnce,
    CatchUp,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum OverlapPolicy {
    Skip,
    Queue,
    Parallel { maximum: u16 },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RetryPolicy {
    pub maximum_attempts: u16,
    pub initial_backoff_seconds: u32,
    pub maximum_backoff_seconds: u32,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            maximum_attempts: 1,
            initial_backoff_seconds: 5,
            maximum_backoff_seconds: 300,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoutineTriggerDefinition {
    pub source: RoutineTriggerSource,
    pub missed_run_policy: MissedRunPolicy,
    pub overlap_policy: OverlapPolicy,
    pub retry_policy: RetryPolicy,
    pub catch_up_limit: u16,
}

#[derive(Debug, thiserror::Error)]
pub enum ScheduleError {
    #[error("schedule is invalid")]
    Invalid,
    #[error("schedule timezone is unknown")]
    UnknownTimezone,
    #[error("schedule occurrence scan exceeded its safety bound")]
    ScanLimit,
}

/// Returns the first schedule occurrence strictly after `after_unix_ms`.
///
/// Daily wall-clock schedules use the IANA timezone database. A nonexistent spring-forward wall
/// time is skipped. For an ambiguous fall-back wall time, the earlier UTC instant is selected.
///
/// # Errors
///
/// Returns a stable error for invalid intervals/times, unknown zones, or conversion overflow.
pub fn next_occurrence(
    schedule: &RoutineSchedule,
    after_unix_ms: i64,
) -> Result<Option<i64>, ScheduleError> {
    match schedule {
        RoutineSchedule::OneShot { at_unix_ms } => {
            Ok((*at_unix_ms > after_unix_ms).then_some(*at_unix_ms))
        }
        RoutineSchedule::Interval {
            anchor_unix_ms,
            every_seconds,
        } => {
            if *every_seconds == 0 {
                return Err(ScheduleError::Invalid);
            }
            let period = i64::from(*every_seconds)
                .checked_mul(1_000)
                .ok_or(ScheduleError::Invalid)?;
            if after_unix_ms < *anchor_unix_ms {
                return Ok(Some(*anchor_unix_ms));
            }
            let elapsed = after_unix_ms
                .checked_sub(*anchor_unix_ms)
                .ok_or(ScheduleError::Invalid)?;
            let steps = elapsed
                .checked_div(period)
                .and_then(|steps| steps.checked_add(1))
                .ok_or(ScheduleError::Invalid)?;
            Ok(
                anchor_unix_ms
                    .checked_add(steps.checked_mul(period).ok_or(ScheduleError::Invalid)?),
            )
        }
        RoutineSchedule::DailyLocal {
            timezone,
            hour,
            minute,
        } => next_local(timezone, None, *hour, *minute, after_unix_ms).map(Some),
        RoutineSchedule::WeeklyLocal {
            timezone,
            weekday,
            hour,
            minute,
        } => next_local(timezone, Some(*weekday), *hour, *minute, after_unix_ms).map(Some),
        RoutineSchedule::Cron {
            expression,
            timezone,
        } => next_cron(expression, timezone, after_unix_ms).map(Some),
    }
}

/// Returns due instants after a durable cursor, applying missed-run policy outside a grace window.
///
/// `CatchUp` is bounded by both `catch_up_limit` and an internal scan cap. Recent occurrences are
/// always returned so normal polling does not accidentally treat small scheduler latency as a miss.
///
/// # Errors
///
/// Returns schedule validation, timezone, overflow, or scan-limit errors.
pub fn due_occurrences(
    schedule: &RoutineSchedule,
    after_unix_ms: i64,
    now_unix_ms: i64,
    grace_ms: u64,
    missed_run_policy: MissedRunPolicy,
    catch_up_limit: u16,
) -> Result<Vec<i64>, ScheduleError> {
    if now_unix_ms <= after_unix_ms {
        return Ok(Vec::new());
    }
    let grace = i64::try_from(grace_ms).unwrap_or(i64::MAX);
    let recent_boundary = now_unix_ms.saturating_sub(grace);
    let mut cursor = after_unix_ms;
    let mut missed = Vec::new();
    let mut recent = Vec::new();
    for _ in 0..MAX_OCCURRENCE_SCAN {
        let Some(next) = next_occurrence(schedule, cursor)? else {
            return Ok(apply_missed(
                missed,
                recent,
                missed_run_policy,
                catch_up_limit,
            ));
        };
        if next > now_unix_ms {
            return Ok(apply_missed(
                missed,
                recent,
                missed_run_policy,
                catch_up_limit,
            ));
        }
        if next <= recent_boundary {
            missed.push(next);
        } else {
            recent.push(next);
        }
        cursor = next;
    }
    Err(ScheduleError::ScanLimit)
}

fn apply_missed(
    missed: Vec<i64>,
    mut recent: Vec<i64>,
    policy: MissedRunPolicy,
    catch_up_limit: u16,
) -> Vec<i64> {
    let mut selected = match policy {
        MissedRunPolicy::Skip => Vec::new(),
        MissedRunPolicy::RunOnce => missed.last().copied().into_iter().collect(),
        MissedRunPolicy::CatchUp => missed
            .into_iter()
            .rev()
            .take(usize::from(catch_up_limit.max(1)))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect(),
    };
    selected.append(&mut recent);
    selected
}

fn next_local(
    timezone: &str,
    weekday: Option<u8>,
    hour: u8,
    minute: u8,
    after_unix_ms: i64,
) -> Result<i64, ScheduleError> {
    if hour > 23 || minute > 59 || weekday.is_some_and(|day| !(1..=7).contains(&day)) {
        return Err(ScheduleError::Invalid);
    }
    let timezone: Tz = timezone
        .parse()
        .map_err(|_| ScheduleError::UnknownTimezone)?;
    let after = chrono::DateTime::<Utc>::from_timestamp_millis(after_unix_ms)
        .ok_or(ScheduleError::Invalid)?;
    let local = after.with_timezone(&timezone);
    let start_date = local.date_naive();
    for day_offset in 0..=MAX_DAILY_SCAN_DAYS {
        let date = start_date
            .checked_add_days(chrono::Days::new(
                u64::try_from(day_offset).map_err(|_| ScheduleError::Invalid)?,
            ))
            .ok_or(ScheduleError::Invalid)?;
        if weekday.is_some_and(|day| date.weekday().number_from_monday() != u32::from(day)) {
            continue;
        }
        let candidate = timezone.with_ymd_and_hms(
            date.year(),
            date.month(),
            date.day(),
            u32::from(hour),
            u32::from(minute),
            0,
        );
        let candidate = match candidate {
            LocalResult::Single(candidate) => Some(candidate),
            LocalResult::Ambiguous(first, second) => Some(first.min(second)),
            LocalResult::None => None,
        };
        if let Some(candidate) = candidate {
            let timestamp = candidate.with_timezone(&Utc).timestamp_millis();
            if timestamp > after_unix_ms {
                return Ok(timestamp);
            }
        }
    }
    Err(ScheduleError::ScanLimit)
}

fn next_cron(expression: &str, timezone: &str, after_unix_ms: i64) -> Result<i64, ScheduleError> {
    if expression.is_empty() || expression.len() > 256 || expression.trim() != expression {
        return Err(ScheduleError::Invalid);
    }
    let timezone: Tz = timezone
        .parse()
        .map_err(|_| ScheduleError::UnknownTimezone)?;
    let after = chrono::DateTime::<Utc>::from_timestamp_millis(after_unix_ms)
        .ok_or(ScheduleError::Invalid)?
        .with_timezone(&timezone);
    let cron = CronParser::builder()
        .seconds(Seconds::Disallowed)
        .build()
        .parse(expression)
        .map_err(|_| ScheduleError::Invalid)?;
    cron.find_next_occurrence(&after, false)
        .map(|next| next.with_timezone(&Utc).timestamp_millis())
        .map_err(|_| ScheduleError::ScanLimit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(value: &str) -> Result<i64, Box<dyn std::error::Error>> {
        Ok(chrono::DateTime::parse_from_rfc3339(value)?.timestamp_millis())
    }

    #[test]
    fn daily_schedule_handles_dst_gaps_and_ambiguity_deterministically()
    -> Result<(), Box<dyn std::error::Error>> {
        let schedule = RoutineSchedule::DailyLocal {
            timezone: "Europe/London".to_owned(),
            hour: 1,
            minute: 30,
        };
        assert_eq!(
            next_occurrence(&schedule, utc("2026-03-29T00:00:00Z")?)?,
            Some(utc("2026-03-30T00:30:00Z")?)
        );
        assert_eq!(
            next_occurrence(&schedule, utc("2026-10-25T00:00:00Z")?)?,
            Some(utc("2026-10-25T00:30:00Z")?)
        );
        Ok(())
    }

    #[test]
    fn weekly_schedule_keeps_its_local_weekday_across_dst() -> Result<(), Box<dyn std::error::Error>>
    {
        let schedule = RoutineSchedule::WeeklyLocal {
            timezone: "Europe/London".to_owned(),
            weekday: 1,
            hour: 8,
            minute: 0,
        };
        assert_eq!(
            next_occurrence(&schedule, utc("2026-03-23T08:00:00Z")?)?,
            Some(utc("2026-03-30T07:00:00Z")?)
        );
        assert_eq!(
            next_occurrence(&schedule, utc("2026-10-19T07:00:00Z")?)?,
            Some(utc("2026-10-26T08:00:00Z")?)
        );
        Ok(())
    }

    #[test]
    fn missed_occurrences_are_bounded_and_recent_poll_latency_always_runs()
    -> Result<(), ScheduleError> {
        let schedule = RoutineSchedule::Interval {
            anchor_unix_ms: 0,
            every_seconds: 60,
        };
        assert_eq!(
            due_occurrences(&schedule, 0, 300_000, 30_000, MissedRunPolicy::Skip, 3)?,
            vec![300_000]
        );
        assert_eq!(
            due_occurrences(&schedule, 0, 300_000, 0, MissedRunPolicy::RunOnce, 3)?,
            vec![300_000]
        );
        assert_eq!(
            due_occurrences(&schedule, 0, 300_000, 0, MissedRunPolicy::CatchUp, 2)?,
            vec![240_000, 300_000]
        );
        Ok(())
    }

    #[test]
    fn one_shot_and_interval_are_strictly_after_cursor() -> Result<(), ScheduleError> {
        assert_eq!(
            next_occurrence(&RoutineSchedule::OneShot { at_unix_ms: 50 }, 49)?,
            Some(50)
        );
        assert_eq!(
            next_occurrence(&RoutineSchedule::OneShot { at_unix_ms: 50 }, 50)?,
            None
        );
        assert_eq!(
            next_occurrence(
                &RoutineSchedule::Interval {
                    anchor_unix_ms: 10,
                    every_seconds: 2,
                },
                2_010,
            )?,
            Some(4_010)
        );
        Ok(())
    }

    #[test]
    fn cron_supports_five_fields_aliases_and_iana_timezones()
    -> Result<(), Box<dyn std::error::Error>> {
        let weekday = RoutineSchedule::Cron {
            expression: "0 8 * * MON-FRI".to_owned(),
            timezone: "Europe/London".to_owned(),
        };
        assert_eq!(
            next_occurrence(&weekday, utc("2026-03-27T08:00:00Z")?)?,
            Some(utc("2026-03-30T07:00:00Z")?)
        );

        let hourly = RoutineSchedule::Cron {
            expression: "@hourly".to_owned(),
            timezone: "UTC".to_owned(),
        };
        assert_eq!(
            next_occurrence(&hourly, utc("2026-01-01T10:12:34Z")?)?,
            Some(utc("2026-01-01T11:00:00Z")?)
        );

        let daily = RoutineSchedule::Cron {
            expression: "@daily".to_owned(),
            timezone: "America/New_York".to_owned(),
        };
        assert_eq!(
            next_occurrence(&daily, utc("2026-07-01T04:00:00Z")?)?,
            Some(utc("2026-07-02T04:00:00Z")?)
        );
        Ok(())
    }

    #[test]
    fn cron_rejects_seconds_unknown_timezones_and_every_alias() {
        for schedule in [
            RoutineSchedule::Cron {
                expression: "*/5 * * * * *".to_owned(),
                timezone: "UTC".to_owned(),
            },
            RoutineSchedule::Cron {
                expression: "@daily".to_owned(),
                timezone: "Mars/Olympus".to_owned(),
            },
            RoutineSchedule::Cron {
                expression: "@every 5m".to_owned(),
                timezone: "UTC".to_owned(),
            },
        ] {
            assert!(next_occurrence(&schedule, 0).is_err());
        }
    }
}
