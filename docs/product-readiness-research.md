# HomeBot product-readiness research

Research date: 2026-08-25. This note turns first-party platform guidance into
requirements for HomeBot as a daily-use, self-hosted assistant on macOS/Linux
with an Android client. It complements the repository contracts in
[README](../README.md), [Android architecture](android.md),
[remote access](remote-access.md), [performance/accessibility](performance-accessibility.md),
and [release acceptance](release-acceptance.md). External sources are limited
to Apple, Android/Google, systemd, freedesktop.org, Tailscale, Arch Linux,
W3C, NIST, and OWASP documentation.

## Bottom line

The server/client architecture is the right foundation: the Rust server is the
single source of truth, clients use an authenticated versioned protocol, and
the current defaults are conservative. The remaining product risks are mostly
operational rather than another feature surface:

1. Replace the macOS UI's direct `~/Library/LaunchAgents` file write with a
   user-approved, status-aware Service Management registration path.
2. Make Linux headless continuity explicit: a user service needs a deliberate
   `systemd` lingering choice (or a documented system service) to survive
   logout; test the real packaged upgrade/restart path.
3. Choose the Android promise. A WebSocket is excellent while the process is
   alive, but Android may stop the process. Either ship a privacy-reviewed push
   relay for completion/approval notifications or clearly label Android as
   reconnect-on-open in v1; do not add an always-on foreground service or a
   polling loop.
4. Add Android TTID/TTFD and frame/jank budgets to the existing cross-platform
   gate, and measure p50/p95/p99 on release builds.
5. Make HTTPS the normal LAN/Tailscale pairing path, retain plaintext only as a
   visibly acknowledged development escape hatch, and move sensitive pairing
   links toward verified HTTPS App Links where a web domain exists.
6. Close physical VoiceOver/TalkBack, clean-install/upgrade, release-signing,
   live-provider, and data-retention evidence before calling the product sold
   or supported.

## Platform requirements and current fit

### macOS: background operation and distribution

