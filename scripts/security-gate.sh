#!/usr/bin/env bash
set -euo pipefail

repo_root=$(git rev-parse --show-toplevel)
cd "$repo_root"

# Search tracked text only. The scanner itself is excluded so its signatures do not self-match.
if git grep -IEn \
  '(sk-[A-Za-z0-9_-]{20,}|gh[pousr]_[A-Za-z0-9]{20,}|AKIA[0-9A-Z]{16}|-----BEGIN (RSA |EC |OPENSSH )?PRIVATE KEY-----)' \
  -- . ':!scripts/security-gate.sh'
then
  echo 'security gate: a credential-shaped value is present in tracked text' >&2
  exit 1
fi

# HomeBot must never gain an ordinary SQLite column that can hold a secret value.
if git grep -IEn \
  '(api_key|access_token|refresh_token|pairing_token|secret_value)[[:space:]]+(TEXT|BLOB)' \
  -- 'crates/homebot-storage/migrations/*.sql'
then
  echo 'security gate: a plaintext credential-shaped SQLite column is present' >&2
  exit 1
fi

echo 'security gate: tracked secret patterns and SQLite credential columns are absent'
