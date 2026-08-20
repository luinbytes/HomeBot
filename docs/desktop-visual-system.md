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

`HomeBotTheme::install` translates tokens into egui `Style` and `Visuals`. Future visible components must accept a `HomeBotTheme` or a narrower semantic token set. A component may introduce a documented pixel-level exception only when a legitimate reference capture proves it is necessary.

## Deterministic goldens

The visual harness renders six reference states at 1120 × 760 logical points and one pixel per point:

- `desktop_empty_light`
- `desktop_chat_light`
- `desktop_approval_dark`
- `desktop_bot_editor_light`
- `desktop_disconnected_dark`
- `desktop_provider_unavailable_light`

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

Public SpaceXAI product documentation and launch material define observable surfaces. Exact typography metrics, colors, spacing, hover/focus states and motion remain `Capture required` in the visual reference index until compared with a legitimately accessed current application build. The initial HomeBot tokens are an original, coherent baseline and are not represented as a completed parity comparison.
