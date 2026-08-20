# Source control and pull requests

Status: M4 server-owned workflow, 21 August 2026.

HomeBot exposes Git as a contextual Bot capability. Desktop and Android consume the same authenticated protocol; neither client invokes Git or decides capability policy locally.

## Status and diffs

The server returns a normalized porcelain-v2 status projection: branch or detached HEAD, immutable HEAD object ID, upstream, ahead/behind counts, merge-conflict state, staged/unstaged/untracked entries, rename sources, and remote names. Remote URLs never cross the client contract because they can contain credentials. Staged and unstaged endpoints return bounded, rename-aware, full-index binary patches and normalized file summaries.

## Local mutations

Commit can use the current index or explicitly stage every non-ignored workspace change. Commit messages are bounded, Git runs without a shell or interactive prompt, and failures leave workspace content available for recovery. Branch creation is deliberately clean-worktree-only and validates literal ref names before Git runs. Detached HEAD, conflicts, dirty workspaces, and missing remotes are explicit states rather than guessed client errors.

Every commit, branch, push, and pull-request response is stored by owner, chat, action, and idempotency key in SQLite migration 15. A retry returns the exact prior response. If the server claimed a mutation but crashed before recording its external result, it refuses to repeat the operation and asks the client to refresh status.

## Remote approval boundary

Push uses the server capability engine's `git_remote` class. Pull-request creation uses `external_mutation`. The first exact request produces a digest-bound, expiring, single-use approval; duplicate submissions reuse the same pending approval. The authenticated approval endpoint records allow/deny. Only an allowed retry with the same operation identity and canonical workspace/remote/branch resource can execute. Denial is tested to occur before a real remote ref changes.

Remote Git inherits only the credential-discovery environment needed by the trusted Git executable (`HOME`, `XDG_CONFIG_HOME`, and `SSH_AUTH_SOCK`); prompts remain disabled and values are never logged or returned. Authentication failure is normalized. Push output and remote URLs are not exposed.

## Pull requests

GitHub remotes are recognized from HTTPS or SSH syntax and reduced to a validated `owner/repository` slug. The server provides compare metadata and, when the GitHub CLI is installed and authenticated, reads pull-request state from its structured JSON interface. Creation is approval-gated, invokes the fixed CLI directly without a shell, and immediately re-reads normalized JSON. Unsupported remotes and a missing CLI are reported as unavailable without weakening Git status/commit/push workflows.

## Verification

Real repositories cover staged, unstaged and untracked status; binary-capable diffs; exact idempotent commits; clean-only branch creation; detached HEAD; merge conflicts; no remote; local bare-remote push; approval deduplication and denial; approved/replayed push; credential-free remote projection; GitHub URL parsing; and structured pull-request metadata/create fixtures. Server integration exercises the authenticated HTTP routes and durable approval/result records used by desktop and Android.
