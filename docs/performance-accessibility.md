# Performance, resource and accessibility release gate

HomeBot keeps performance measurements local. The desktop's bounded in-memory telemetry retains at most 512 timing samples, emits only to the local `homebot.performance` tracing target, never transmits them, and discards them at process exit.

## v1 budgets

These are release blockers on macOS Intel, macOS Apple Silicon and representative Arch/Omarchy x86_64 hardware. Android uses the same interaction budgets on a current mid-range physical device.

| Surface | Budget | Automated evidence |
| --- | ---: | --- |
| Headless server cold start through authenticated version response | 5 s | Real SQLite/router integration test |
| Desktop launch through hydrated usable projection | 8 s | Release-machine first-run probe |
| WebSocket reconnect, replay or stale-cursor snapshot fallback | 2 s | Real socket integration tests |
| Open and sort a 10,000-message chat projection | 250 ms | Desktop projection budget test |
| Stream projection latency | 50 ms per event | Eight-Bot/2,000-event projection test |
| Concurrent working Bots | 8 | Eight independent live projections in the budget test |
| Idle server CPU after five minutes | 2% average | Release-machine process probe |
| Idle desktop plus supervised server CPU after five minutes | 3% average | Release-machine process probe |
| Idle desktop plus supervised server resident memory | 350 MiB | Release-machine process probe |
| Android reconnect wakeups while offline | Connectivity callbacks only | Client tests and source gate; no polling loop |

CI runs `scripts/performance-accessibility-gate.sh` plus `scripts/process-resource-budget.sh` on Linux, macOS Intel and macOS Apple Silicon. The clean-install Arch package lifecycle runs the same process probe against the packaged server. The probe starts a real release server with a temporary database, requires health within five seconds, then gates CPU/RSS after a 15-second idle settling period. Wall-clock budgets are intentionally generous enough for shared CI but strict enough to detect accidental blocking work or unbounded projection algorithms. The five-minute physical-machine sample remains part of the final release evidence.

## Platform measurement

Build release binaries first. Start HomeBot with an empty temporary data directory and a credential file containing a generated test token; never print real credentials.

On Linux/Arch, use `/usr/bin/time -v homebot-server` for cold-start maximum RSS, then sample the server and desktop after five idle minutes with `ps -o pid,pcpu,rss,command -p <pid>`. Divide RSS KiB by 1024 for MiB. On macOS, use `/usr/bin/time -l` and `ps -o pid,%cpu,rss,command -p <pid>`. Record the machine model, OS version, architecture, HomeBot commit, database fixture and median of five cold starts in the 6C7-70 release evidence.

Measure desktop readiness from process spawn until the authenticated snapshot has hydrated and the composer accepts focus. Measure reconnect from network restoration until the first replayed event or replacement snapshot is projected. Keep provider latency outside HomeBot's reconnect and UI-frame measurements.

## Accessibility contract

Desktop:

- every operation remains reachable with native Tab/Shift-Tab traversal;
- Command/Ctrl+, toggles Settings, Command/Ctrl+N opens Bot creation, Command/Ctrl+K focuses the composer, and Escape dismisses modal/settings state;
- egui's AccessKit integration exposes native controls; labels never encode status by colour alone;
- light and dark token tests enforce readable contrast guards;
- text scales from 80% to 200% through Settings without changing server state.

Android:

- Compose controls use native roles, selected state, headings and polite/assertive live regions;
- informational cards are not exposed as no-op buttons;
- labels use scalable `sp` typography and layouts remain scrollable;
- Android lint, unit tests and the static semantics gate run in CI;
- notifications and deep links retain exact Bot/chat/activity descriptions.

Before v1, run TalkBack on Android and VoiceOver on both macOS architectures, plus keyboard-only navigation on macOS and Omarchy. Record failures as release-blocking issues rather than waiving them silently.
