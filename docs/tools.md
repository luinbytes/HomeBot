# Local computer capabilities

Status: provider-integrated capability layer, 27 August 2026.

`homebot-tools` is the server-side boundary for filesystem, terminal and browser work. A client or provider can request an operation, but only the server-owned `PolicyEngine` can mint the private authorization value consumed by a capability service. The crate defaults unmatched requests to approval, applies deny before approval or allow, and binds one-time approvals to the complete canonical request digest and current policy revision.

## Request and approval flow

Every request carries the authenticated owner and device, Bot, chat, workspace and operation identities. A policy rule can scope capability, actor, Bot, chat, workspace and action prefix. Approval tickets contain only safe presentation fields, expire, are single use, and become invalid whenever policy changes. Text from a prompt, provider, plugin or tool output cannot create or satisfy an approval.

Each authorized operation emits structured started and terminal activity through `ActivitySink`. This is the boundary the authoritative server uses to persist normalized events before forwarding them to desktop or Android. Native process, filesystem and Chrome DevTools Protocol payloads do not become client contracts.

Provider-neutral structured input uses the same server-owned turn boundary. `homebot_request_decision` creates either a confirm card or a bounded pick-one card; `homebot_request_secret` creates a native password card. The request is tied to one active operation, accepts exactly one response, expires or cancels with that operation, and resumes only its waiting tool call. Secret values go directly to an opaque OS-vault locator. SQLite, timeline events, provider results, and idempotency hashes receive only the locator and a secure acknowledgement, never the submitted value. Desktop and Android render the server activity rather than inventing client-side Bot state.

## Filesystem

`ScopedFilesystem` opens an existing workspace through `cap-std` and accepts relative paths only. Parent traversal, absolute paths and symlink components are rejected. Reads, writes and directory listings have explicit limits. Writes use a create-new temporary file, flush it, and atomically rename it inside the same capability directory. Destructive removal has a distinct action and approval effect. Write approvals include a digest of the proposed content, so approved bytes cannot be substituted at retry time.

Every provider receives the same bounded list, read, write, and create-directory tools rooted at the authoritative chat workspace or the Bot's private server directory. Binary reads are explicitly base64-labelled, writes accept UTF-8 only, and neither local paths nor capability roots enter the model contract. Write and directory mutations use the ordinary durable HomeBot approval flow.

## Terminal and processes

`TerminalService` uses a real platform PTY through `portable-pty`. It launches one explicit absolute executable with structured arguments, an existing workspace-relative working directory and a cleared environment. Only configured environment keys are admitted. Command, arguments, working directory and environment are bound into the approval digest, while logs and debug output omit argument and environment values.

The provider-neutral command tool keeps that narrow contract: one absolute executable, at most 64 bounded arguments, no model-supplied environment, one process per Bot operation, 256 KiB output, and a two-minute deadline. Stopping the Bot turn cancels and reaps the server-owned process. This deliberately does not expose an implicit shell, background daemon, arbitrary environment, or interactive input channel.

PTY input, output, dimensions and runtime are bounded. A run exposes streamed byte chunks, resize and input controls, exit status, cancellation and timeout. Slow consumers trigger bounded backpressure termination rather than unbounded memory growth. Cancellation and timeout kill and reap the child before the terminal event is emitted.

## Browser

`BrowserService` controls page targets over Chrome DevTools Protocol. It accepts only a loopback HTTP control endpoint and loopback WebSocket target URLs, disables HTTP redirects, bounds protocol messages and call duration, and normalizes navigation, evaluation, current URL and PNG screenshot results. Browser profile paths must remain beneath a server-owned local profile root with no symlink components. Cookies, storage databases and other browser authentication files are not copied into HomeBot SQLite or synchronized to clients. Authorized page actions can still return page-derived data, so observe and act policy remains mandatory.

The server additionally fences each browser-runtime action at 30 seconds. A stalled worker is detached, its authoritative session and activity become failed, approval and takeover leases are cleared, and the next open can create a fresh runtime session. HomeBot never leaves a timed-out worker projected as active.

Every provider adapter receives the same five server-owned browser tools when a local browser runtime is configured: open, HTTPS navigation, current URL, PNG screenshot, and close. They reuse one persistent server profile and the latest active session for the chat, use the existing durable approval flow, attach screenshot artifacts to the Bot message, and stop cleanly with the provider operation. Arbitrary JavaScript, cookies, CDP addresses, local paths, headers, and raw profile data are deliberately absent from the model contract. Android only projects these server sessions and approvals; it does not run a browser or Bot runtime.

The live CDP target registry is intentionally in-memory. Production startup now terminalizes every persisted `active` or `awaiting_approval` browser projection as `failed`, clears its runtime target, takeover lease and pending approval, and fails its running activity. A restarted server therefore shows a truthful reopenable failure instead of claiming that a vanished browser worker is still active.

A human takeover also pauses every HomeBot-managed connector tool in that chat. The shared plugin execution seam checks the durable takeover state before resolving or transmitting a stored credential, and resumes only after the controlling device explicitly returns the browser to the Bot. Provider-native credentials outside HomeBot's vault remain outside this guarantee and are not described as gated.

The native browser API accepts HTTP and HTTPS. The narrower provider tool accepts HTTPS only; both reject credentials embedded in URLs. Observe and act are separate capabilities. Each browser action, including its URL or expression, is digest-bound to its approval without writing the action contents to activity details.

## Verification

Unit and hostile-input integration fixtures cover policy denial, scoped actors, four-pending and ten-minute approval bounds, one-time/expiring approvals, policy revision invalidation, content substitution, traversal, symlink escape, atomic file replacement, environment injection, working-directory escape, PTY output, cancellation, timeout, output limits, remote browser endpoints, unsafe navigation, local CDP action round trips, approval-gated provider turns, and a secure-input turn whose value is absent from transcript and SQLite files. Desktop interaction controls have rendered golden and accessibility-tree coverage; Android response transport and Compose compilation are automated, while physical TalkBack acceptance remains open.
