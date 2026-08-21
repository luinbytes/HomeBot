#!/usr/bin/env bash
set -euo pipefail

cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo run -p homebot-protocol --example export_schema -- --check
cargo run -p homebot-protocol --example export_android -- --check
./scripts/check-packaging.sh
./scripts/security-gate.sh
./scripts/performance-accessibility-gate.sh
