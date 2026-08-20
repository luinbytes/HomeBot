# Routines

A versioned routine belongs to one Bot and combines typed inputs, structured steps, expected outputs and explicit approval requirements. Demonstration recording produces an editable disabled draft; it never grants authority that was not explicit in the resulting steps.

## Recording and editing

`POST /api/v1/routine-recordings` starts a durable recording. Each appended action is already structured as a Bot prompt, plugin tool call or output record. HomeBot never records mouse coordinates, secret values or raw credentials. Stop and review converts the immutable action sequence into routine version 1 as a disabled draft. Cancel closes the recording without creating a routine.

Routine create/update/list/delete, rename, duplicate and enable/disable operations are authenticated and owner-scoped. Every edit creates a new immutable `routine_versions` row and atomically advances `active_version_id`; prior chat or run history therefore retains the exact definition it used. Duplicates start at version 1 as disabled drafts.

## Deterministic replay

`homebot-routines` validates definitions and replays steps sequentially through a server-owned `RoutineActionExecutor`. Dry run validates every structured step and produces `planned` results without invoking the executor. Run now binds its durable record to the exact active version. Bot-prompt steps append a real message to the Bot's authoritative direct chat and start its configured provider when available. Plugin steps invoke the named MCP tool through the constrained server adapter after checking that the plugin is connected, enabled, discovered and assigned to the routine's Bot; untrusted MCP content is not copied into durable routine results. A step marked `requires_approval` stops at `approval_required` before dispatch; text or plugin output cannot satisfy it.

The API exposes Run now, Dry run and recent run history. Duplicate mutation retries return the prior run instead of dispatching steps twice. Successful, approval-waiting and failed attempts are all durable, use safe error summaries and survive restart. Invalid recording conversion leaves the recording open so the user can correct it. Durable history stores only input kind/presence metadata, never input values. Draft or disabled routines cannot run normally. The desktop includes routine list, editor and recording states plus a read-only projection updated by routine server events; 6C7-73 owns wiring that projection into the production authenticated desktop transport. Android uses the generated protocol models.

## Scheduling boundary

Issue 6C7-53 adds the headless scheduler: one-shot/recurring/event triggers, time zones, missed-run policy, retries/backoff, cancellation, duplicate-event idempotency, concurrency limits and durable redacted run history. Routine execution does not depend on a connected client.
