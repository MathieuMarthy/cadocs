#!/usr/bin/env bash
# RAM benchmark: the scenario is fixed in docs/ARCHITECTURE.md. Do not tune it to pass a target.
#
#   bench/rss.sh read <pid> <label> [file]   print one line of measures for a running server
#   bench/rss.sh build                build the servers and the load tool
#   bench/rss.sh run [out-dir]        build, then the scenario with each allocator
#   bench/rss.sh extras [out-dir]     per-socket cost and load peak, outside the scenario
set -euo pipefail

cd "$(dirname "$0")/.."
SCRIPT=bench/rss.sh
BENCH=target/release/cadocs-bench
PORT=${CADOCS_BENCH_PORT:-3917}
ROOM_IDLE_SECS=60 # the default of CADOCS_ROOM_IDLE_SECS
# mimalloc was measured and dropped: see ADR 0015.
ALLOCATORS=(glibc glibc-arena2)

# Prints: label rss pss anonymous peak, all in KiB.
read_measures() {
    local pid=$1 label=$2
    awk -v label="$label" '
        FILENAME ~ /smaps_rollup/ && $1 == "Rss:" { rss = $2 }
        FILENAME ~ /smaps_rollup/ && $1 == "Pss:" { pss = $2 }
        FILENAME ~ /smaps_rollup/ && $1 == "Anonymous:" { anon = $2 }
        FILENAME ~ /status/ && $1 == "VmHWM:" { peak = $2 }
        END { printf "%-24s %8d %8d %8d %8d\n", label, rss, pss, anon, peak }
    ' "/proc/$pid/smaps_rollup" "/proc/$pid/status"
}

build() {
    # `-p` on each: built together, the load tool's features would reach the server's dependencies.
    cargo build --release -p cadocs-server
    cargo build --release -p cadocs-bench
}

# Starts a server on a copy of the seeded database, and sets SERVER_PID and DATA_DIR.
start_server() {
    local allocator=$1 seeded=$2
    DATA_DIR=$(mktemp -d "$WORK/data.XXXXXX")
    cp "$seeded" "$DATA_DIR/cadocs.db"
    local arena=()
    if [[ $allocator == glibc-arena2 ]]; then arena=(MALLOC_ARENA_MAX=2); fi
    # `-i`: the scenario uses default settings, whatever this shell exports.
    env -i "${arena[@]}" CADOCS_DATA_DIR="$DATA_DIR" CADOCS_BIND="127.0.0.1:$PORT" \
        CADOCS_PUBLIC_URL="http://127.0.0.1:$PORT" \
        target/release/cadocs >>"$WORK/server.log" 2>&1 &
    SERVER_PID=$!
    for _ in $(seq 100); do
        if curl -sf "http://127.0.0.1:$PORT/health" >/dev/null; then return; fi
        sleep 0.1
    done
    echo "the server did not start" >&2
    exit 1
}

stop_server() {
    kill "$SERVER_PID"
    wait "$SERVER_PID"
    SERVER_PID=
    rm -rf "$DATA_DIR"
}

# Runs on any exit: a server left behind would answer the next run's health check.
clean_up() {
    if [[ -n ${SERVER_PID:-} ]]; then kill "$SERVER_PID" || true; fi
    cp "$WORK/server.log" "$OUT_DIR/server.log" 2>/dev/null || true
    rm -rf "$WORK"
}

seed() {
    local file=$1 docs=$2
    shift 2
    "$BENCH" seed --db "$file" --docs "$docs" "$@"
    # Folds the WAL back, so that the file alone holds everything when copied.
    sqlite3 "$file" 'PRAGMA wal_checkpoint(TRUNCATE);' >/dev/null
}

# One scenario row on a fresh server: idle, then load, then after unload.
scenario() {
    local allocator=$1 docs=$2 out=$3
    start_server "$allocator" "$WORK/seeded.db"
    sleep 5
    read_measures "$SERVER_PID" "$allocator/idle" | tee -a "$out"
    "$BENCH" load --url "ws://127.0.0.1:$PORT" --docs "$docs" \
        --then "$SCRIPT read $SERVER_PID $allocator/$docs-docs '$out'" 2>>"$out.load"
    sleep $((ROOM_IDLE_SECS + 10))
    read_measures "$SERVER_PID" "$allocator/$docs-docs-unloaded" | tee -a "$out"
    stop_server
}

run() {
    OUT_DIR=$1
    build
    mkdir -p "$OUT_DIR"
    WORK=$(mktemp -d)
    trap clean_up EXIT
    seed "$WORK/seeded.db" 100
    local out="$OUT_DIR/rss.txt"
    rm -f "$out.load"
    printf '%-24s %8s %8s %8s %8s\n' measure rss pss anon peak | tee "$out"
    for allocator in "${ALLOCATORS[@]}"; do
        scenario "$allocator" 10 "$out"
        scenario "$allocator" 100 "$out"
    done
}

# Per-socket cost: one document, then 1 and 201 clients with no edits. Load peak: 100 documents
# opened at once, with a compacted log and with their 2,000 rows uncompacted.
extras() {
    OUT_DIR=$1
    build
    mkdir -p "$OUT_DIR"
    local out="$OUT_DIR/extras.txt"
    WORK=$(mktemp -d)
    trap clean_up EXIT
    seed "$WORK/seeded.db" 100
    seed "$WORK/full-log.db" 100 --keep-log true
    printf '%-24s %8s %8s %8s %8s\n' measure rss pss anon peak | tee "$out"

    start_server glibc "$WORK/seeded.db"
    for clients in 1 201; do
        "$BENCH" load --url "ws://127.0.0.1:$PORT" --docs 1 --clients-per-doc "$clients" \
            --duration-secs 0 --hold-secs 5 \
            --then "$SCRIPT read $SERVER_PID sockets-$clients '$out'"
    done
    stop_server

    for db in seeded full-log; do
        start_server glibc "$WORK/$db.db"
        "$BENCH" load --url "ws://127.0.0.1:$PORT" --docs 100 --clients-per-doc 1 \
            --duration-secs 0 --hold-secs 5 \
            --then "$SCRIPT read $SERVER_PID open-100-$db '$out'"
        stop_server
    done
}

case ${1:-} in
    read) read_measures "$2" "$3" | tee -a "${4:-/dev/null}" ;;
    build) build ;;
    run) run "${2:-target/bench}" ;;
    extras) extras "${2:-target/bench}" ;;
    *)
        sed -n 2,7p "$0"
        exit 1
        ;;
esac
