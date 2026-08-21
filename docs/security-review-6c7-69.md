# M6 security review

This review re-executes the M0 threat model against the release-candidate architecture. It is a security gate, not a claim that untrusted Bots, repositories, plugins, providers, clients, or network peers are benign.

## Reviewed boundaries

| Boundary | Verification | Result |
| --- | --- | --- |
| HTTP and WebSocket authentication | Owner/device authentication, protocol skew, revocation, bounded server/Android WebSocket queues and messages, replay cursor and idempotency integration tests | Enforced server-side |
| Pairing | Expiry, one-time use, origin binding, durable rate limit, device administration denial and restart/revocation tests | Enforced server-side |
| Capability policy | Every capability class defaults to a structured approval; deny precedence, digest binding, expiry, policy-revision invalidation and single use are tested | Fails closed |
| Filesystem and PTY | Traversal, absolute paths, symlinks, environment injection, working-directory escape, timeout, cancellation and output-limit fixtures | Fails closed |
| Browser | Remote control endpoints, unsafe navigation, profile symlinks, bounded messages and approval boundaries | Fails closed |
| Git repositories | Dirty-tree preservation, hostile refs and paths, disabled hooks, and executable local/worktree Git configuration fixtures | Fails closed |
| Plugins and MCP | Direct executable/no shell, cleared environment, deadlines, bounded stderr/stdout, message/schema/tool limits and malicious oversized peer fixture | Bounded and untrusted |
| Providers and secrets | Cleared child environments, redacted diagnostics, OS-vault references, no redirects, HTTPS remote endpoints and rejection of credentials in endpoint URLs | Secret-reference-only |
| Persistence and updates | Owner scoping, migration backup integrity, corruption/newer-schema refusal, symlink rejection, exact artifact digest/size and same-origin manifest tests | Fails closed |
| Protocol parsers | Golden schema/model drift, randomized hostile byte corpus, monotonic cursor properties, JSON/body and WebSocket bounds | Bounded |

## Findings resolved during the review

Repository-local Git configuration was an executable-content boundary. A hostile repository could define credential helpers, filters, file monitors, external diff commands, transport commands or proxy/URL rewrites that Git might execute during an otherwise approved server operation. `GitRuntime` now inspects effective local and enabled worktree configuration before every repository command and returns a safe structured conflict before invoking Git. Server and VCS fixtures prove credential-helper, filter, file-monitor and text-conversion canaries are never created. Hooks remain explicitly disabled for mutating operations.

OpenAI-compatible profile URLs could carry user-info, query parameters or fragments. Those components can contain credentials and are visible in URL diagnostics. Profiles now reject all three; credentials remain available only through an opaque `SecretReference` resolved immediately before a request.

The authenticated WebSocket previously relied on library defaults for inbound frame/message size. The upgrade now caps frames at 64 KiB and messages at 256 KiB, with an integration test proving an oversized peer is disconnected.

The Android event client previously used an unlimited parser channel. A malicious or compromised configured server could outpace parsing and grow the client heap. The channel is now bounded to 128 events, applies backpressure by closing with retry semantics, and rejects messages larger than the shared 256 KiB transport budget.

## Automated release checks

`scripts/security-gate.sh` scans tracked text for common live credential/private-key shapes and rejects credential-shaped SQLite columns. CI runs it alongside strict clippy, the full Rust test workspace, dependency policy/audit, protocol schema generation, generated Android model drift and packaging jobs.

The randomized protocol corpus is deterministic and reproducible. It complements, rather than replaces, authenticated integration fixtures and typed golden contracts. Any crash, unexpected acceptance of unversioned noise, cursor-property violation, secret-shaped tracked value, or plaintext credential column fails CI.

## Residual and external risks

- The egui dependency graph contains the documented unmaintained `ttf-parser` advisory. It is not a known vulnerability and no safe compatible upgrade is published; CI continues to fail known vulnerability advisories and pins this exception narrowly.
- A process already running as the same OS account as HomeBot can generally interfere with that account's files and process environment. HomeBot still uses server-owned directories, restrictive secret storage, no-follow/path checks where boundaries accept untrusted paths, and refuses unsafe repository configuration. Strong isolation from a fully compromised same-account process requires an OS sandbox outside the v1 self-host contract.
- Release signing/notarisation credentials and authenticated live Codex/Claude acceptance are external release gates. Their absence is not treated as a passed security check.

No unresolved high- or critical-severity finding was identified in the reviewed release-candidate boundaries. This conclusion remains conditional on the full CI gate and final platform/provider acceptance succeeding for the exact published commit.