Apple's current Service Management API supports bundled Login Items,
LaunchAgents, and LaunchDaemons through `SMAppService`; registration is subject
to user approval, exposes authorization status, and can open System Settings
for the user to change that status. A registered Login Item starts now and at
subsequent logins; an approved LaunchDaemon is bootstrapped at boot.
([SMAppService](https://developer.apple.com/documentation/servicemanagement/smappservice),
 [register](https://developer.apple.com/documentation/servicemanagement/smappservice/register%28%29))

HomeBot currently writes a raw plist from
`crates/homebot-desktop/src/app.rs::set_launch_at_login`. On the supported
macOS 13+ baseline this is weaker than the platform flow: it cannot present or
reconcile the user's authorization state. Use a user-level LaunchAgent/helper
for the bundled server or app, expose enabled/disabled/denied/error states,
and keep the setting reversible. Do not use a root LaunchDaemon for ordinary
personal data or provider access; reserve it for a separately justified
system-wide service.

For direct distribution, Apple expects Developer ID signing and notarization
so Gatekeeper can establish that the app is genuine and unaltered. Notarization
requires valid signatures on distributed executables, a secure timestamp, the
Hardened Runtime, and review of the notary result; staple the ticket and test
Gatekeeper behavior on clean Intel and Apple Silicon Macs.
([macOS distribution](https://developer.apple.com/macos/distribution/),
 [notarization](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution),
 [Hardened Runtime](https://developer.apple.com/documentation/security/hardened-runtime))

The existing scripts and runbook already model this correctly, but the README
still says there are no supported packages. Treat ad-hoc CI artifacts as test
artifacts only. The supported release must carry architecture-specific hashes,
notarization evidence, a rollback path, and a clean first-run/upgrade check.
App Store distribution should remain a separate decision: Apple requires App
Sandbox there, while HomeBot's local repository, terminal, and browser
capabilities need an explicit sandbox/entitlement design. Direct notarized
distribution is the simpler current product path.

### Linux: service and desktop distribution

The upstream `systemd.service` contract recommends `Restart=on-failure` for
long-running services and supports bounded restart delay/rate limiting and an
optional watchdog. Its credential design supports encrypted credentials that
are decrypted only when a service is activated; sensitive values should not be
literal unit-file settings.
([systemd.service](https://github.com/systemd/systemd/blob/main/man/systemd.service.xml),
 [systemd credentials](https://github.com/systemd/systemd/blob/main/docs/CREDENTIALS.md),
 [systemd user lingering](https://github.com/systemd/systemd/blob/main/man/org.freedesktop.login1.xml))

HomeBot's Arch unit already uses a user service, `Restart=on-failure`,
`LoadCredentialEncrypted`, `NoNewPrivileges`, `PrivateTmp`, and a restrictive
umask. Keep that shape. Add a documented health/restart diagnostic and test
that a crash, host reboot, network restoration, and package upgrade preserve
the database and recover the server. A systemd *user* manager normally follows
the user's session; headless Android connectivity therefore requires an
explicit, reversible lingering choice or a documented system-service mode.
Do not silently enable either one.

The freedesktop Desktop Entry specification defines the portable launcher
contract: UTF-8 `.desktop` files, reverse-DNS IDs, required `Type` and `Name`,
correctly quoted `Exec`, icon/category metadata, and startup notification where
supported. HomeBot's entry is structurally aligned. Validate it and the icon
on GNOME/KDE/Wayland/X11 release hosts. Add AppStream metainfo (summary,
description, screenshots, license, supported hardware and release links) if
the package is expected to appear professionally in software centers.
([Desktop Entry Specification](https://specifications.freedesktop.org/desktop-entry/latest/),
 [AppStream specification](https://www.freedesktop.org/software/appstream/docs/))

Arch's package guidance expects direct dependencies, correct upstream version
and package-release numbering, source integrity checks, and encourages
reproducibility checks. Keep the current no-`SKIP` source checksum, add package
signature/provenance where the distribution channel supports it, and publish
the exact supported distro/kernel/desktop matrix rather than calling one Arch
package “Linux support.”
([Arch package guidelines](https://wiki.archlinux.org/title/Arch_package_guidelines),
 [reproducible-builds definition](https://reproducible-builds.org/docs/definition/))

### Android: network, lifecycle, security, and UX

Android 9/API 28 and later disable cleartext by default; Network Security
Configuration can enforce HTTPS, constrain trust anchors, and provide narrow
debug-only exceptions. Keep HomeBot's `cleartextTrafficPermitted=false` base
config and loopback-only exceptions; never broaden it to LAN/Tailscale in the
release build.
([Network Security Configuration](https://developer.android.com/privacy-and-security/security-config),
 [cleartext communications](https://developer.android.com/privacy-and-security/risks/cleartext-communications))

Android can stop background processes and limits background services. Official
guidance says to defer work until foreground, use WorkManager for scheduled
work, use a foreground service only for work the user can actively notice, and
use FCM to selectively wake for network events rather than polling. HomeBot's
current “WebSocket while alive, reconnect on open, no permanent polling/FGS”
choice is platform-correct. It is not, by itself, a promise of timely
completion/approval notifications after process death. A future push design
must send minimal opaque event IDs, rehydrate over authenticated HTTPS, honor
notification permission, and keep the self-hosted server as authority.
([background execution limits](https://developer.android.com/about/versions/oreo/background),
 [services](https://developer.android.com/develop/background-work/services),
 [FCM priority](https://firebase.google.com/docs/cloud-messaging/android-message-priority))

Android Keystore keeps key material non-exportable and can restrict its allowed
cryptographic uses. HomeBot's AES-GCM session envelope and redacted logs match
that model. Continue to store only non-secret endpoint/preferences outside the
Keystore and never put provider credentials, pairing material, or transcript
secrets in notifications, deep-link URLs, analytics, or crash logs.
([Android Keystore](https://developer.android.com/privacy-and-security/keystore))

Compose guidance requires semantic roles/state/content descriptions for
custom controls and a 48dp minimum interactive target. Android 13/API 33+
requires runtime `POST_NOTIFICATIONS`; a denial must leave the app usable with
in-app unread/attention state. Use scalable text, adaptive layouts, and
TalkBack-tested notification/deep-link destinations. Verified HTTPS App Links
use Digital Asset Links to prevent other apps from intercepting a domain's
links; keep the custom `homebot://` scheme for compatibility only, and prefer
verified HTTPS pairing links when a domain exists. If custom-scheme pairing
remains, require a visible confirmation and preserve the current short-lived,
single-use credential/proof design.
([Compose accessibility defaults](https://developer.android.com/develop/ui/compose/accessibility/api-defaults),
 [Compose semantics](https://developer.android.com/develop/ui/compose/accessibility/semantics),
 [notification permission](https://developer.android.com/develop/ui/compose/notifications/notification-permission),
 [verified App Links](https://developer.android.com/training/app-links/about))

Android's architecture guidance assumes components can be destroyed at any
time and recommends persistent models, a single source of truth, and a UI
driven by data. HomeBot can remain server-authoritative, but must choose one
user-facing contract: either add a bounded, encrypted, read-only last-safe
projection for offline viewing, or state clearly that history requires a
reachable host. Do not add a second mutable cache without an offline mutation
contract.
([Android architecture](https://developer.android.com/topic/architecture),
 [offline-first guidance](https://developer.android.com/topic/architecture/data-layer/offline-first))

## Speed: make “fast” measurable

Google's current Android guidance separates time to initial display (TTID) from
time to full display (TTFD), asks teams to keep p95/p99 close to the median,
and gives an aspirational target of cold <500 ms, warm <200 ms, and hot <150 ms.
Android Vitals treats cold >=5 s, warm >=2 s, and hot >=1.5 s as excessive.
Smooth 60 Hz rendering needs frames under 16 ms (about 11 ms at 90 Hz and 8 ms
at 120 Hz). Apple XCTest exposes repeatable launch, wall-clock, CPU, memory,
hitch, and signpost metrics.
([Android performance measurement](https://developer.android.com/topic/performance/measuring-performance),
 [Android startup](https://developer.android.com/topic/performance/vitals/launch-time),
 [Android rendering](https://developer.android.com/topic/performance/vitals/render),
 [Apple XCTest performance metrics](https://developer.apple.com/documentation/xctest/performance-tests))

Apply these metrics to real HomeBot journeys, not provider response time:

| Journey | Release evidence |
| --- | --- |
| Desktop/server cold start | first visible shell, authenticated usable projection, then composer-ready; p50/p95/p99 and CPU/RSS |
| Android cold/warm/hot launch | TTID and TTFD on a current mid-range physical device, release build, offline and reachable-host cases |
| Chat open/reconnect | cached projection first, then snapshot/replay completion; p50/p95/p99 and no main-thread blocking |
| Streaming | event-to-projection latency and frame time under a realistic long chat/activity feed |
| Background completion | notification arrival, deep-link open, authoritative hydration, and duplicate/replay behavior |

Keep the existing HomeBot budgets as blockers, but add Android TTID/TTFD,
frame-time and p95/p99 thresholds. The first screen should render from a safe
projection before provider/network work completes, show an honest connection
state, and stream progress incrementally. Measure release builds with
Macrobenchmark/Perfetto (Android) and XCTest/signposts/Instruments (macOS);
do not infer everyday speed from unit tests or a provider's token latency.

## Secure local and remote connectivity

HomeBot's loopback default, explicit remote opt-in, short-lived single-use
pairing, hashed server-side material, revocable device sessions, and
server-side capability checks are the correct trust boundaries. Preserve them.

Tailscale says tailnet traffic is end-to-end encrypted and ACL/grant policy
still controls access to services. It also recommends HTTPS for web/API
services because clients and browsers treat HTTP as insecure and HTTP services
can be exposed to DNS-rebinding risks. `tailscale serve --https` terminates
TLS with a tailnet certificate while proxying to a loopback HomeBot server.
Prefer this documented path for remote Android pairing; never make Funnel or a
public listener the default.
([Tailscale Serve](https://tailscale.com/docs/features/tailscale-serve),
 [Tailscale access control](https://tailscale.com/docs/features/access-control),
 [Tailscale security guidance](https://tailscale.com/docs/reference/best-practices/security))

For direct LAN use, require HTTPS with a user-understandable certificate/trust
flow. Keep plain private-network HTTP only as an explicit, warning-bearing
development escape hatch. Bind narrowly to the selected LAN/Tailscale address,
avoid `0.0.0.0`, validate endpoint origin/shape, disable credential-bearing
redirects, and prove revocation plus reconnect behavior on a real phone and
host.

## Accessibility, privacy, and reliability

Apple's HIG asks for intuitive, perceivable, adaptable interfaces and recommends
Accessibility Inspector audits plus VoiceOver testing; Apple's VoiceOver
criteria require every visible/interactive element to be navigable, labeled,
and usable for common tasks without sighted assistance. Use those checks on
every macOS workflow, including pairing, approvals, settings, notifications,
and failure recovery. Treat WCAG 2.2 AA as a useful cross-client baseline for
contrast, names, keyboard access, focus, and error state even though the native
clients are not web pages.
([Apple accessibility HIG](https://developer.apple.com/design/human-interface-guidelines/accessibility/),
 [Apple accessibility audits](https://developer.apple.com/documentation/accessibility/performing-accessibility-audits-for-your-app),
 [VoiceOver criteria](https://developer.apple.com/help/app-store-connect/manage-app-accessibility/voiceover-evaluation-criteria),
 [WCAG 2.2](https://www.w3.org/TR/WCAG22/))

The current local-only bounded telemetry, OS-backed secret storage, redaction,
server authority, and no-provider-payload client boundary are strong. A
professional product still needs a plain-language data-flow/retention policy:
what remains on the host, what reaches a configured provider, what Android
caches, how backups/deletion work, and how a user revokes devices. NIST frames
privacy as risk management; OWASP MASVS makes storage, crypto, auth, network,
platform, resilience, and privacy explicit mobile verification areas.
([NIST Privacy Framework](https://www.nist.gov/privacy-framework/privacy-framework),
 [OWASP MASVS](https://mas.owasp.org/MASVS/))

Reliability evidence should include host reboot, process crash, SQLite migration
and backup/restore, network loss/recovery, stale cursor, duplicate events,
provider timeout/cancellation, notification denial, device revocation, and
upgrade rollback. Each failure needs a visible bounded state and a recovery
action; “green CI” is not physical-device or live-provider proof.

## Professional release bar and execution order

For macOS, ship only Developer ID-signed, notarized, stapled artifacts with
architecture manifests and independently verified hashes. For Android, ship a
non-debuggable, optimized, digitally signed release artifact; if using Google
Play, use an App Bundle/Play App Signing and meet the current target API rule
(new apps and updates must target API 36 from 2026-08-31); direct APK
distribution still requires signed artifacts and a safe update story.
([Android publishing](https://developer.android.com/studio/publish/),
 [Android app signing](https://developer.android.com/studio/publish/app-signing),
 [target API requirement](https://developer.android.com/google/play/requirements/target-sdk))

Before public sale/support, run the existing exact clean-install, upgrade,
provider, physical-device, accessibility, performance, pairing, and release
matrix; publish known limitations and recovery instructions. The shortest
credible implementation order is:

1. macOS Service Management registration/status and headless launch behavior.
2. Linux lingering/headless choice, package metadata, and service recovery.
3. Android killed-process promise (push relay versus explicit online-only
   contract), verified pairing links, and bounded read-only offline state.
4. Android/macOS journey telemetry and physical performance/accessibility runs.
5. Signed/notarized/reproducible artifacts, privacy documentation, upgrade/
   rollback evidence, and live-provider acceptance.
