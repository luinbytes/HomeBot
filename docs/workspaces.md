# Repository workspaces

Status: M4 coding-workspace foundation, 20 August 2026.

HomeBot registers an existing Git repository once and may associate a chat with either its primary working tree or a server-managed isolated worktree. The repository remains user-owned. HomeBot never cleans, resets, checks out, or deletes the primary working tree, and a dirty primary does not prevent creating an isolated coding chat.

## Durable model

`repository_workspaces` stores the owner-scoped canonical repository path and display name. `chat_workspaces` stores one optional association per chat, including primary/isolated mode, effective path, branch, base ref, and timestamps. The server re-inspects Git for current branch and clean/dirty/conflicted state when producing an authenticated snapshot or response. A missing repository is reported as unavailable without deleting its durable association.

The Rust-owned protocol exposes repository and chat-workspace summaries in the initial snapshot and sequenced changed/removed events. Desktop treats these values as a replaceable projection and sends register, attach, branch-list, and detach mutations through its authenticated server transport. The generated Android contract contains the same models and requests.

## Isolated worktrees

An isolated worktree path is deterministic: `<server-managed-root>/<chat UUID>`. Unless the caller chooses another valid branch, its branch is `homebot/<chat UUID>`. The requested base may be an existing local branch or another literal Git ref such as `HEAD`; option-like and syntactically dangerous ref values are rejected before Git runs.

Git is invoked directly without a shell, with inherited environment and interactive prompting disabled, a timeout, and bounded accepted output. Worktree creation uses `git worktree add` and does not alter the primary checkout or its uncommitted/untracked files.

## Detach and failure safety

Detaching a primary association removes only the SQLite association. Detaching an isolated association first canonicalizes the managed root and worktree, proves the path is a child of that root, and verifies it is clean. Dirty or conflicted worktrees are preserved and the server returns a conflict; the durable association remains so the user can recover their work. A database failure after external worktree creation triggers best-effort cleanup only through the same guarded path.

HomeBot intentionally retains an isolated worktree's Git branch after clean detachment. Later source-control work can decide whether and when a branch should be deleted; lifecycle cleanup must never destroy commits implicitly.

## Verification

Fixtures cover dirty and untracked primary changes, clean isolated creation/removal, dirty cleanup denial, out-of-root cleanup denial, hostile refs, detached HEAD, primary association, idempotent API replay, restart persistence, and v12-to-v13 migration.
