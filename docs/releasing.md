# Releasing

No v1 tag is permitted until every required Linear issue is complete and each parity row is Pass on macOS Intel, macOS Apple Silicon, Arch/Omarchy, and Android, except the documented hosted-VM exclusion.

Release CI must build signed/notarised macOS artifacts, Arch/Omarchy packages and headless service assets, and the Android artifact; test clean installation and supported upgrades; produce SHA-256 checksums and a signed release manifest; run security, migration/recovery, protocol, performance, accessibility, provider round-trip, and parity suites; then publish immutable GitHub release artifacts and known limitations.

## macOS artifact pipeline

Every pull request builds complete Intel and Apple Silicon `HomeBot.app` bundles. The bundles contain the native desktop plus the headless server, are structurally and architecture checked, ad-hoc signed for CI execution, and uploaded with a reproducible tarball, notarisation ZIP, deterministic manifest, and SHA-256 checksums. See `packaging/macos/README.md`.

Ad-hoc CI artifacts are never release candidates. A release tag must set `HOMEBOT_REQUIRE_RELEASE_SIGNING=1`, import the Developer ID Application identity into an ephemeral keychain, submit the ZIP to Apple, staple the accepted ticket, and pass `codesign --verify --deep --strict` plus `spctl --assess --type execute`. Missing signing/notarisation credentials are an explicit release blocker.
