# Install HomeBot

HomeBot is not released yet. The commands below build the current development server; packaged macOS, Arch/Omarchy, and Android installation instructions will be activated only after those artifacts are tested. Development builds expose only a loopback health endpoint and are not a usable product.

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

v1 will provide separate signed and notarised downloads for Intel x86_64 and Apple Silicon arm64, with first-launch permission guidance and provider CLI discovery. Those packages do not exist yet. Do not download unofficial files claiming to be a HomeBot v1 release.

## Arch Linux and Omarchy

v1 will provide an AUR-ready `PKGBUILD`, desktop entry, Wayland/X11 guidance, and a systemd user service for headless mode. Graphical and systemd launches frequently have a narrower `PATH` than an interactive shell; HomeBot will support explicit provider binary paths.

## Android and pairing

The Android application and pairing flow are not implemented yet. v1 pairing will use a short-lived, single-use QR/deep-link credential exchanged for a named, revocable device session. Permanent API keys will not appear in QR codes.

## Headless and remote access

The development server currently binds only to `127.0.0.1:7123`. Authentication, LAN/Tailscale binding, pairing, HTTPS configuration, and the headless service are release blockers. Do not proxy or expose the current endpoint.
