# HomeBot deterministic agent installation

Status: pre-release development bootstrap. An agent MUST NOT report HomeBot installed for end-user use until a signed release route and the complete verification section are available. Current success means only that the development workspace builds, tests, starts on loopback, and returns its health contract.

## 1. Detect the machine

Run without printing environment variables or secret files:

```bash
uname -s
uname -m
command -v sw_vers >/dev/null && sw_vers -productVersion || true
test -r /etc/os-release && sed -n 's/^\(ID\|ID_LIKE\|VERSION_ID\)=/\1=/p' /etc/os-release || true
command -v pacman || command -v apt-get || command -v dnf || command -v zypper || command -v brew || true
printf '%s\n' "$SHELL"
command -v git || true
command -v rustup || true
command -v rustc || true
command -v cargo || true
command -v codex || true
command -v claude || true
command -v tailscale || true
printf '%s\n' "${XDG_SESSION_TYPE:-unset}"
```

Never run `env`, `set`, credential-dumping commands, `codex auth` inspection that reveals tokens, or commands that print API keys.

## 2. Installation decision tree

```text
IF a verified HomeBot release newer than this document exists:
  IF macOS arm64: use the signed arm64 package
  ELSE IF macOS x86_64: use the signed x86_64 package
  ELSE IF Arch/Omarchy x86_64: use the verified package/PKGBUILD route
  ELSE: use the supported source-build route
ELSE:
  use the source-development route below and report “development bootstrap”, not “HomeBot installed”
```

Do not infer a release from a tag name alone. Verify GitHub release provenance, artifact name, SHA-256 checksum from the release manifest, platform/architecture match, and signature/notarisation where applicable.

## 3. Dependencies for the current source route

macOS with Homebrew:

```bash
xcode-select -p >/dev/null 2>&1 || xcode-select --install
brew install git rustup-init
rustup-init -y --profile minimal --default-toolchain stable
```

Arch/Omarchy (install a Secret Service provider such as GNOME Keyring for BYOK secrets):

```bash
sudo pacman -S --needed base-devel git rustup gnome-keyring
rustup default stable
rustup component add clippy rustfmt
```

Debian/Ubuntu fallback source host:

```bash
sudo apt-get update
sudo apt-get install -y build-essential curl git pkg-config
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable
```

After rustup installation, start a new login shell or source the path stated by rustup. Do not assume a user home path.

## 4. Clone, build, and test

Use a new destination or inspect an existing checkout before changing it:

```bash
git clone https://github.com/luinbytes/HomeBot.git HomeBot
cd HomeBot
git status --short --branch
git remote -v
cargo --version
rustc --version
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo build --workspace --release
```

If `HomeBot` already exists, do not overwrite, reset, clean, or switch branches. Inspect `git status`, branch, remotes, and recent commits. Stop and report any unrelated modifications.

## 5. Start and verify the development server

Start `./target/release/homebot-server` under the host's normal process supervisor. The current binary binds only to `127.0.0.1:7123`.

Verify the real postcondition:

```bash
curl --fail --silent --show-error http://127.0.0.1:7123/health
```

Expected JSON shape:

```json
{"status":"ok","service":"homebot-server","protocol_version":1}
```

Also verify the process remains alive after the request and the listener is loopback-only using `lsof -nP -iTCP:7123 -sTCP:LISTEN` on macOS or `ss -ltnp '( sport = :7123 )'` on Linux. A command exit code without these postconditions is not success.

SQLite migrations and the authenticated Bot/chat transport are implemented and covered by tests. The standalone server still lacks a supported bootstrap command for creating its first device token/Bot, and real provider health/message verification depends on installed authenticated provider CLIs or a configured BYOK profile. Pairing is not implemented. Therefore an agent MUST report those end-to-end installation checks as unavailable and MUST NOT claim a functional v1 installation.

## 6. Provider checks

Discovery only:

```bash
command -v codex || true
command -v claude || true
```

Do not print provider configuration or tokens. Codex, Claude Code, and OpenAI-compatible adapters are implemented, but this environment may only run their protocol-faithful fixtures. Never place BYOK values in command-line arguments, repository files, SQLite, logs, or this verification transcript. On Linux, verify a Secret Service without displaying entries using `busctl --user status org.freedesktop.secrets >/dev/null`; a missing or locked service is a fail-closed provider blocker, not a reason to use plaintext storage.

## 7. Networking and Android pairing

Current builds: localhost only. Do not enable LAN, Tailscale, reverse proxying, or public access because authentication and pairing are not implemented.

The finished flow will provide a server command that creates a short-lived single-use pairing token, display only a QR/deep link to the owner, exchange it for a named device session, verify that reuse fails, and verify revocation. Exact commands will replace this paragraph before v1.

## 8. Troubleshooting

```bash
git status --short --branch
rustup show
cargo metadata --no-deps --format-version 1 >/dev/null
RUST_LOG=homebot_server=debug ./target/release/homebot-server
curl --verbose http://127.0.0.1:7123/health
```

Typical current failures: Rust is absent from `PATH`, port 7123 is occupied, the checkout is not the expected repository, or platform build tools are missing. Logs must be reviewed for accidental secrets before sharing.

## 9. Uninstall the development build

Stop the exact `homebot-server` process started for this checkout. Removing only `HomeBot/target` removes build output. Removing the checkout removes source and local build output but may remove user edits, so inspect `git status` and obtain explicit approval first.

Do not delete future HomeBot application data, databases, artifacts, browser profiles, credential-store entries, or paired device records by default. v1 uninstall commands will distinguish binaries from user data and require a separate explicit data-removal action.
