# Assistant Packs

Assistant Packs are HomeBot's curated personal-assistant marketplace. An installation reuses the existing server-owned primitives: one versioned Skill assigned to one Bot, one enabled routine, and one timezone-safe schedule. The routine writes its result into that Bot's existing chat and run history.

The initial catalog contains Morning Brief, Weekly Rundown, and End-of-Day Review. Catalog contents are bundled with the server; there is no remote publishing format or executable marketplace code in this version.

Installation is `browse -> configure Bot/timezone/time -> install and enable`. The authenticated server validates the Bot and schedule, then commits the Skill, assignment, routine, and trigger atomically. Repeating the same installation returns the existing records; changing the schedule requires editing or removing the installed routine rather than silently replacing it.

Routes:

- `GET /api/v1/assistant-packs`
- `POST /api/v1/assistant-packs/{pack_id}/install`

Weekly schedules use an ISO weekday (`1` Monday through `7` Sunday) and retain the selected local wall-clock time across daylight-saving changes. Installed prompts must distinguish available evidence from unavailable sources and must not claim access to connectors that are not configured.

Public marketplace publishing, OAuth connectors, external-channel delivery, arbitrary executable plugins, and a separate Brief data model remain outside this initial contract.
