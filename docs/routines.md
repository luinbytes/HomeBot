# Routines

A versioned routine belongs to one Bot and combines structured steps with a trigger, inputs, expected outputs, approval policy, failure policy, concurrency rule, and reporting destination. Demonstration recording produces an editable draft; it never grants authority that was not explicit in the resulting steps.

The headless scheduler owns one-shot, recurring, and event triggers, time zones, missed-run policy, retries/backoff, cancellation, duplicate-event idempotency, and durable redacted run history. Routine execution must not require a connected client.
