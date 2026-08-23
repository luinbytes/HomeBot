# Release acceptance operator runbook

This is the deterministic handoff for GitHub Issues #47, #48, and #49 (historical Linear IDs 6C7-66, 6C7-75, and 6C7-71). Run it from an exact clean `main` commit with authenticated Codex CLI, Claude Code, Apple Developer ID/notary credentials, physical Intel and Apple Silicon Macs, an Arch/Omarchy host, and an Android device. Never paste credentials, command environments, Keychain contents, or provider transcripts into evidence.

## Record the immutable candidate

Set the candidate once and do not reuse the evidence directory for another commit:

```sh
export HOMEBOT_VERSION=1.0.0
export HOMEBOT_CANDIDATE_SHA="$(git rev-parse HEAD)"
test -z "$(git status --porcelain)"
test "$(git branch --show-current)" = main
mkdir -p "release-evidence/$HOMEBOT_CANDIDATE_SHA"
git show -s --format='%H%n%T%n%an <%ae>%n%aI' > "release-evidence/$HOMEBOT_CANDIDATE_SHA/source.txt"
./scripts/check.sh 2>&1 | tee "release-evidence/$HOMEBOT_CANDIDATE_SHA/check.log"
```

The source identity must be `luinbytes <42706009+luinbytes@users.noreply.github.com>`. Record `sw_vers`, `uname -m`, Android OS/build and device model, Arch kernel/package versions, `codex --version`, and `claude --version`; do not record authentication tokens.

## Sign, notarise, and staple macOS artifacts

On each architecture, import the Developer ID Application certificate into an ephemeral keychain using the organisation's credential procedure. Store notary credentials without placing values in shell history:

```sh
xcrun notarytool store-credentials homebot-v1-notary
export HOMEBOT_SIGN_IDENTITY='Developer ID Application: ORGANISATION (TEAMID)'
export HOMEBOT_NOTARY_PROFILE=homebot-v1-notary
export HOMEBOT_REQUIRE_RELEASE_SIGNING=1
export HOMEBOT_TARGET="$(case "$(uname -m)" in x86_64) echo x86_64-apple-darwin;; arm64) echo aarch64-apple-darwin;; *) exit 2;; esac)"
export HOMEBOT_OUTPUT_DIR="$PWD/dist"
cargo build --release --target "$HOMEBOT_TARGET" -p homebot-server -p homebot-desktop
./scripts/package-macos.sh
```

Expected files for each `ARCH` (`x86_64` or `arm64`) are:

```text
HomeBot-1.0.0-macos-ARCH.tar.gz
HomeBot-1.0.0-macos-ARCH-notarization.zip
HomeBot-1.0.0-macos-ARCH.manifest.json
HomeBot-1.0.0-macos-ARCH.notarization.json
HomeBot-1.0.0-macos-ARCH.SHA256SUMS
```

Verify without trusting the build log:

```sh
(cd dist && shasum -a 256 -c "HomeBot-$HOMEBOT_VERSION-macos-ARCH.SHA256SUMS")
tar -xzf "dist/HomeBot-$HOMEBOT_VERSION-macos-ARCH.tar.gz" -C /tmp/homebot-release-check
codesign --verify --deep --strict --verbose=2 /tmp/homebot-release-check/HomeBot.app
xcrun stapler validate /tmp/homebot-release-check/HomeBot.app
spctl --assess --type execute --verbose=2 /tmp/homebot-release-check/HomeBot.app
python3 -m json.tool "dist/HomeBot-$HOMEBOT_VERSION-macos-ARCH.manifest.json" >/dev/null
python3 -m json.tool "dist/HomeBot-$HOMEBOT_VERSION-macos-ARCH.notarization.json" >/dev/null
```

Record artifact SHA-256 values, notary request ID/status, `codesign -dv --verbose=4` authority/team/identifier lines, `stapler validate`, and Gatekeeper outcome. Remove certificate material and the ephemeral keychain according to the organisation procedure after both builds.

## Live provider smoke matrix

