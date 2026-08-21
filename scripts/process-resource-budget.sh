#!/bin/sh
set -eu

binary=${1:-target/release/homebot-server}
test -x "$binary"
temporary=$(mktemp -d "${TMPDIR:-/tmp}/homebot-resource.XXXXXX")
pid=
cleanup() {
    if test -n "$pid"; then
        kill "$pid" 2>/dev/null || true
        wait "$pid" 2>/dev/null || true
    fi
    rm -rf "$temporary"
}
trap cleanup EXIT HUP INT TERM

port=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')
started=$(python3 -c 'import time; print(time.monotonic())')
HOMEBOT_DATABASE="$temporary/homebot.db" \
HOMEBOT_DEVICE_TOKEN="ci-owner-token" \
HOMEBOT_BIND="127.0.0.1:$port" \
"$binary" >"$temporary/server.log" 2>&1 &
pid=$!

ready=false
for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
    if curl --fail --silent "http://127.0.0.1:$port/health" >"$temporary/health.json"; then
        ready=true
        break
    fi
    sleep 0.25
done
test "$ready" = true
finished=$(python3 -c 'import time; print(time.monotonic())')
python3 - "$started" "$finished" <<'PY'
import sys
elapsed = float(sys.argv[2]) - float(sys.argv[1])
assert elapsed <= 5.0, f"cold start {elapsed:.3f}s exceeded 5s"
print(f"cold_start_seconds={elapsed:.3f}")
PY

sleep 15
if test -r "/proc/$pid/stat" && test -r "/proc/$pid/status"; then
    ticks_before=$(awk '{ print $14 + $15 }' "/proc/$pid/stat")
    sample_started=$(python3 -c 'import time; print(time.monotonic())')
    sleep 5
    ticks_after=$(awk '{ print $14 + $15 }' "/proc/$pid/stat")
    sample_finished=$(python3 -c 'import time; print(time.monotonic())')
    clock_ticks=$(getconf CLK_TCK)
    cpu=$(python3 - "$ticks_before" "$ticks_after" "$clock_ticks" "$sample_started" "$sample_finished" <<'PY'
import sys
ticks = float(sys.argv[2]) - float(sys.argv[1])
elapsed = float(sys.argv[5]) - float(sys.argv[4])
print(f"{ticks / float(sys.argv[3]) / elapsed * 100.0:.3f}")
PY
)
    rss_kib=$(awk '/^VmRSS:/ { print $2 }' "/proc/$pid/status")
else
    process_sample=$(ps -o %cpu=,rss= -p "$pid") || {
        echo "process metrics unavailable for PID $pid" >&2
        exit 1
    }
    set -- $process_sample
    test "$#" -eq 2
    cpu=$1
    rss_kib=$2
fi
python3 - "$cpu" "$rss_kib" <<'PY'
import sys
cpu = float(sys.argv[1])
rss_kib = int(sys.argv[2])
assert cpu <= 2.0, f"idle server CPU {cpu:.2f}% exceeded 2%"
assert rss_kib <= 250 * 1024, f"idle server RSS {rss_kib / 1024:.1f}MiB exceeded 250MiB"
print(f"idle_cpu_percent={cpu:.2f}")
print(f"idle_rss_mib={rss_kib / 1024:.1f}")
PY
