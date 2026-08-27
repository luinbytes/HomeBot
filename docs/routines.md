# Routines

A versioned routine belongs to one Bot and combines typed inputs, structured steps, expected outputs and explicit approval requirements. Demonstration recording produces an editable disabled draft; it never grants authority that was not explicit in the resulting steps.

## Recording and editing

`POST /api/v1/routine-recordings` starts a durable recording. Each appended action is already structured as a Bot prompt, plugin tool call or output record. HomeBot never records mouse coordinates, secret values or raw credentials. Stop and review converts the immutable action sequence into routine version 1 as a disabled draft. Cancel closes the recording without creating a routine.

Routine create/update/list/delete, rename, duplicate and enable/disable operations are authenticated and owner-scoped. Every edit creates a new immutable `routine_versions` row and atomically advances `active_version_id`; prior chat or run history therefore retains the exact definition it used. Duplicates start at version 1 as disabled drafts.

## Deterministic replay

`homebot-routines` validates definitions and replays steps sequentially through a server-owned `RoutineActionExecutor`. Dry run validates every structured step and produces `planned` results without invoking the executor. Run now binds its durable record to the exact active version. Bot-prompt steps append a real message to the Bot's authoritative direct chat and start its configured provider when available. Plugin steps invoke the named MCP tool through the constrained server adapter after checking that the plugin is connected, enabled, discovered and assigned to the routine's Bot; untrusted MCP content is not copied into durable routine results. A step marked `requires_approval` stops at `approval_required` before dispatch; text or plugin output cannot satisfy it.

The API exposes Run now, Dry run and recent run history. Duplicate mutation retries return the prior run instead of dispatching steps twice. Successful, approval-waiting and failed attempts are all durable, use safe error summaries and survive restart. Invalid recording conversion leaves the recording open so the user can correct it. Durable history stores only input kind/presence metadata, never input values. Draft or disabled routines cannot run normally. The desktop includes routine list, editor and recording states plus a read-only projection updated by routine server events; 6C7-73 owns wiring that projection into the production authenticated desktop transport. Android uses the generated protocol models.

## Headless schedules and triggers

The server evaluates enabled triggers without a connected client. A trigger can be a one-shot instant, an anchored interval (`@every` semantics), a daily or weekly wall-clock time, a five-field cron expression (`@hourly` and `@daily` aliases included), an authenticated webhook delivery, a durable HomeBot event, or a plugin-scoped event. Wall-clock and cron schedules require an IANA timezone. They skip nonexistent spring-forward times and choose the earlier UTC instant when a fall-back time is ambiguous. Each schedule stores its evaluation cursor, and each event trigger stores its monotonic outbox cursor, so restart recovery does not depend on an in-memory timer or broadcast. Event notifications use a 750 ms quiet window, coalesce at most 25 matching events into one durable job, and inspect at most 500 queued events per trigger pass. A larger backlog remains in the outbox and the job metadata reports `backlog_limited` rather than silently dropping it.

Missed occurrences use an explicit `skip`, `run_once`, or bounded `catch_up` policy. Every accepted occurrence becomes a durable job bound to the routine version that was active at enqueue time. External `delivery_key` values and schedule/event identifiers are unique per trigger, making retries safe. The scheduler claims jobs transactionally, applies `skip`, `queue`, or bounded `parallel` overlap policy, and executes independent allowed jobs concurrently. Failures use bounded exponential backoff; queued, retrying, or running jobs can be cancelled through the authenticated API.

Composio account triggers enter through the connector's signed public webhook route rather than the authenticated generic delivery route. HomeBot reduces each accepted delivery to a safe plugin ID, trigger slug, and stable event ID before the outbox. The scheduler then applies the same enabled-routine, exact-version, overlap, retry, cancellation, and per-trigger deduplication rules as every other plugin event. Disabled triggers remain inert, and provider payload fields never enter job inputs or run history.

Routes:

- `GET|POST /api/v1/routines/{routine_id}/triggers`
- `DELETE /api/v1/routine-triggers/{trigger_id}`
- `POST /api/v1/routine-triggers/{trigger_id}/deliver`
- `GET /api/v1/routines/{routine_id}/jobs`
- `POST /api/v1/routine-jobs/{job_id}/cancel`

Run history records trigger metadata, exact routine version, Bot, scheduled/start/finish times, attempt and outcome. Input values are reduced to declared kind and presence metadata before the run row or protocol event is written. Plugin response bodies are never copied into durable routine activity. Approval-marked steps stop before dispatch; unattended jobs fail safely instead of bypassing the approval boundary.
