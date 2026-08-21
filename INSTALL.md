# Install HomeBot

HomeBot is not released yet. The commands below build the current development server. CI now assembles structurally verified ad-hoc-signed Intel and Apple Silicon macOS bundles and an Android debug APK, but these are test artifacts rather than supported installations. Public installation instructions will be activated only after signed/notarised release artifacts and clean-machine checks pass.

## Build from source

Install Git and the current stable Rust toolchain, then:

```bash
git clone https://github.com/luinbytes/HomeBot.git
cd HomeBot
cargo build --workspace --release
cargo test --workspace --all-features
./target/release/homebot-server
```

In another terminal, verify `curl --fail --silent http://127.0.0.1:7123/health`. A healthy development server returns JSON containing `"status":"ok"`, `"service":"homebot-server"`, and the supported protocol version.

## macOS

CI produces separate `HomeBot-<version>-macos-x86_64` and `HomeBot-<version>-macos-arm64` test artifacts with manifests and SHA-256 checksums. They are ad-hoc signed and are not public v1 packages. The release gate still requires Developer ID signing, Apple notarisation, Gatekeeper assessment, and clean Intel/Apple Silicon first-run checks. Packaged provider discovery checks standard Intel and Apple Silicon Homebrew locations without evaluating shell startup files. Do not download unofficial files claiming to be a HomeBot v1 release.

## Arch Linux and Omarchy

v1 will provide an AUR-ready `PKGBUILD`, desktop entry, Wayland/X11 guidance, and a systemd user service for headless mode. Graphical and systemd launches frequently have a narrower `PATH` than an interactive shell; HomeBot will support explicit provider binary paths.

## Android and pairing

The native Android application and authoritative protocol client build in CI, but no public release APK exists yet. The desktop Devices screen creates a five-minute, single-use QR/deep-link credential, which is exchanged for a named and revocable device session. Permanent credentials never appear in the QR code. See [Remote access and device pairing](docs/remote-access.md) for endpoint and revocation rules.

## Headless and remote access

The server binds to `127.0.0.1:7123` by default. LAN or Tailscale listening requires both an explicit `HOMEBOT_BIND` IP socket and `HOMEBOT_ALLOW_REMOTE=1`; public/custom endpoints require an HTTPS reverse proxy. Follow [Remote access and device pairing](docs/remote-access.md). Packaged headless services remain release work.
