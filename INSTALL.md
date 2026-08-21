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

The native Android application is still under construction. The server and desktop already implement secure pairing: the desktop Devices screen creates a five-minute, single-use QR/deep-link credential, which is exchanged for a named and revocable device session. Permanent credentials never appear in the QR code. See [Remote access and device pairing](docs/remote-access.md) for endpoint and revocation rules.

## Headless and remote access

The server binds to `127.0.0.1:7123` by default. LAN or Tailscale listening requires both an explicit `HOMEBOT_BIND` IP socket and `HOMEBOT_ALLOW_REMOTE=1`; public/custom endpoints require an HTTPS reverse proxy. Follow [Remote access and device pairing](docs/remote-access.md). Packaged headless services remain release work.
