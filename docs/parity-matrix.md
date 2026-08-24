# Grok Bot parity matrix

Baseline researched 20 August 2026. `Specified` means an authoritative reference and acceptance contract exist, not that HomeBot implements the row. No row becomes `Pass` without platform evidence. Visual golden references are still required before 6C7-31 can close.

Visible surfaces and state-specific golden IDs are tracked in [visual-reference-index.md](visual-reference-index.md). `Capture required` is intentional: public authoritative material fixes the behavioural contract, while exact current-app pixel comparisons remain a 6C7-42 gate.

Sources: [overview](https://docs.x.ai/grok-bot/overview), [Bots](https://docs.x.ai/grok-bot/bots), [chat and collaboration](https://docs.x.ai/grok-bot/chat-and-collaboration), [files and results](https://docs.x.ai/grok-bot/files-and-results), [computer and apps](https://docs.x.ai/grok-bot/computer-and-apps), [skills and routines](https://docs.x.ai/grok-bot/skills-routines-and-automations), [settings and notifications](https://docs.x.ai/grok-bot/settings-and-notifications), [security and approvals](https://docs.x.ai/grok-bot/approvals-security-and-privacy), [mobile](https://docs.x.ai/grok-bot/mobile), and [FAQ](https://docs.x.ai/grok-bot/faq).

The clean-room-oriented [Grok Bot 0.18 runtime parity audit](grok-bot-0.18-runtime-parity.md)
adds edge-case acceptance detail from a pinned reconstructed-runtime evidence set. It does not
replace the official sources above, grant code-reuse rights, or turn a row into `Pass`.

| Capability/state | Source | Desktop contract | Android contract | Server capability | Acceptance | Status |
| --- | --- | --- | --- | --- | --- | --- |
| Bot create and default identity | Bots | New flow and validation | Mobile create flow | Durable Bot create | Create, restart, reopen | Specified |
| Edit name, title, description, avatar | Bots, settings | Full identity editor | Equivalent editor | Versioned Bot update | Values survive restart and sync | Specified |
| Pin, hide, unhide | Bots | Sidebar controls and hidden list | Mobile swipe/menu | Durable roster state | State syncs without pausing work | Specified |
| Duplicate Bot | Bots | Duplicate action | Equivalent action | Copy config, not history/memory/files | Copy has correct included/excluded state | Specified |
| Delete Bot | Bots, FAQ | Destructive confirmation | Equivalent confirmation | Remove profile/chat/routines, retain shared files | Exact lifecycle proven | Specified |
| Durable Bot memory/context | Bots, overview | Explain retained state | Same visibility | App memory plus provider mapping | Survives restarts and provider switch | Specified |
| Direct chat text/links/images | Chat | Rich composer/timeline | Mobile composer | Durable message parts | Round trip and replay are exact | Specified |
| File attachments | Files | Drag/picker/paste, progress/errors | Picker/camera/photo | HTTP upload and artifact metadata | Limits, resume, corrupt input tested | Specified |
| Skill `/` reference | Chat, skills | Slash menu | Reachable mobile picker | Resolve applied skill version | Historical applied version remains fixed | Specified |
| `@` Bot/group/routine/plugin mention | Chat | Mention menu | Mobile mention menu | Typed reference parts | References survive rename/replay | Specified |
| Reply/thread | Chat | Thread/reply presentation | Mobile thread UX | Parent/thread metadata | Reply context survives reconnect | Specified |
| Reactions | Chat | Reaction controls | Mobile reaction controls | Durable reaction state | Concurrent add/remove reconciles | Specified |
| Streaming response | Chat | Incremental stable timeline | Same stream | Sequenced provider-neutral parts | Disconnect/replay has no duplicates | Specified |
| Redirect while working | Chat | Priority steering | Queue/steer control | Ordered steering policy | New user direction preempts safely | Specified |
| Stop/cancel | Chat | Visible stop and terminal state | Same | Provider/tool cancellation | Children stop; completed effects remain | Specified |
| Activity cards | Chat | Tool/computer/file/question cards | Same detail reachability | Normalised Activity events | No provider payload leakage | Specified |
| Group create, rename, membership | Chat | Select 2 to 6+ Bots | Mobile group flow | Durable participants | Edit membership after restart | Specified |
| Group routing and `@everyone` | Chat | Visible routing | Equivalent | Coordinator policy | Named and automatic routing work | Specified |
| Bot-to-Bot handoff | Chat | Visible asynchronous handoff | Same transcript | Durable inter-Bot message | Receiver wakes and replies later | Specified |
| Ownership and parallel work | Overview, chat | Owner/parallel indicators and stop | Same monitoring | Bounded coordinator | Three Bots recover after restart | Specified |
| Shared files/browser/login state | Overview, computer | Shared-computer explanation | Review surface | User-scoped host state | Handoff can consume saved result | Specified |
| Files/results preview cards | Files | Preview/open/copy/save | Mobile preview/share | Artifact service | Supported result is independently reviewable | Specified |
| Browser live activity/takeover | Computer, mobile | Watch/takeover/return | Mobile watch/takeover | Browser session controller | Sensitive step pauses and resumes | Specified |
| Terminal/filesystem activity | Computer | Expandable commands, paths, output | Remote activity details | Scoped capability APIs | Denials are server-enforced | Specified |
| Plugin discovery/connect/status/remove | Computer | Settings states | Equivalent settings | Plugin/MCP registry | Connect, fail, recover, revoke | Specified |
| Skills create/edit/enable | Skills | Library and per-Bot enable | Equivalent management | Versioned Skill model | Historical prompt assembly is deterministic | Specified |
| Teach workflow recording | Skills | Record/review draft | Mobile can inspect result | Structured action capture | Recording becomes editable/testable skill | Specified |
| Routine create/manual run/test | Skills | Bot routine editor | Mobile create/edit/duplicate/delete and dry-run/Run-now controls | Versioned routine runner | Safe test and run history exist | Specified |
| Scheduled routine/time zone | Skills | Schedule/next-run UI | View/edit controls | Headless scheduler | DST and restart boundary pass | Specified |
| Event-triggered routine | Skills | Narrow event rule UI | Equivalent controls | Idempotent webhook/plugin trigger | Duplicate event produces one run | Specified |
| Routine enable/pause/delete/history | Skills, mobile | Full management | Server-authoritative management and refreshed history | Durable run state/history | No client open required | Specified |
| Structured approval once/deny | Security | Target/scope/value card | Equivalent approve/deny | Operation-digest approval | Prose cannot approve; expiry works | Specified |
| Persistent capability rules | Security | Narrow allow/require/deny rules | Safe mobile management | Server policy evaluator | Deny wins and changes are audited | Specified |
| Secure secret entry | Security | Masked non-chat flow | Android secure entry | OS credential store references | Canary never enters transcript/log | Specified |
| Attention, unread, working states | Settings | Sidebar/dock states | Mobile list states | Read cursor and activity state | Manual and automatic read sync | Specified |
| Notifications/deep links | Settings, mobile | Native finish/input/error | Android native notification | Notification intent events | Click opens exact activity | Specified |
| Error notices | Settings | Dismiss/clear/copy request ID | Equivalent | Safe structured errors | Clearing does not alter history/action | Specified |
| Appearance and settings | Settings, mobile | Light/dark and grouped settings | Mobile-native navigation | Authoritative/shared settings where needed | Theme/focus/error states have goldens | Specified |
| Search prior conversations/results | Mobile | Global search | Home search | Authorised search index | Message/file/link/routine hits open exactly | Specified |
| Desktop/mobile continuity | Overview, mobile | Same server state | Resume/cache/drafts | Authenticated resumable stream | Network change and restart recover | Specified |
| Hosted cloud VM | Overview | Replaced by self-host host | Monitor self-host host | Not implemented by design | Documented only exclusion | Excluded |

## HomeBot differentiators tracked outside strict parity

Repository workspaces, isolated worktrees, per-turn checkpoints, exact/full-chat diffs, safe revert, source-control and PR flows, multi-provider profiles, context compaction, Android controls for all server-supported features, Linux, and headless operation are HomeBot requirements tracked operationally in GitHub Issues #42-#49; their 6C7 identifiers preserve the historical Linear roadmap provenance.
