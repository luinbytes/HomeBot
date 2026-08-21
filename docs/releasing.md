# Releasing

No v1 tag is permitted until every required Linear issue is complete and each parity row is Pass on macOS Intel, macOS Apple Silicon, Arch/Omarchy, and Android, except the documented hosted-VM exclusion.

Release CI must build signed/notarised macOS artifacts, Arch/Omarchy packages and headless service assets, and the Android artifact; test clean installation and supported upgrades; produce SHA-256 checksums and checksummed release manifests bound to platform-signed artifacts; run security, migration/recovery, protocol, performance, accessibility, provider round-trip, and parity suites; then publish immutable GitHub release artifacts and known limitations.

The exact performance/resource budgets and keyboard, VoiceOver, TalkBack and text-scaling evidence required at the release gate are maintained in [performance-accessibility.md](performance-accessibility.md).

## macOS artifact pipeline

Every pull request builds complete Intel and Apple Silicon `HomeBot.app` bundles. The bundles contain the native desktop plus the headless server, are structurally and architecture checked, ad-hoc signed for CI execution, and uploaded with a reproducible tarball, notarisation ZIP, deterministic manifest, and SHA-256 checksums. See `packaging/macos/README.md`.

Ad-hoc CI artifacts are never release candidates. A release build must set `HOMEBOT_REQUIRE_RELEASE_SIGNING=1`, import the Developer ID Application identity into an ephemeral keychain, and select a stored `HOMEBOT_NOTARY_PROFILE`. `package-macos.sh` submits the ZIP, requires Apple's accepted status, staples and validates the ticket, and passes `codesign --verify --deep --strict` plus `spctl --assess --type execute` before it creates the final archive. Missing signing/notarisation credentials are an explicit release blocker.

Release manifests use schema version 1 and include product/version, platform, architecture, exact artifact name/bytes/SHA-256, signing classification, and minimum/maximum compatible protocol versions. Desktop update checks are HTTPS-only and user initiated; downloads require a second explicit approval and are staged only after exact verification. See [recovery.md](recovery.md) for the migration backup and rollback contract.

The exact credentialed, physical-platform, live-provider, evidence, and post-download commands are in [release-acceptance.md](release-acceptance.md). That runbook is the operator handoff for 6C7-66, 6C7-75, and 6C7-71; fixture results must never be entered as physical or live-provider evidence.
