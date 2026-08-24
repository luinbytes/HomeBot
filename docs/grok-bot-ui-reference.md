# Grok Bot UI reference

Research snapshot: 24 August 2026. This is a behavior reference, not a pixel
specification. Sources are first-party SpaceXAI/xAI pages only. The public
launch page is a product introduction and its embedded video is not a stable
pixel fixture; the linked open-graph image is a title card, not a desktop
screen capture.

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

The docs do not expose exact sidebar width, header height, composer geometry,
button order, colors, type scale, hover states, right-click behavior, or
whether controls are icon-only versus labeled. Those remain capture-required.

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
macOS context-menu contents; precise desktop geometry or typography; settings
modal layout; or a Link Device/pairing flow. The official launch page's [embedded
video](https://media.x.ai/v1/website/260810_2245_bw_dr_cursor_bot_edit_v8-60724aba.mp4)
and [open-graph image](https://x.ai/images/news/introducing-grok-bot-og-2.png)
are legitimate first-party media, but do not provide a deterministic,
inspectable reference for those states. Any HomeBot behavior chosen for these
gaps should be documented as HomeBot design, not claimed as 1:1 Grok parity.
