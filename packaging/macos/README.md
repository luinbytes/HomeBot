# macOS packaging

`scripts/package-macos.sh` assembles the native Rust desktop and headless server into a HomeBot application bundle. It validates the bundle architecture and signature, emits a reproducible `tar.gz`, creates an Apple-compatible notarisation ZIP, and writes a machine-readable manifest plus SHA-256 checksums.

CI builds and structurally verifies both `x86_64-apple-darwin` and `aarch64-apple-darwin` artifacts. Pull requests and ordinary `main` builds use an explicit ad-hoc signature so the complete bundle is testable without exposing signing credentials.

For an immutable release, import a Developer ID Application certificate into a temporary keychain, set `HOMEBOT_SIGN_IDENTITY` to its exact identity, and set `HOMEBOT_REQUIRE_RELEASE_SIGNING=1`. Submit the generated `*-notarization.zip` with `xcrun notarytool submit --wait`, staple the accepted ticket to `HomeBot.app`, verify with `spctl --assess --type execute`, then regenerate the final distribution archive and manifest. Release CI must fail closed if the certificate or notarisation credentials are missing.

HomeBot stores macOS application data beneath `~/Library/Application Support/HomeBot`. The packaged desktop supervises the same loopback-only Rust server used remotely; `Contents/Resources/bin/homebot-server` is included for headless diagnostics and later service packaging. Provider discovery retains its environment allow-list and additionally checks `/opt/homebrew/bin` and `/usr/local/bin`, covering standard Apple Silicon and Intel Homebrew/npm CLI installations without evaluating shell profiles.
