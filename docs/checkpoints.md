# Turn checkpoints, diffs and restore

Status: M4 coding-workflow foundation, 20 August 2026.

For a chat with an attached repository, the authoritative server brackets every provider turn with before/after Git checkpoints. Checkpoints are commits reachable only through `refs/homebot/checkpoints/<chat>/<checkpoint>`; they never move the user's branch, working-tree `HEAD`, or real index.

## Capture model

HomeBot builds each checkpoint with a short-lived alternate Git index. It seeds that index from `HEAD`, stages the complete tracked, staged, unstaged, and non-ignored untracked workspace state, writes a tree, creates a commit object, and atomically anchors the commit under its hidden ref. The alternate index is server metadata and is removed after capture. Dirty primary baselines therefore remain dirty in exactly the same way after checkpointing.

SQLite migration 14 stores the owner, chat, repository workspace, optional Bot message, before/after/restore-safety phase, hidden ref, immutable object ID, provider profile/conversation context, and creation time. Client summaries deliberately omit the Git object ID and hidden ref; clients address opaque checkpoint UUIDs through authenticated APIs.

## Diffs

Per-turn diff compares an explicit before/after checkpoint pair. Full-chat diff compares the earliest before-turn checkpoint with the latest completed after-turn checkpoint. Git produces a full-index, rename-aware, binary-capable patch, plus a normalized changed-file summary with added/modified/deleted/renamed/copied/type-changed state and binary indication. Output is bounded to protect the server.

## Safe restore

Restore is allowed only while the Bot is stopped and while the chat still points at the same repository workspace. Before changing files, HomeBot captures the current state under a new restore-safety ref. It then uses another alternate index to materialize the target tree and removes only paths proven to exist in the captured current tree but not the target. The user's index and branch remain unchanged; ignored files are not deleted. A restore is refused before mutation when a target path would overwrite ignored content that the safety checkpoint cannot capture.

Provider conversations generally cannot be rewound to match a filesystem checkpoint. When an active provider mapping exists, HomeBot clears that mapping atomically with the restore audit record and reports `forked`; the next turn starts a fresh provider conversation against the restored files. If there was no mapping, reconciliation is `unchanged`. Repeating the same restore idempotency key returns the original result and creates no additional safety checkpoint.

## Verification

Real-Git fixtures cover staged state, dirty baselines, untracked files, renames, binary patches, ignored-path conflicts, index preservation, hidden refs, exact diff summaries and recovery through the safety checkpoint. Server integration brackets an actual fixture-provider turn, serves per-turn/full-chat diffs, restores through authenticated HTTP, clears the incompatible provider mapping, persists the audit trail and verifies idempotent replay.
