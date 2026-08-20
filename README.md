# HomeBot

## Your AI team. On your computer.

HomeBot is an open-source home for persistent AI teammates. Create Bots, message them like people, put specialists in a group chat, and let them hand work to each other while your own Mac or Linux machine remains the computer they work on.

Bring the providers you already use, including Codex CLI, Claude Code, and OpenAI-compatible APIs. Attach a repository when a Bot needs coding powers. Turn a good workflow into a routine. Check in from the native desktop app or Android over your LAN or Tailscale. Your chats, files, credentials, and execution history stay under your control.

> HomeBot is under active development and is not yet a v1 release. The server currently exposes only an unauthenticated loopback health endpoint. Do not expose development builds to a network.

### What HomeBot is building

- Persistent, customisable Bots with durable chats and memory
- Direct chats and group chats with mentions, parallel work, and visible handoffs
- Bring-your-own Codex, Claude Code, or OpenAI-compatible backend
- Native Rust and egui desktop apps for Intel/Apple Silicon macOS and Linux
- A first-class native Android client with secure QR pairing
- Headless operation and Tailscale-friendly remote access
- Skills, plugins/MCP connections, secure secrets, and scheduled routines
- Optional repository workspaces, worktrees, exact turn diffs, checkpoints, and safe revert
- Server-enforced permissions and structured approvals

The hosted cloud VM is the one intentional Grok Bot parity exclusion. Your HomeBot host replaces it.

## Status and installation

HomeBot is in active M2 desktop-parity development. There are no supported release packages yet. Follow [INSTALL.md](INSTALL.md) for human installation status or [AGENT_INSTALL.md](AGENT_INSTALL.md) for deterministic automation instructions. Release readiness is tracked against [the parity matrix](docs/parity-matrix.md).

Supported v1 targets are macOS x86_64 and arm64, Linux x86_64 with Arch/Omarchy first-class, headless macOS/Linux, and Android.

## Architecture

The Rust server is authoritative. It owns Bot/chat state, providers, permissions, tools, routines, plugins, secrets, browser and terminal execution, Git operations, and persistence. Desktop and Android are clients of the same versioned HTTP and WebSocket protocol. The desktop app may supervise a bundled local server, but never bypasses its contracts.

| Area | Responsibility |
| --- | --- |
| `homebot-domain` | Provider-independent Bots, chats, activities, approvals, and routines |
| `homebot-protocol` | Versioned client/server messages and compatibility rules |
| `homebot-storage` | SQLite migrations, repositories, outbox, and recovery |
| `homebot-server` | Authenticated HTTP/WebSocket API and headless process |
| `homebot-providers` | Codex, Claude Code, BYOK, and community adapter boundary |
| `homebot-tools` | Server-enforced filesystem, PTY, browser, plugin, and secret capabilities |
| `homebot-vcs` | Workspaces, worktrees, checkpoints, diffs, and safe source control |
| `homebot-desktop` | Native egui client and local-server supervision |
| `android/` | Kotlin/Compose client and Android Keystore device session |

Bots own stable HomeBot identity and app-managed history. A provider profile maps a Bot/chat to backend-specific conversations; switching providers does not replace the Bot.

Large artifacts live in a content-addressed application data directory. SQLite is the durable source of truth for application state and an outbox provides sequenced, resumable events. Secret values are never stored in normal SQLite rows; the database holds opaque references to OS-backed credential storage.

Remote clients pair with a short-lived, single-use credential and receive a named, revocable device session. HomeBot binds to loopback by default. Private-network access such as Tailscale is the recommended remote path.

Read the deeper contracts:

- [Architecture](docs/architecture.md)
- [Desktop visual system](docs/desktop-visual-system.md)
- [Desktop settings and notifications](docs/desktop-settings.md)
- [Protocol](docs/protocol.md)
- [Group chats and coordination](docs/group-chats.md)
- [Activity and artifact surfaces](docs/activity-surfaces.md)
- [Providers](docs/providers.md)
- [Local computer capabilities](docs/tools.md)
- [Security and threat model](docs/security.md)
- [Android](docs/android.md)
- [Routines](docs/routines.md)
- [Plugins](docs/plugins.md)
- [Development](docs/development.md)
- [Releasing](docs/releasing.md)

## Development

Prerequisites: Rust stable, Git, and platform build dependencies. Android work additionally requires JDK 17 and the Android SDK.

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo run -p homebot-server
curl --fail --silent http://127.0.0.1:7123/health
```

Protocol models are Rust-owned. Every protocol change must update the machine-readable schema, golden fixtures, compatibility notes, and Android mechanical validation before merge.

Provider implementations conform to `ProviderAdapter`. Provider-specific process or API events are normalised before crossing the client boundary. See [providers.md](docs/providers.md) before adding an adapter.

HomeBot is MIT licensed. T3 Code is architectural inspiration and is also MIT licensed; HomeBot does not copy proprietary Grok Bot code or assets. See [CONTRIBUTING.md](CONTRIBUTING.md) and [SECURITY.md](SECURITY.md).
