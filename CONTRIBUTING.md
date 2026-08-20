# Contributing to HomeBot

HomeBot is currently establishing its implementation contracts. Discuss large product or architecture changes before investing in them, and link work to a Linear or GitHub issue where possible.

## Development rules

1. Keep user-facing vocabulary messaging-first: Bots, chats, groups, routines, skills, and plugins.
2. Keep product logic on the server and provider-specific payloads behind adapters.
3. Preserve unrelated working-tree changes and test dirty repository cases for Git features.
4. Add negative tests for permission boundaries and failure tests for reconnectable operations.
5. Update the protocol schema, compatibility notes, Android validation, docs, and parity row when applicable.
6. Do not commit secrets, credentials, captured private data, or proprietary assets.

Run formatting, clippy with warnings denied, and the complete touched test suite before opening a pull request. UI changes require deterministic golden evidence for every affected visible state.
