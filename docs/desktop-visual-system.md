# Desktop visual system

HomeBot's desktop UI is an independent egui implementation informed by observable Grok Bot behavior. It does not copy proprietary source or assets. The authoritative visible-state inventory remains [visual-reference-index.md](visual-reference-index.md); a HomeBot golden proves deterministic rendering, not Grok Bot parity by itself.

## Token ownership

All reusable visual values live in `crates/homebot-desktop/src/tokens.rs`:

- light and dark semantic palettes, including Bot identity colors;
- typography roles and line-height intent;
- a compact spacing scale;
- corner-radius roles;
- window, sidebar, roster, avatar, activity and composer geometry;
- panel and popup shadows;
- interaction timing roles.

Components consume semantic values instead of choosing raw colors or dimensions. The only numeric geometry outside the token module is mathematical construction of shapes, such as circle radii and the six vertices of a hexagonal Bot avatar. These are shape algorithms, not adjustable styling values.

The native executable uses eframe 0.32.3 with accessibility, OpenGL, Wayland and X11 support. Its Windows clipboard transitive dependencies use the OSI-approved permissive Boost Software License 1.0, which is admitted explicitly by the dependency policy.

`HomeBotTheme::install` translates tokens into egui `Style` and `Visuals`. Future visible components must accept a `HomeBotTheme` or a narrower semantic token set. A component may introduce a documented pixel-level exception only when a legitimate reference capture proves it is necessary.

## Production shell and deterministic goldens

The ordinary desktop hierarchy is a responsive two-pane shell: an identity-led sidebar occupies 30% of the reference window (bounded from 276 to 324 logical points), while chat remains the dominant surface. Computer, workspace, checkpoint, source-control and context details are opened from the conversation header instead of permanently occupying a third pane. The bottom composer is a persistent rounded action surface; queue, steering and stop are disclosed only while a Bot is running.

HomeBot avatars are original procedural characters. Stable Bot name, selected color and selected shape determine silhouette, eye spacing, face and accent. No Grok artwork is imported, traced or redistributed.

The release visual harness now constructs deterministic server projections and renders the actual `HomeBotApp::render` path at 1120 × 760 logical points and one pixel per point. It covers:

- `production_chat_dark`
- `production_approval_dark`
- `production_group_chat_dark`
- `production_disconnected_dark`
- `production_provider_unavailable_light`
- `production_computer_details_dark`
- `production_settings_light`
- `production_routines_dark`
- `production_routine_editor_dark`
- `production_routine_recording_dark`

These images exercise the production sidebar, header, transcript, activity and approval cards, anchored composer, settings/routines navigation, routine editor/recording workflows and contextual details. The legacy showcase remains a component development aid, not release evidence.

Goldens are stored under `crates/homebot-desktop/tests/snapshots`. The harness uses egui's normal tessellation with a HomeBot-owned CPU triangle renderer. Fixed nearest-neighbor texture sampling and premultiplied-alpha blending remove GPU, driver, Metal, Vulkan, Wayland and X11 variance. The exact same checked-in image is therefore compared on Linux and macOS.

Run the gate with:

```sh
cargo test -p homebot-desktop --all-features --test visual_goldens
```

After visually reviewing an intentional change, refresh fixtures with:

```sh
UPDATE_SNAPSHOTS=true cargo test -p homebot-desktop --all-features --test visual_goldens
```

Never update a golden merely to silence a failure. Inspect the old, new and diff image and record which visual-reference IDs changed.

## Reference and parity discipline

Public SpaceXAI product documentation and launch material define observable behavior. A current user-supplied Grok Bot desktop capture (22 August 2026; red arrow annotation excluded) supplied the populated-shell comparison: a wide identity/grid/recent-conversation sidebar, two-pane chat, contextual computer action, card-based messages and a quiet anchored composer. Official current documentation additionally confirms the computer takeover flow, file attachment behavior, Bot lifecycle, settings and routine surfaces. Exact typography metrics, colors, hover/focus states and motion for states not visible in a legitimate capture remain `Capture required` in the visual reference index. HomeBot's own green snapshots prove regression stability, not Grok parity by themselves.