Start the exact packaged server with an owner token file and a fresh database. Confirm `/health`, create one Bot against each projected profile, create a direct chat, and send `Reply with exactly HOMEBOT_PROVIDER_OK`. For Codex and Claude independently record pass/fail for auth discovery, streamed response, activity/tool event, approval decision, cancellation, server restart/resume, plan mode, and context compaction where the provider advertises the capability. Use the authenticated public API shapes documented in `docs/protocol.md`; redact transcript content other than the fixed marker. A missing advertised capability is `N/A (not advertised)`, not Pass.

Evidence must include OS/architecture, package hash, CLI version, HomeBot profile ID, start/end UTC timestamps, advertised capabilities, normalized HomeBot terminal status, and the relevant GitHub #48/parity-matrix row. It must not include the owner token, provider credential, pairing token, environment dump, or provider-native payload.

## Physical platform matrix

Build the Android candidate with the production keystore supplied only through local environment references:

```sh
(cd android && ./gradlew clean assembleRelease \
  -PhomebotVersionName="$HOMEBOT_VERSION")
export HOMEBOT_ANDROID_KEYSTORE=/absolute/path/to/release.keystore
export HOMEBOT_ANDROID_KEY_ALIAS=homebot
export HOMEBOT_ANDROID_STORE_PASSWORD_NAME=HOMEBOT_ANDROID_STORE_PASSWORD
export HOMEBOT_ANDROID_KEY_PASSWORD_NAME=HOMEBOT_ANDROID_KEY_PASSWORD
apksigner sign --ks "$HOMEBOT_ANDROID_KEYSTORE" --ks-key-alias "$HOMEBOT_ANDROID_KEY_ALIAS" \
  --ks-pass "env:$HOMEBOT_ANDROID_STORE_PASSWORD_NAME" \
  --key-pass "env:$HOMEBOT_ANDROID_KEY_PASSWORD_NAME" \
  --out /tmp/HomeBot-release-signed.apk android/app/build/outputs/apk/release/app-release-unsigned.apk
HOMEBOT_ANDROID_SIGNING=android-release \
  scripts/package-android.sh /tmp/HomeBot-release-signed.apk dist
(cd dist && sha256sum -c "HomeBot-$HOMEBOT_VERSION-android.SHA256SUMS")
```

Expected files are `HomeBot-1.0.0-android.apk`, `HomeBot-1.0.0-android.manifest.json`, `HomeBot-1.0.0-android.signature.json`, and `HomeBot-1.0.0-android.SHA256SUMS`. Record the final certificate SHA-256 digest from the signature evidence. Delete the temporary signed input after the release assets are safely staged; do not delete or export the production keystore.

Install from the candidate artifact on clean Intel macOS, Apple Silicon macOS, Arch/Omarchy, and Android. On each platform verify first launch, server discovery, upgrade from the latest supported pre-v1 fixture, Bot create/edit/archive/restore, direct and group chat, streaming, approval, attachment, queue/steer/cancel, routine run/history, Skill/plugin state, remote pairing/revocation, reconnect, workspace/checkpoint/diff/restore, and source-control read surfaces. Confirm uninstall preserves user data by default.

On both Macs run the complete flow with keyboard-only navigation and VoiceOver. On Android run it with TalkBack, system text scaling, notification permission denied and allowed, background reconnect, and exact deep links. Record device/OS versions, artifact hash, UTC timestamps, and pass/fail per parity row. Screenshots must exclude secrets and private transcript/repository content.

## Final publication

Only after every matrix row is Pass or the documented hosted-VM exclusion:

```sh
(cd dist && shasum -a 256 -c ./*SHA256SUMS)
git tag -s v1.0.0 "$HOMEBOT_CANDIDATE_SHA"
git push origin v1.0.0
```

Upload the immutable macOS Intel/Apple Silicon archives, Arch package and service assets, Android APK, manifests, notarisation evidence, and checksum files to GitHub release `v1.0.0`. Download each public asset into a new directory and re-run its checksum verification. Then update GitHub Issues #47, #48, and #49 with the secret-free evidence locations and hashes before closing #42.
