# Skills

Skills are reusable, provider-neutral instruction bundles that can be assigned to one or more Bots or selected for an individual message. A Skill contains bounded instructions, labelled context, and portable plugin/tool references. Tool references communicate intent only: they never grant a Bot a capability or bypass server approval policy.

## Version and history contract

Every edit creates an immutable version. The active version is resolved when a message is accepted or queued, and the exact Skill/version pair is stored with that message. Editing or deleting a Skill therefore cannot rewrite historical chat context. Retries use the versions recorded on the original message. Scheduled routine Bot steps resolve assigned Skills when the durable run executes.

Prompt assembly is deterministic: Skills are ordered by normalized name and stable ID; context labels and tool references are independently sorted. HomeBot sends providers a clearly delimited Skill block followed by the unmodified user message. Provider-specific formats never enter the server protocol.

## Server API

All operations require the same authenticated v1 transport as other HomeBot state:

- `GET/POST /api/v1/skills` lists or creates Skills.
- `PUT/DELETE /api/v1/skills/{skill_id}` creates a version or soft-deletes the Skill.
- `POST /api/v1/skills/{skill_id}/duplicate` copies the active definition into a new Skill.
- `GET /api/v1/skills/{skill_id}/export` produces format version 1 of the portable bundle.
- `POST /api/v1/skills/import` supports `reject`, deterministic `rename`, or `create_version` conflict policy.
- `PUT /api/v1/skills/{skill_id}/assignment` assigns or removes a Skill from a Bot.

Mutation idempotency keys also identify created Skills or immutable edit versions. Import/export omits owner IDs, Bot assignments, and secret values. Invalid bundle versions, malformed identifiers, duplicate context labels/tool references, control characters, and bounded-size violations fail before persistence.

Desktop receives the initial Skill library in the authenticated snapshot and maintains a replaceable projection from sequenced `skill_changed` and `skill_removed` events. Android representations are generated from the Rust-owned protocol contract.
