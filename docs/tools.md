# Local computer capabilities

Status: M1 capability layer, 20 August 2026.

`homebot-tools` is the server-side boundary for filesystem, terminal and browser work. A client or provider can request an operation, but only the server-owned `PolicyEngine` can mint the private authorization value consumed by a capability service. The crate defaults unmatched requests to approval, applies deny before approval or allow, and binds one-time approvals to the complete canonical request digest and current policy revision.

## Request and approval flow

Every request carries the authenticated owner and device, Bot, chat, workspace and operation identities. A policy rule can scope capability, actor, Bot, chat, workspace and action prefix. Approval tickets contain only safe presentation fields, expire, are single use, and become invalid whenever policy changes. Text from a prompt, provider, plugin or tool output cannot create or satisfy an approval.

Each authorized operation emits structured started and terminal activity through `ActivitySink`. This is the boundary the authoritative server uses to persist normalized events before forwarding them to desktop or Android. Native process, filesystem and Chrome DevTools Protocol payloads do not become client contracts.

## Filesystem

`ScopedFilesystem` opens an existing workspace through `cap-std` and accepts relative paths only. Parent traversal, absolute paths and symlink components are rejected. Reads, writes and directory listings have explicit limits. Writes use a create-new temporary file, flush it, and atomically rename it inside the same capability directory. Destructive removal has a distinct action and approval effect. Write approvals include a digest of the proposed content, so approved bytes cannot be substituted at retry time.

## Terminal and processes

`TerminalService` uses a real platform PTY through `portable-pty`. It launches one explicit absolute executable with structured arguments, an existing workspace-relative working directory and a cleared environment. Only configured environment keys are admitted. Command, arguments, working directory and environment are bound into the approval digest, while logs and debug output omit argument and environment values.

PTY input, output, dimensions and runtime are bounded. A run exposes streamed byte chunks, resize and input controls, exit status, cancellation and timeout. Slow consumers trigger bounded backpressure termination rather than unbounded memory growth. Cancellation and timeout kill and reap the child before the terminal event is emitted.

## Browser

`BrowserService` controls page targets over Chrome DevTools Protocol. It accepts only a loopback HTTP control endpoint and loopback WebSocket target URLs, disables HTTP redirects, bounds protocol messages and call duration, and normalizes navigation, evaluation, current URL and PNG screenshot results. Browser profile paths must remain beneath a server-owned local profile root with no symlink components. Cookies, storage databases and other browser authentication files are not copied into HomeBot SQLite or synchronized to clients. Authorized page actions can still return page-derived data, so observe and act policy remains mandatory.

Navigation accepts HTTP and HTTPS only and rejects credentials embedded in URLs. Observe and act are separate capabilities. Each browser action, including its URL or expression, is digest-bound to its approval without writing the action contents to activity details.

## Verification

Unit and hostile-input integration fixtures cover policy denial, scoped actors, one-time/expiring approvals, policy revision invalidation, content substitution, traversal, symlink escape, atomic file replacement, environment injection, working-directory escape, PTY output, cancellation, timeout, output limits, remote browser endpoints, unsafe navigation and local CDP action round trips.
