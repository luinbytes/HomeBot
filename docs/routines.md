# Routines

A versioned routine belongs to one Bot and combines typed inputs, structured steps, expected outputs and explicit approval requirements. Demonstration recording produces an editable disabled draft; it never grants authority that was not explicit in the resulting steps.

## Recording and editing

`POST /api/v1/routine-recordings` starts a durable recording. Each appended action is already structured as a Bot prompt, plugin tool call or output record. HomeBot never records mouse coordinates, secret values or raw credentials. Stop and review converts the immutable action sequence into routine version 1 as a disabled draft. Cancel closes the recording without creating a routine.

Routine create/update/list/delete, rename, duplicate and enable/disable operations are authenticated and owner-scoped. Every edit creates a new immutable `routine_versions` row and atomically advances `active_version_id`; prior chat or run history therefore retains the exact definition it used. Duplicates start at version 1 as disabled drafts.

## Deterministic replay

`homebot-routines` validates definitions and replays steps sequentially through a server-owned `RoutineActionExecutor`. Dry run validates every structured step and produces `planned` results without invoking the executor. Run now binds its durable record to the exact active version. Bot-prompt steps append a real message to the Bot's authoritative direct chat and start its configured provider when available. A step marked `requires_approval` stops at `approval_required`; text or plugin output cannot satisfy it. Plugin steps must reference an enabled, discovered tool assigned to the routine's Bot. Results remain redacted structured metadata.

The API exposes Run now, Dry run and recent run history. Duplicate mutation retries return the prior run instead of dispatching steps twice. Durable history stores only input kind/presence metadata, never input values. Draft or disabled routines cannot run normally. The desktop includes routine list, editor and recording states; Android uses the generated protocol models.

## Scheduling boundary

Issue 6C7-53 adds the headless scheduler: one-shot/recurring/event triggers, time zones, missed-run policy, retries/backoff, cancellation, duplicate-event idempotency, concurrency limits and durable redacted run history. Routine execution does not depend on a connected client.
