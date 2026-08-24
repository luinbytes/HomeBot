# Grok Bot UI reference

Research snapshot: 24 August 2026. Official SpaceXAI/xAI material establishes
the product behavior; the pinned release reconstruction below supplies the
measurable desktop geometry and theme tokens that the official docs omit.

## Current official 0.24.0 capture

The current [official Grok Bot page](https://x.ai/bot) supplied the signed
macOS arm64 0.24.0 release used for this comparison:

- DMG: `https://downloads.cursor.com/grokbot/stable/darwin-arm64/0.24.0/Grok_Bot_0.24.0.dmg`
- DMG SHA-256: `255873da42d2f19b27d7f34cdfb5b058002095ade883d8b321d6494f3cf6c615`
- bundle: `com.anysphere.sand`, version `0.24.0`
- signer: Developer ID Application: Anysphere Incorporated (`DCNK4UB866`)

The current packaged renderer retains a 280px sidebar and 52px title bar. Its
sidebar opens and closes over 200ms with a responsive cubic curve; ordinary
control state transitions are predominantly 90–180ms. The light semantic
colors remain `#fcfcfc` base/elevated, `#f7f7f7` subtle, `#14141426` border,
`#77777717` hover, `#7777772b` selected, and `#1084fe` accent.

The [official launch video](https://x.ai/news/introducing-grok-bot) provides a
clear populated desktop frame around 78 seconds. It shows a quiet inline
Search row, dense avatar/title/preview/trailing-state sidebar rows, restrained
selected state, fixed conversation header, and a bottom-anchored rounded
composer with a circular vector send control. These observations were made
from locally extracted frames; the proprietary frames are deliberately not
redistributed in this repository.

## Pinned local 0.18.0 reference and measurable parity contract

The concrete desktop reference for this work is the macOS arm64 Grok Bot
0.18.0 release, represented by the read-only
[reconstruction pinned at commit `a9f633e`](https://github.com/b-nnett/grok-bot-0.18-reconstructed/tree/a9f633e09d49a85829b8236331b9e21f7e612634).
This is an unofficial, source-oriented reconstruction, not an official Grok
source repository. Its
`PROVENANCE.md` records the public release artifact and its immutable hashes:

- DMG: `https://downloads.cursor.com/grokbot/stable/darwin-arm64/0.18.0/Grok_Bot_0.18.0.dmg`
- DMG SHA-256: `a253ccd8aab01e083f9812a0264354c5034d8ba7f0610bbb557e82ae77d203eb`
- original `app.asar` SHA-256:
  `6665408168466f9cacc6087e917890c17f59d2e2e9c2404a5c4a59ad79c1de58`

Evidence paths in the table are relative to that pinned commit. The
reconstruction explicitly says that `frontend/` is partial
evidence-backed source, while the preserved release artifact is the product
specification. The paths below therefore identify observable behavior and
geometry without presenting reconstruction code as an official API.

| Area | Observable 0.18 behavior | Acceptance check for HomeBot | Evidence source |
| --- | --- | --- | --- |
| App frame | Full viewport is owned by the shell; `html`, `body`, and `#root` do not scroll. The sidebar and chat stage are separate flex/grid regions. | At any window size, only the intended sidebar list, transcript, and settings/plugin bodies scroll. The page itself never acquires a second scrollbar. | `frontend/src/production/production.css:18-26`; `frontend/src/recovered/features/conversation/workspace/view.css:8-17,98-108` |
| Sidebar geometry | Header is 50px high. List has 4px top, 12px horizontal, and 24px bottom insets. Expanded rows are a 34px avatar + flexible body + trailing status/time, with 9px column gap and 58px minimum height. | Resize the Mac window and verify header/list/footer remain stable; rows do not jump when preview or working state changes. | `frontend/src/recovered/features/conversation/workspace/view.css:8-17,48-60`; `frontend/src/recovered/features/conversation/workspace/sidebar.tsx:123-140,204-220` |
| Chat header and transcript | Chat header is at least 51px high with 16px horizontal padding and a 28px avatar. The transcript is the only primary scrolling surface and uses 28px vertical padding with a 690px content measure and 30px minimum side padding. | Header and composer stay fixed while transcript scrolls. Content remains readable at narrow and wide Mac sizes without horizontal page drift. | `frontend/src/recovered/features/conversation/workspace/view.css:98-109` |
| Message sizing | Message bubbles are capped at `min(88%, 640px, calc(100% - 82px))`, use 8px/12px padding and an 18px radius. Rows have 22px bottom spacing. | Long assistant output wraps inside the bubble; it never expands the entire transcript column or creates a detached activity stack. | `frontend/src/recovered/features/conversation/workspace/view.css:109-126` |
| One chronological feed | `ConversationTranscript` maps one `entries` array once. Message, `thinking`, `tool-call`, notices, timeline events, computer handoffs, permissions, files/cards, and send-message entries occupy their event-time positions. | Seed interleaved message/thinking/tool/message data and assert rendered DOM order exactly equals input order. Thinking and commands must not be collected below all messages. | `frontend/src/recovered/features/conversation/workspace/transcript.tsx:616-707`; `frontend/src/production/model.ts:463-477,497-510` |
| Activity rows | Thinking and tool calls are compact inline outline rows: 34px minimum row, 9px border radius, 11px label, one-line 10px preview, chevron, and local expand/collapse. Tool details scroll within a 220px max-height `<pre>`; pending tools spin and failed tools show an error icon. | Default activity is compact. Clicking one row expands only that row in place; details never render as a full-height panel after the transcript. A long result gets an inner scrollbar. | `frontend/src/recovered/features/conversation/workspace/transcript.tsx:571-613`; `frontend/src/recovered/features/conversation/workspace/view.css:249-262` |
| Initial/history request | The renderer makes one `getAgentTranscriptTail({ id, limit, beforeSeq })` call. The response contains one ordered `entries` array and an optional `nextBeforeSeq`; the server reads rows by sequence and reverses the tail page before returning it. | Initial load and “load older” use the same transcript-tail request shape. There is no separate reasoning poll, command poll, or post-message activity request. | `frontend/src/production/ProductionRenderer.tsx:974-980`; `source/host/extensions/session/agent-db-transcript-pages.ts:1-6`; `source/host/extensions/session/agent-db-schema.ts:63` |
| Live updates | One shared client `transcript` subscription receives `snapshot`, `appended`, `updated`, and `cleared` records. Entries are projected through the same order-preserving boundary as tail pages. | Subscribe once per app/client, not once per message kind. An update replaces its entry identity in place; it does not append a second reasoning/command section. | `frontend/src/recovered/features/conversation/cards/transcript-card/transcript-feed-source.ts:1-16,55-122`; `frontend/src/production/model.ts:463-477` |
| Older-page anchoring | History loads when the transcript is within 600px of the top. The controller records `scrollTop` and `scrollHeight`, prepends unique older entries, then restores `scrollTop + (newScrollHeight - oldScrollHeight)`. | Scroll upward while a page loads: the message under the pointer stays stationary. No jump to top, bottom, or a rebuilt list. | `frontend/src/recovered/features/conversation/workspace/transcript.tsx:651-676`; `frontend/src/recovered/features/conversation/workspace/pagination.ts:194-253` |
| Message actions and macOS right-click | Actionable messages expose a hover/focus toolbar without layout shift: reaction (when provided), reply, and “More message actions”. Right-click opens the same menu unless the target is a link/image/input/contenteditable or text is selected. Menu items are Reply, Start a thread, and Copy; Escape restores trigger focus and outside pointer closes. | On Mac trackpad/mouse right-click every ordinary sent message and verify the menu opens at the message. Verify text selection and link/image right-click retain native behavior. Verify Tab/Enter/Escape work without losing focus. | `frontend/src/recovered/features/conversation/workspace/transcript.tsx:145-223`; `frontend/src/production/production.css:127-134` |
| Sidebar context menu | Sidebar rows support pointer context menu and the `ContextMenu`/Shift+F10 keyboard equivalent. The menu can expose Edit Profile, Show full conversation, Show async tasks, Move to section, Pin/unpin, Duplicate, Copy conversation ID, Mark unread/read, Hide, and Delete according to capabilities. | Right-click and keyboard-open a sidebar row; each enabled item performs its mutation and dismisses the menu. No action is inert or dependent on a platform-specific mouse event. | `frontend/src/production/AgentRowActions.tsx:45-107`; `frontend/src/recovered/ui/sand-floating-primitives.tsx` |
| Global keyboard behavior | `mod` means Meta or Ctrl. Registered commands include New Bot (`mod+n`), Jump to (`mod+k`), Settings (`mod+,`), Customize (`mod+shift+m`), Focus prompt (`mod+i`/`mod+l`), Search (`mod+shift+f`), Find (`mod+f`), previous/next agent (`alt+up`/`alt+down`), back/forward (`mod+[`/`]`), agent 1–9, and compact sidebar (`mod+b`). Escape closes the active overlay. | Exercise each Mac shortcut with focus in the composer and outside it. Editable content must keep text-editing shortcuts while global commands explicitly marked content-editable-safe still work. | `frontend/src/recovered/features/window-chrome/global-keyboard-shortcuts.ts:5-18,44-73,98-141,173-183` |
| Composer | Bottom dock is fixed by flex layout with 8px top, 18px bottom, and 24px/centered 700px horizontal insets. Prompt shell is a 16px-radius bordered surface with 9px padding and a 48px minimum field. Attach, mic, and send controls are 30px circular buttons; attachments wrap as compact chips and drag/drop has a bounded overlay. | Composer remains visible while the transcript scrolls. Paste, attach, drag/drop, voice, reply state, and send all update the same composer state; sending while the agent runs appends the user entry in chronological position. | `frontend/src/recovered/features/conversation/workspace/view.css:293-329`; `frontend/src/production/ProductionRenderer.tsx:989-1000`; official attachment behavior in the links above |
| Settings overlay | Settings is a modal dialog, not a route or replacement sidebar: max 860px wide by 620px high (with 16px viewport margins), 14px radius, dark elevated surface and shadow. It has a 190px navigation column, 54px panel header, 24px body padding, active selected nav row, close button, backdrop/Escape close, and focus trapping. | Account/settings and `Cmd/Ctrl+,` open a centered modal. Verify every visible row has a working control, close returns focus, and settings does not reorder or remount the transcript underneath. | `frontend/src/recovered/features/settings/overlay/view.tsx`; `frontend/src/recovered/features/settings/overlay/view.css:4-20,22-48` |
| Plugins | Plugins is a separate modal surface capped at 820px by 620px with a 14px radius. It supports browse/filter/search and explicit Add/Install, Authenticate, Authorize, Enable/Disable, Remove/Uninstall, Edit Values, and tool toggles. | Open Plugins from the sidebar/account flow and verify each displayed action changes state or reports its real capability error; no placeholder button silently does nothing. | `frontend/src/recovered/features/plugins/overlay/browser.css`; `frontend/src/recovered/features/plugins/overlay/browser.tsx` |
| Device linking | The 0.18 reference has no authoritative device pairing, QR, device list, or unlink contract. | Do not label HomeBot’s Link Device UI “Grok parity.” Test and document it as a HomeBot-owned flow with an explicit success/error state. | Official sign-in/settings links above; absence confirmed against the pinned reconstruction’s settings sections in `frontend/src/recovered/features/settings/overlay/view.tsx` |
| Transport/reconnect | The coordinator gateway uses a single `/events` `text/event-stream` connection with bounded reconnect (1s minimum, 10s maximum) and a 35s stall timeout. | A reconnect must preserve the transcript and pending composer state, then resume one shared stream; it must not duplicate entries or create a second poll loop. | `source/node-agent-coordinator/gateway/gateway-client.ts` |

### Observable acceptance checklist

Run these checks against a real Mac build at both a normal desktop size and a
narrow window. They are intentionally behavioral and DOM-observable, so a
screenshot alone cannot pass them:

1. Insert `user → thinking → tool-call → assistant → approval → tool-result`
   fixtures and verify the rendered `data-entry-id`/`data-kind` sequence is
   unchanged. No activity block may be moved below the last message.
2. Expand a thinking row and a tool row independently. The surrounding entry
   order, row position, and scroll anchor must remain unchanged; tool detail
   must cap at 220px and scroll internally.
3. Load an initial page and an older page, recording request names and payloads.
   Both must use `getAgentTranscriptTail`; no kind-specific polling request is
   allowed. Confirm older entries prepend and the prior viewport anchor is
   preserved.
4. Attach a live `transcript` stream and send `snapshot`, `appended`,
   `updated`, and `cleared` events. Confirm one subscription handles all kinds,
   updates replace by stable id, and reconnect does not duplicate entries.
5. Scroll upward during an older-page response and during a live append. The
   viewport must not jitter; live auto-follow is allowed only when already at
   the bottom, with a visible jump-to-latest affordance otherwise.
6. On macOS, right-click a message, right-click selected text, right-click a
   link/image, and right-click a sidebar row. Confirm the correct native/menu
   behavior for each and verify Shift+F10/ContextMenu opens the sidebar menu.
7. Exercise `Cmd+,`, `Cmd+K`, `Cmd+I`, `Cmd+F`, `Cmd+1`, and `Cmd+B` with and
   without composer focus. Confirm overlays trap/restore focus and all visible
   settings/plugin controls have a real callback and error state.
8. Paste text, a link, and an image; attach files by picker and drag/drop; send
   while the agent is running; and cancel/retry a failed send. Verify the same
   composer remains anchored and all resulting entries stay chronological.

### Claims this reference cannot verify

The pinned artifact and first-party docs do not provide a deterministic answer
for exact sidebar width at every breakpoint, native macOS title-bar treatment,
font-file licensing/selection outside the recovered CSS tokens, simultaneous
event tie-breaking, scrollbar thumb appearance, smooth-scroll timing,
auto-follow threshold beyond the implementation above, or the visual content
of unreproduced provider/computer states. They also do not define device
pairing. Those details require a captured build or an explicit HomeBot design
decision; they must not be presented as proven 1:1 Grok behavior.

## What the official material establishes

### Transcript and activity

- Grok Bot presents the work as a conversation: the transcript contains normal
  messages, tool activity, computer use, created files, questions, and approval
  requests "alongside" one another. This is a single mixed activity stream,
  not a separate activity feed placed after all messages. [Message and
  collaborate](https://docs.x.ai/grok-bot/chat-and-collaboration#message-a-bot)
- Replies belong in threads when feedback applies to one result or approval;
  threads keep the main transcript focused. Reactions are for lightweight
  acknowledgement, while a changed instruction should be a written reply.
  [Message and collaborate](https://docs.x.ai/grok-bot/chat-and-collaboration#use-threads-and-reactions)
- Generated files, links, images, and tool results are represented as cards in
  the conversation and can be opened for preview or follow-up. [Files and
  results](https://docs.x.ai/grok-bot/files-and-results#preview-generated-work)
- The docs do not define a separate visible "reasoning" card, its default
  collapsed/expanded state, a maximum activity-card height, or an activity
  density rule. The xAI Grok 4 launch page does show a separate `Show entire
  trace` affordance and a short thought/activity summary, but explicitly for
  Grok 4's product example—not a documented Grok Bot desktop contract. [Grok
  4](https://x.ai/news/grok-4#native-tool-use)

Implementation implication (inference): persist and render every transcript
entry in event-time order, including assistant text, user text, tool calls,
tool results, questions, approvals, and files. Keep the entry in that position
when its details expand; use a thread/detail view for verbose content instead
of appending a giant reasoning/tool column below the conversation.

### Scrolling and anchoring

- The sources do not specify scroll physics, animation timing, scrollbar
  treatment, or whether a new event forces the viewport to the bottom.
- Grok Bot explicitly allows the user to leave the computer preview while work
  continues and says cloud work continues when the desktop or iPhone app is
  disconnected. [Use the computer and apps](https://docs.x.ai/grok-bot/computer-and-apps#watch-computer-work);
  [Troubleshooting](https://docs.x.ai/grok-bot/troubleshooting)

Implementation implication (inference): preserve the user's offset while they
are reading history; auto-follow only when already near the bottom; expose a
clear jump-to-latest affordance when new activity arrives away from the
bottom. Do not rebuild/reorder the list on each streaming update.

### Message actions and menus

- Desktop chat supports replying to a specific message and reacting to a
  message. [Message and collaborate](https://docs.x.ai/grok-bot/chat-and-collaboration#message-a-bot)
- Bot actions include Edit Profile, pin, hide, duplicate, and delete. Hidden
  conversations are restored from Show hidden chats; hiding does not pause the
  Bot or its routines. [Create and manage Bots](https://docs.x.ai/grok-bot/bots#pin-or-hide-a-bot);
  [Create and manage Bots](https://docs.x.ai/grok-bot/bots#duplicate-a-bot)
- The docs do not say that macOS right-click is required, nor do they specify a
  context-menu placement, menu labels, hover affordance, or keyboard menu
  shortcut. They establish the actions, not the exact interaction mechanism.

Implementation implication (inference): make Reply and React reachable from
the message itself (with a keyboard/context-menu equivalent where the desktop
platform provides one), and keep Bot lifecycle actions in the Bot menu. Do not
hide essential actions behind right-click only.

### Sidebar, header, and composer

- Open a Bot from the sidebar. New in the sidebar creates a Bot or a group;
  pin keeps active Bots at the top, hide removes them from the main list, and
  Show hidden chats restores them. The Bot list communicates Needs attention,
  unread activity, and working/typing states; opening a conversation marks
  current activity read. [Create and manage Bots](https://docs.x.ai/grok-bot/bots#create-a-bot);
  [Settings and notifications](https://docs.x.ai/grok-bot/settings-and-notifications#understand-attention-states)
- Search/command-palette behavior is documented for switching Bots/groups,
  finding prior messages/files/links/routines, opening settings, and jumping
  to a matching place in conversation history. [Message and collaborate](https://docs.x.ai/grok-bot/chat-and-collaboration#find-prior-work)
- Conversation details expose Agent Computer and the Bot profile/settings.
  Agent Computer previews clicks, typing, navigation, and current status;
  takeover is a deliberate control handoff for passwords, MFA, CAPTCHAs, and
  similar sensitive steps. [Use the computer and apps](https://docs.x.ai/grok-bot/computer-and-apps#watch-computer-work);
  [Get started](https://docs.x.ai/grok-bot/get-started#sign-in-to-the-tools-it-needs)
- The desktop composer accepts pasted text/links/images, local attachments,
  `/` saved-skill references, `@` Bot/group/routine/connector mentions, and a
  new instruction while work is running. Attachments can be selected or
  dragged into the composer; desktop permits up to six at once. [Message and
  collaborate](https://docs.x.ai/grok-bot/chat-and-collaboration#message-a-bot);
  [Files and results](https://docs.x.ai/grok-bot/files-and-results#attach-files)
- An in-app error appears above the composer in Notifications and can be
  dismissed or cleared; clearing the notice does not remove the underlying
  action or history. [Settings and notifications](https://docs.x.ai/grok-bot/settings-and-notifications#handle-in-app-errors)

The current signed desktop artifact establishes the sidebar/header geometry,
semantic colors, and common motion timing above. Exact font metrics, every
responsive breakpoint, and unrecorded hover/menu states remain capture-required.

### Settings and device/account flows

- Settings opens from the account menu or `Cmd/Ctrl+,`. Documented sections
  include Account, Appearance (Follow System/Light/Dark), Agent (model,
  timezone, local execution, Auto Review), Plugins, Usage & Billing, Team
  Setup, and Beta/updates. [Settings and notifications](https://docs.x.ai/grok-bot/settings-and-notifications#open-grok-bot-settings)
- App updates and Agent Computer updates are separate. Beta exposes Check for
  Updates/Restart to Update, Update Agent Computer, and Reset Agent Computer;
  reset is the last-resort path because it can lose recent unsynced work.
  [Settings and notifications](https://docs.x.ai/grok-bot/settings-and-notifications#beta-and-updates);
  [Troubleshooting](https://docs.x.ai/grok-bot/troubleshooting#the-computer-cannot-be-reached)
- Plugins are added through Settings → Plugins → Add, then authenticated in a
  browser; installed connector tools can be enabled/disabled individually.
  [Use the computer and apps](https://docs.x.ai/grok-bot/computer-and-apps#connect-an-app)
- The sources describe Cursor sign-in and browser-based authentication, not a
  HomeBot-style "Link device" control. There is no authoritative Grok Bot
  desktop behavior for pairing, QR codes, device lists, or device unlinking.
  Treat those as HomeBot-original flows, not parity requirements. [Get started](https://docs.x.ai/grok-bot/get-started#sign-in)

## Unobservable states and parity boundary

No first-party source inspected here establishes: exact chronological tie
breaking for simultaneous events; the visible rendering of reasoning; default
collapse state or truncation for tool output; scroll anchoring/overscroll;
macOS context-menu contents; complete typography; every settings state; or a
Link Device/pairing flow. The launch video is a legitimate first-party visual
reference for its recorded populated state, but not for surfaces it never
shows. Any HomeBot behavior chosen for those gaps should be documented as
HomeBot design, not claimed as 1:1 Grok parity.
