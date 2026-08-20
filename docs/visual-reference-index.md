# Grok Bot visual reference index

Baseline: 20 August 2026. This index is the canonical input to HomeBot's future egui and Android golden tests. It records independently observed product behaviour from public, authoritative SpaceXAI material without copying proprietary source code or redistributing proprietary assets.

## Reference set

- **Launch**: [Introducing Grok Bot](https://x.ai/news/introducing-grok-bot), including the official launch video and product examples.
- **Start**: [Get started](https://docs.x.ai/grok-bot/get-started).
- **Bots**: [Create and manage Bots](https://docs.x.ai/grok-bot/bots).
- **Chat**: [Message and collaborate](https://docs.x.ai/grok-bot/chat-and-collaboration).
- **Files**: [Files and results](https://docs.x.ai/grok-bot/files-and-results).
- **Computer**: [Use the computer and apps](https://docs.x.ai/grok-bot/computer-and-apps).
- **Skills**: [Skills and routines](https://docs.x.ai/grok-bot/skills-routines-and-automations).
- **Settings**: [Settings and notifications](https://docs.x.ai/grok-bot/settings-and-notifications).
- **Security**: [Approvals, security, and privacy](https://docs.x.ai/grok-bot/approvals-security-and-privacy).
- **Mobile**: [Grok Bot for iOS](https://docs.x.ai/grok-bot/mobile).
- **Troubleshooting**: [Troubleshooting](https://docs.x.ai/grok-bot/troubleshooting).

The public sources specify product surfaces and observable states. Exact pixel measurements, colours, font metrics, hover transitions, and animation timing must be captured from a legitimately accessed current app build during 6C7-42 before a HomeBot golden can become `Pass`. Where public material does not expose a pixel reference, the row remains `Capture required`; implementers must not invent Grok-specific styling and call it parity.

## Desktop surfaces and states

| Golden ID | Surface/state | Authoritative behavioural reference | Required HomeBot evidence | Visual status |
| --- | --- | --- | --- | --- |
| `desktop.welcome.signed_out` | Welcome and Get started | Start, Sign in | macOS Intel/arm64 golden | Capture required |
| `desktop.onboarding.intro` | Bots/shared computer/routines introduction | Start, Sign in | Step-by-step onboarding goldens | Capture required |
| `desktop.onboarding.tools` | Tool-use questions | Start, Sign in | Selection and validation goldens | Capture required |
| `desktop.computer.provisioning` | Starting/Updating your computer progress | Troubleshooting, computer setup | Progress, stalled, retry goldens | Capture required |
| `desktop.teammate.suggestions` | Meet a future teammate | Start, Create first Bot | Suggested and custom paths | Capture required |
| `desktop.bot.create` | Create your own/New Agent | Start; Bots, Create a Bot | Empty, valid, validation states | Capture required |
| `desktop.roster.empty` | No Bots or groups yet | Start, first-Bot flow | Empty roster golden | Contract defined; pixel capture required |
| `desktop.roster.populated` | Pinned/normal/hidden conversations | Bots, Pin or hide | Light/dark and long-name goldens | Capture required |
| `desktop.roster.attention` | Needs attention, unread, working/typing | Settings, attention states | Every indicator and combination | Capture required |
| `desktop.chat.empty` | New Bot conversation before first message | Bots, Create a Bot | Empty timeline and starter affordance | Contract defined; pixel capture required |
| `desktop.chat.idle` | Durable transcript at rest | Chat, Message a Bot | Short/long/mixed-part transcript | Capture required |
| `desktop.chat.streaming` | Assistant streaming and working state | Chat, transcript/tool activity | Streaming frame sequence | Capture required |
| `desktop.chat.question` | Bot requires user input | Chat; Settings, attention | Inline question and roster attention | Capture required |
| `desktop.chat.redirect` | New instruction while work is active | Chat, Redirect work in progress | Queued/redirected state | Capture required |
| `desktop.chat.stopping` | Stop requested/in progress/completed | Chat, Redirect work in progress | Cancellation lifecycle goldens | Capture required |
| `desktop.chat.error` | Request/tool/provider error above composer | Settings, in-app errors; Troubleshooting | Retryable/non-retryable variants | Capture required |
| `desktop.composer.default` | Text composer | Chat, Message a Bot | Empty, focused, multiline, disabled | Capture required |
| `desktop.composer.mention` | `@` mention picker | Chat, Message a Bot | Bot/group/routine/plugin results | Capture required |
| `desktop.composer.skill` | `/` skill picker | Chat; Skills, Save a skill | Empty/search/selected states | Capture required |
| `desktop.composer.attachments` | Drag/paste/select files | Files, Attach files | Uploading, complete, limit/error | Capture required |
| `desktop.message.reply` | Thread/reply view | Chat, threads and reactions | Collapsed/open thread states | Capture required |
| `desktop.message.reactions` | Message reactions | Chat, threads and reactions | Picker and applied reactions | Capture required |
| `desktop.activity.tool` | Tool activity in transcript | Chat, transcript activity | Running/success/failure/expanded | Capture required |
| `desktop.activity.file` | File/result card and preview | Files, Preview generated work | File types, preview, unavailable | Capture required |
| `desktop.activity.terminal` | Command and output activity | Computer, command line; Chat activity | Approval/running/output/error | Capture required |
| `desktop.activity.browser` | Browser action and current status | Computer, Watch computer work | Observe/takeover/disconnected | Capture required |
| `desktop.approval.pending` | Proposed target/scope/arguments | Security, Review an action | Allow once/deny/persistent rule | Capture required |
| `desktop.approval.resolved` | Approved, denied, expired, stale | Security; Troubleshooting, approval blocked | All terminal states | Capture required |
| `desktop.group.create` | Select multiple Bots | Chat, Start a group chat | 2-6 Bots, validation, generated name | Capture required |
| `desktop.group.timeline` | Multi-Bot messages and ownership | Chat, Direct a message and handoff | Mentions, owner, parallel status | Capture required |
| `desktop.bot.profile` | Name/title/description/avatar | Bots, Edit a Bot; Settings, Edit one Bot | Edit, validation, save failure | Capture required |
| `desktop.bot.actions` | Pin/hide/duplicate/delete menu | Bots, lifecycle sections | Enabled/disabled/destructive states | Capture required |
| `desktop.routines.list` | Bot routines and recent runs | Skills, Manage routines | Empty, enabled, paused, failure | Capture required |
| `desktop.routine.editor` | Schedule/instruction/trigger editor | Skills, Create/trigger/manage routine | Valid, invalid, unsaved, test-run | Capture required |
| `desktop.routine.recording` | Teach a task recording | Skills, Teach by demonstration | Recording, time limit, review draft | Capture required |
| `desktop.plugins.catalog` | Plugin discovery | Computer, Connect an app | Empty/loading/connect states | Capture required |
| `desktop.plugin.detail` | Auth/health/remove | Computer; Troubleshooting, plugin auth | Waiting/connected/error/revoked | Capture required |
| `desktop.settings.general` | General and Auto Review | Security, Configure Auto Review | Default, rule editor, conflict | Capture required |
| `desktop.settings.appearance` | Appearance/theme | Mobile settings; roadmap parity | Light/dark/system states | Capture required |
| `desktop.settings.notifications` | Per-Bot notification preference | Settings, Control notifications | Permission allowed/denied | Capture required |
| `desktop.settings.updates` | Check/restart/update computer | Start; Troubleshooting, Update Grok Bot | Checking/ready/error states | Capture required |
| `desktop.connection.offline` | Client disconnected while work continues | Troubleshooting opening statement | Reconnecting, cached, resumed | Contract defined; pixel capture required |
| `desktop.computer.unreachable` | Retry/recover/update/reset | Troubleshooting, computer unreachable | Least-destructive action hierarchy | Capture required |

## Android surfaces and states

HomeBot intentionally exceeds initial Grok Bot mobile parity where the official iOS client defers advanced controls to desktop. Android still preserves the mobile visual character and exposes every relevant server-backed capability.

| Golden ID | Surface/state | Authoritative behavioural reference | Required HomeBot evidence | Visual status |
| --- | --- | --- | --- | --- |
| `android.pairing.scan` | QR/deep-link pairing | HomeBot protocol/security contract | Fresh/expired/used/invalid token | HomeBot-original design required |
| `android.home.roster` | Bot/group list and search | Mobile, Create Bots/groups and search | Empty/populated/attention/offline | Capture required |
| `android.bot.create` | New Bot profile | Mobile, Create Bots and groups | Valid/invalid/provider unavailable | Capture required |
| `android.group.create` | New group and membership | Mobile; Chat, Start a group | Selection/validation states | Capture required |
| `android.chat.timeline` | Direct/group transcript | Mobile, Message a Bot | Streaming/reply/reaction/mention | Capture required |
| `android.chat.composer` | Text/dictation/photo/file | Mobile, Message a Bot | Permission, upload, offline queue | Capture required |
| `android.activity.approval` | Approval request | Security, Review an action | Approve once/deny/stale | Capture required |
| `android.computer.observe` | Watch/take over/return control | Mobile, Review the computer | Loading/live/disconnected | Capture required |
| `android.routines` | List, schedule, history, controls | Mobile, Manage recurring work; Skills | Full HomeBot parity states | Capture required |
| `android.plugins` | Install/review plugins | Mobile, Settings | Auth and health states | Capture required |
| `android.notifications` | Result/question/approval push | Mobile setup; Settings notifications | Permission/deep-link/fallback | Capture required |
| `android.settings` | Appearance, devices, providers | Mobile, Settings | Full HomeBot settings reachability | Capture required |
| `android.connection` | Offline/reconnecting/resumed/revoked | Troubleshooting; HomeBot protocol contract | Network-change and revocation | HomeBot-original design required |

## Golden-test rule

Every future visual implementation issue must name the affected IDs, add or update deterministic goldens, record the renderer/platform/font inputs, and keep the status `Capture required` until a legitimate reference comparison has been performed. Feature existence alone never changes a row to `Pass`.
