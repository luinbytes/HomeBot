# HomeBot autonomous agent state

Updated: 23 August 2026, Europe/London

This is the operational handoff for coding and release agents. GitHub Issues are the source of truth. The 6C7 identifiers are historical Linear references only; do not recreate or synchronize a Linear tracker.

## Authoritative repository state

- Repository: `https://github.com/luinbytes/HomeBot`
- Target branch: `main`
- Latest implementation merge: `4a3b67428e77d445f328e8e543299eda35355cde`
- Tree: `3e1efc4f15de2af4fd20a09a5ff0737104d331e8`
- Complete merged-main gate: Actions run `32663832104`, all 16 jobs passed
- Release identity: `1.0.0` throughout source and packaging metadata
- Public release state: pre-v1; no supported public packages or `v1.0.0` release exist

The complete gate covers Rust format, clippy and workspace tests; schema and generated Android drift; security; dependency audit and policy; performance and accessibility automation; Android quality and CI-only signed packaging; Linux and macOS Intel/Apple Silicon builds, visual goldens and resource budgets; macOS package assembly; and the Arch clean install/update/uninstall lifecycle.

## GitHub release tracker

- #42 `[6C7-65]` M6 release epic — open
- #43 `[6C7-69]` security gate — closed from merged-main evidence
- #47 `[6C7-66]` signed/notarized macOS acceptance — open, externally blocked
- #48 `[6C7-75]` live-provider, physical-platform and assistive-technology acceptance — open, externally blocked
- #49 `[6C7-71]` immutable parity/release gate — open and blocked by #47/#48

Security corrections #44, #45, #46, #52, and #55 are merged and closed. The final security review found no unresolved high/critical issue. `scripts/check-packaging.sh` is executable on `main`.

## Production-path audit

The installed composition roots reach the implemented features:

- the server binary and supervised desktop server share `provider_bootstrap::compose_app_state`;
- Codex, Claude Code, OpenAI-compatible and generic-process adapters register in the real provider runtime;
- authenticated routes expose Bots/chats/groups, approvals, pairing/devices, browser isolation/takeover, plugins/MCP, routines, workspaces/checkpoints/VCS, secrets and events;
- Android and desktop mutations use those server contracts rather than fixture authority;
- the desktop updater verifies the canonical schema-2 manifest with a compile-time Ed25519 key and enforces origin, size and digest checks;
- provider terminal outcomes expire unresolved approvals, cancel unfinished activities and reject late approval execution.

No additional production-composition defect was reproduced in the final coherence audit.

## Live acceptance completed here

Authenticated Codex CLI 0.147.0 was exercised through the source-built production composition root. Demonstrated flows include discovery/authentication, streaming, structured terminal activity, new/resumed-thread approvals with denied writes remaining absent, restart/resume, plan/default mode switching, native compaction, and cancellation. Cancellation produced a cancelled message, expired approval, cancelled activity, HTTP 409 for late allow, and no requested target file.

This is useful implementation evidence but does not close #48: it was not run from the final signed release artifact, and provider failure/recovery remains unproved.

## External release blockers

- Apple: no Developer ID Application identity, stored notary profile, or clean Apple Silicon Mac is available. The current Intel development host is not a clean acceptance machine. #47 contains the exact signing, notarization, stapling, Gatekeeper and first-run commands.
- Claude Code: no executable or authenticated account is available.
- Android: no `adb`, physical device, production signing identity or TalkBack acceptance environment is available.
- Arch/Omarchy: no representative clean physical host is available; CI container packaging is not physical acceptance.
- macOS accessibility: no signed release artifact or VoiceOver run on clean Intel and Apple Silicon hosts is available.

Do not substitute fixture providers, CI-only Android signing, ad-hoc macOS signing, simulated devices, compilation, or CI packaging for these rows.

## Next actions

1. Execute `docs/release-acceptance.md` on the credentialed release hosts and record secret-free evidence directly in #47 and #48.
2. Keep README's pre-v1/no-public-packages statement until the public release is independently verified.
3. When #47 and #48 genuinely close, run #49's immutable gate from a clean latest `main`, create `v1.0.0`, verify every downloaded public artifact/signature/checksum, then update installation docs and close #49/#42.

## Accepted non-blocking warnings

- `egui` 0.32.3 transitively uses unmaintained `ttf-parser` 0.25.1. RUSTSEC-2026-0192 reports no known vulnerability or safe upgrade; recheck before the immutable release.
- GitHub currently warns that several `actions/*@v4` actions target Node.js 20 and are being forced onto Node.js 24. The latest jobs passed; this is not release evidence failure.
