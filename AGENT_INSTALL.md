# HomeBot deterministic agent installation

Status: pre-release development bootstrap. An agent MUST NOT report HomeBot installed for end-user use until a signed release route and the complete verification section are available. Current success means only that the development workspace builds, tests, starts on loopback, and returns its health contract.

An agent MUST NOT approve a desktop update on the user's behalf. Validate the release manifest, artifact checksum/signature, compatibility, and backup state, then leave the explicit download/install action to the user unless the user separately authorizes that exact release artifact. Use [docs/recovery.md](docs/recovery.md) for deterministic migration recovery.

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
sudo pacman -S --needed base-devel git rustup gnome-keyring systemd openssl
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

SQLite migrations, authenticated Bot/chat transport, secure pairing, and the native Android client are implemented and covered by tests. A source-development server requires an operator-provided owner credential; the packaged systemd route below creates one without displaying or storing it as plaintext. Real provider health/message verification depends on installed authenticated provider CLIs or a configured BYOK profile. Therefore an agent MUST report unavailable end-to-end installation checks accurately and MUST NOT claim a functional v1 installation.

For a verified Arch package artifact, create the encrypted systemd owner credential and start the headless unit without exposing the token:

```bash
install -d -m 700 "$HOME/.config/homebot"
openssl rand -hex 32 | systemd-creds encrypt --user - "$HOME/.config/homebot/homebot-owner-token.cred"
systemctl --user daemon-reload
systemctl --user enable --now homebot.service
systemctl --user is-active --quiet homebot.service
curl --fail --silent --show-error http://127.0.0.1:7123/health
```

Require the health JSON shape documented above, a live process after the request, a loopback-only listener, and a database under `~/.local/share/homebot`. Do not print, decrypt, or copy the credential into `server.env`.

## 6. Provider checks

Discovery only:

```bash
command -v codex || true
command -v claude || true
```

Do not print provider configuration or tokens. Codex, Claude Code, and OpenAI-compatible adapters are implemented, but this environment may only run their protocol-faithful fixtures. Never place BYOK values in command-line arguments, repository files, SQLite, logs, or this verification transcript. On Linux, verify a Secret Service without displaying entries using `busctl --user status org.freedesktop.secrets >/dev/null`; a missing or locked service is a fail-closed provider blocker, not a reason to use plaintext storage.

## 7. Networking and Android pairing

Default to `HOMEBOT_BIND=127.0.0.1:7123`. For an explicitly requested private listener, detect the machine address first, set `HOMEBOT_BIND=IP:7123` and `HOMEBOT_ALLOW_REMOTE=1`, and confirm the startup log contains the remote-listener warning. Prefer Tailscale. Never advertise a public/custom HTTP endpoint; terminate HTTPS at a trusted reverse proxy.

Create, inspect, and revoke pairing/device state with the deterministic owner-authenticated commands in [docs/remote-access.md](docs/remote-access.md). Verify that the offer expires within five minutes, the deep link contains an `hbpair_` token rather than an `hbds_` session, a second exchange fails, the new named device appears in `GET /api/v1/devices`, and authenticated access fails after revocation. Never print a persistent device session in diagnostic output.

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
