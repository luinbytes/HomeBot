#!/usr/bin/env bash
set -euo pipefail

cargo test -p homebot-desktop performance::
cargo test -p homebot-desktop large_transcript_and_concurrent_stream_projections_meet_release_budgets
cargo test -p homebot-desktop text_scaling_is_clamped_and_preserves_geometry
cargo test -p homebot-server cold_start_and_authenticated_protocol_probe_meet_release_budgets
cargo test -p homebot-server reconnect_replays_events_strictly_after_cursor
cargo test -p homebot-server reconnect_uses_snapshot_when_cursor_falls_outside_retention

grep -q 'Role\.Tab' android/app/src/main/java/dev/homebot/android/MainActivity.kt
grep -q 'LiveRegionMode\.Assertive' android/app/src/main/java/dev/homebot/android/MainActivity.kt
grep -q 'heading()' android/app/src/main/java/dev/homebot/android/MainActivity.kt
grep -q 'text_scale_percent' crates/homebot-desktop/src/settings.rs

echo "performance/accessibility gate: automated budgets and semantics passed"
