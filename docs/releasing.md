# Releasing

No v1 tag is permitted until every GitHub release-blocker issue is complete and each parity row is Pass on macOS Intel, macOS Apple Silicon, Arch/Omarchy, and Android, except the documented hosted-VM exclusion.

Release CI must build signed/notarised macOS artifacts, Arch/Omarchy packages and headless service assets, and the Android artifact; test clean installation and supported upgrades; produce SHA-256 checksums and checksummed release manifests bound to platform-signed artifacts; run security, migration/recovery, protocol, performance, accessibility, provider round-trip, and parity suites; then publish immutable GitHub release artifacts and known limitations.

The exact performance/resource budgets and keyboard, VoiceOver, TalkBack and text-scaling evidence required at the release gate are maintained in [performance-accessibility.md](performance-accessibility.md).

## macOS artifact pipeline

Every pull request builds complete Intel and Apple Silicon `HomeBot.app` bundles. The bundles contain the native desktop plus the headless server, are structurally and architecture checked, ad-hoc signed for CI execution, and uploaded with a reproducible tarball, notarisation ZIP, deterministic manifest, and SHA-256 checksums. See `packaging/macos/README.md`.

Ad-hoc CI artifacts are never release candidates. A release build must set `HOMEBOT_REQUIRE_RELEASE_SIGNING=1`, import the Developer ID Application identity into an ephemeral keychain, and select a stored `HOMEBOT_NOTARY_PROFILE`. `package-macos.sh` submits the ZIP, requires Apple's accepted status, staples and validates the ticket, and passes `codesign --verify --deep --strict` plus `spctl --assess --type execute` before it creates the final archive. Missing signing/notarisation credentials are an explicit release blocker.

Artifact manifests use schema version 1 and include product/version, platform, architecture, exact artifact name/bytes/SHA-256, signing classification, and minimum/maximum compatible protocol versions. A desktop update-channel manifest is schema version 2: it contains the same canonical fields plus a public-key fingerprint and an Ed25519 signature. The desktop verifies that signature with a public key pinned at compile time before trusting the artifact URL or checksum. HTTPS remains defense in depth; a compromised release channel cannot replace both the artifact and its digest.

Generate the offline update key once on a controlled release host and keep the private PEM outside the repository:

```sh
install -d -m 700 "$HOME/.config/homebot"
openssl genpkey -algorithm ED25519 -out "$HOME/.config/homebot/update-signing-private.pem"
chmod 600 "$HOME/.config/homebot/update-signing-private.pem"
HOMEBOT_UPDATE_PUBLIC_KEY_HEX="$(openssl pkey -in "$HOME/.config/homebot/update-signing-private.pem" -pubout -outform DER | tail -c 32 | xxd -p -c 256)" cargo build --release -p homebot-desktop
./scripts/sign-update-manifest.py --input packaging-output/HomeBot-release.json --output packaging-output/HomeBot-update.json --private-key "$HOME/.config/homebot/update-signing-private.pem"
```

Never print, commit, upload as an artifact, or pass the private key through a command-line value. The public hex value is non-secret and must be identical for every v1 desktop build. A build without `HOMEBOT_UPDATE_PUBLIC_KEY_HEX` fails update-manifest verification rather than accepting a development key. Update checks are user initiated; downloads require a second explicit approval and are staged only after signature plus exact size/SHA-256 verification. See [recovery.md](recovery.md) for the migration backup and rollback contract.

The exact credentialed, physical-platform, live-provider, evidence, and post-download commands are in [release-acceptance.md](release-acceptance.md). That runbook is the operator handoff for GitHub Issues #47, #48, and #49; fixture results must never be entered as physical or live-provider evidence.
