# Android design QA

- Source visual truth: `/tmp/homebot-design-refs/apple-messages-ios26-pins.png` (760 × 890 px) and `docs/evidence/homebot-grok-parity-macos-chat.png` (1120 × 788 px).
- Implementation screenshot: unavailable.
- Intended viewport: Android phone, 360–430 dp wide, portrait and landscape, system light and dark themes.
- State: connected conversation index with pinned and recent Bots.
- Density normalization: not performed because no implementation capture exists.

**Findings**

- [P1] Rendered fidelity is unverified.
  - Location: Android conversation index and chat surfaces.
  - Evidence: the source references were opened and inspected, but no emulator or physical Android device was used, per the host-resource constraint.
  - Impact: typography, wrapping, landscape behavior, largest-font scaling, and exact light/dark contrast cannot be judged from source code or lint output.
  - Fix: capture the connected home, chat, Tools, source-control, light-theme, dark-theme, landscape, and largest-font states on a physical device or a separately authorized resource-capped emulator, then compare them with the references in one visual input.

**Open Questions**

- Whether the phone rendering needs tighter spacing after physical-device review.

**Implementation Checklist**

- Capture the required Android states at a fixed device size and density.
- Run the side-by-side comparison and fix any P0/P1/P2 findings.
- Record the post-fix captures and change `final result` only after the comparison passes.

**Follow-up Polish**

- Evaluate whether the pinned row should show status text or use the extra vertical space for larger Bot identities.

final result: blocked
