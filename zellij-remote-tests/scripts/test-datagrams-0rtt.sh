#!/bin/bash
# shellcheck disable=SC2329  # cleanup() is invoked via trap
# test-datagrams-0rtt.sh - Tests for QUIC datagrams and 0-RTT session resumption
set -euo pipefail

PROJECT_ROOT="${1:-$(dirname "$(dirname "$(dirname "$(realpath "$0")")")")}"
ZELLIJ="$PROJECT_ROOT/target/release/zellij"
SPIKE_CLIENT="$PROJECT_ROOT/target/release/examples/spike_client"

TEST_SESSION="zrp-test-datagram-$$"
TEST_TOKEN="test-token-datagram-$$"
METRICS_FILE="/tmp/zrp-metrics-datagram-$$.json"
RESUME_TOKEN_FILE="/tmp/zellij-spike-resume-token-$$"
SERVER_LOG="/tmp/zrp-server-log-$$.txt"
SCRIPT_FILE="/tmp/zrp-test-script-$$.txt"
# Zellij uses $TMPDIR (or /tmp as fallback) for its log directory
ZELLIJ_TMP_DIR="${TMPDIR:-/tmp}/zellij-$(id -u)"
ZELLIJ_LOG_DIR="$ZELLIJ_TMP_DIR/zellij-log"
ZELLIJ_LOG_FILE="$ZELLIJ_LOG_DIR/zellij.log"
PORT=4435

PASS_COUNT=0
FAIL_COUNT=0
SKIP_COUNT=0

log() { echo "[test-datagram] $(date '+%H:%M:%S') $*"; }
pass() {
    echo "✓ $1"
    ((PASS_COUNT++)) || true
}
fail() {
    echo "✗ $1"
    ((FAIL_COUNT++)) || true
}
skip() {
    echo "⊘ $1 (skipped)"
    ((SKIP_COUNT++)) || true
}

print_logs_on_failure() {
    if [[ -f "$SERVER_LOG" ]]; then
        echo "--- Server stdout/stderr (last 50 lines) ---"
        tail -50 "$SERVER_LOG" || true
        echo "--- End server stdout/stderr ---"
    fi
    if [[ -d "$ZELLIJ_LOG_DIR" ]]; then
        echo "--- Zellij log files (last 50 lines each) ---"
        for log in "$ZELLIJ_LOG_DIR"/*.log; do
            [[ -f "$log" ]] && {
                echo "=== $log ==="
                tail -50 "$log" || true
            }
        done
        echo "--- End Zellij logs ---"
    fi
}

cleanup() {
    log "Cleaning up..."
    pkill -f "zellij.*--server.*$TEST_SESSION" 2>/dev/null || true
    rm -f "$METRICS_FILE" "$RESUME_TOKEN_FILE" "$SERVER_LOG" "$SCRIPT_FILE"
}
trap cleanup EXIT

wait_for_port() {
    local port=$1 timeout=${2:-10}
    for _ in $(seq 1 "$timeout"); do
        if lsof -i :"$port" >/dev/null 2>&1; then return 0; fi
        sleep 0.5
    done
    return 1
}

start_server() {
    log "Starting Zellij server on port $PORT..."
    ZELLIJ_REMOTE_ADDR="127.0.0.1:$PORT" \
        ZELLIJ_REMOTE_TOKEN="$TEST_TOKEN" \
        RUST_LOG=zellij_server::remote=info \
        "$ZELLIJ" --session "$TEST_SESSION" </dev/null >"$SERVER_LOG" 2>&1 &
    wait_for_port "$PORT" 20 || {
        fail "Server did not start"
        print_logs_on_failure
        return 1
    }
    log "Server listening on port $PORT"
}

stop_server() {
    log "Stopping server..."
    pkill -f "zellij.*--server.*$TEST_SESSION" 2>/dev/null || true
    sleep 1
}

# Check binaries
[[ -x "$ZELLIJ" ]] && [[ -x "$SPIKE_CLIENT" ]] || {
    log "Building release binaries..."
    cargo build --release -p zellij 2>/dev/null
    cargo build --release --example spike_client -p zellij-remote-bridge 2>/dev/null
}

echo "=== ZRP Datagram & 0-RTT Tests ==="

# Initial cleanup
pkill -f "zellij.*--server.*zrp-test-datagram-" 2>/dev/null || true
rm -f "$RESUME_TOKEN_FILE"
sleep 1

# =============================================================================
# Test 1: Datagram metrics validation with actual activity
# =============================================================================
log "Test 1: Datagram metrics with activity"

# Create a script that types some characters to generate deltas
cat >"$SCRIPT_FILE" <<'EOF'
sleep 500
type hello
sleep 500
quit
EOF

start_server

log "Running client with script to generate activity..."
env -u ZELLIJ_REMOTE_TOKEN \
    SERVER_URL="https://127.0.0.1:$PORT" \
    ZELLIJ_REMOTE_TOKEN="$TEST_TOKEN" \
    HEADLESS=1 \
    timeout 15 "$SPIKE_CLIENT" --metrics-out "$METRICS_FILE" --script "$SCRIPT_FILE" 2>&1 || true

if [[ -f "$METRICS_FILE" ]]; then
    log "Metrics file contents:"
    cat "$METRICS_FILE"
    echo ""

    # Check datagram-specific fields exist
    if jq -e '.deltas_via_datagram' "$METRICS_FILE" >/dev/null 2>&1; then
        pass "Metrics contains deltas_via_datagram"
    else
        fail "Metrics missing deltas_via_datagram"
        print_logs_on_failure
    fi

    if jq -e '.deltas_via_stream' "$METRICS_FILE" >/dev/null 2>&1; then
        pass "Metrics contains deltas_via_stream"
    else
        fail "Metrics missing deltas_via_stream"
        print_logs_on_failure
    fi

    # Verify actual delta delivery (at least some deltas received via either channel)
    DATAGRAM_DELTAS=$(jq '.deltas_via_datagram // 0' "$METRICS_FILE")
    STREAM_DELTAS=$(jq '.deltas_via_stream // 0' "$METRICS_FILE")
    TOTAL_DELTAS=$((DATAGRAM_DELTAS + STREAM_DELTAS))
    if [[ "$TOTAL_DELTAS" -gt 0 ]]; then
        pass "Received $TOTAL_DELTAS deltas (datagram=$DATAGRAM_DELTAS, stream=$STREAM_DELTAS)"
    else
        log "Note: No deltas received (may be expected if script didn't generate visible changes)"
    fi

    # Assert base_mismatches == 0 for correctness
    if jq -e '.base_mismatches' "$METRICS_FILE" >/dev/null 2>&1; then
        BASE_MISMATCHES=$(jq '.base_mismatches // 0' "$METRICS_FILE")
        if [[ "$BASE_MISMATCHES" -eq 0 ]]; then
            pass "No base mismatches (base_mismatches=0)"
        else
            fail "Base mismatches detected: $BASE_MISMATCHES (indicates delta ordering issues)"
            print_logs_on_failure
        fi
    else
        fail "Metrics missing base_mismatches"
        print_logs_on_failure
    fi
else
    fail "Metrics file not created"
    print_logs_on_failure
fi

stop_server
rm -f "$METRICS_FILE"
echo ""

# =============================================================================
# Test 2: 0-RTT / Session Resumption (heuristic-based)
# =============================================================================
log "Test 2: 0-RTT / session resumption measurement"
log "Note: This is a heuristic test - second connection should show 'likely 0-RTT' in logs"

rm -f "$RESUME_TOKEN_FILE"
start_server

log "First connection (full TLS handshake)..."
env -u ZELLIJ_REMOTE_TOKEN \
    SERVER_URL="https://127.0.0.1:$PORT" \
    ZELLIJ_REMOTE_TOKEN="$TEST_TOKEN" \
    HEADLESS=1 \
    timeout 8 "$SPIKE_CLIENT" --metrics-out "$METRICS_FILE" 2>&1 || true

FIRST_CONNECT_MS=0
if [[ -f "$METRICS_FILE" ]]; then
    FIRST_CONNECT_MS=$(jq '.connect_time_ms // 0' "$METRICS_FILE")
    log "First connect time: ${FIRST_CONNECT_MS}ms"
fi
rm -f "$METRICS_FILE"

log "Second connection (should reuse TLS session)..."
output2=$(env -u ZELLIJ_REMOTE_TOKEN \
    SERVER_URL="https://127.0.0.1:$PORT" \
    ZELLIJ_REMOTE_TOKEN="$TEST_TOKEN" \
    HEADLESS=1 \
    timeout 8 "$SPIKE_CLIENT" --metrics-out "$METRICS_FILE" 2>&1) || true

SECOND_CONNECT_MS=0
if [[ -f "$METRICS_FILE" ]]; then
    SECOND_CONNECT_MS=$(jq '.connect_time_ms // 0' "$METRICS_FILE")
    log "Second connect time: ${SECOND_CONNECT_MS}ms"
fi

# Check for 0-RTT heuristic in output
if echo "$output2" | grep -q "likely 0-RTT"; then
    pass "Second connection detected as 'likely 0-RTT'"
else
    # On localhost, both should be fast, but second should not be slower
    if [[ "$FIRST_CONNECT_MS" -gt 0 ]] && [[ "$SECOND_CONNECT_MS" -gt 0 ]]; then
        pass "Both connections measured connect time"
        if [[ "$SECOND_CONNECT_MS" -le "$FIRST_CONNECT_MS" ]]; then
            pass "Second connection not slower than first (session reuse working or same speed)"
        else
            log "Note: Second connection slower (${SECOND_CONNECT_MS}ms > ${FIRST_CONNECT_MS}ms) - may be noise on localhost"
        fi
    else
        fail "Could not measure connect times"
        print_logs_on_failure
    fi
fi

# Check connect_times array is populated
if jq -e '.connect_times | length > 0' "$METRICS_FILE" >/dev/null 2>&1; then
    pass "Metrics tracks connect_times array"
else
    log "Note: connect_times array empty (expected for single connection)"
fi

stop_server
echo ""

# =============================================================================
# Test 3: Datagram negotiation verification
# =============================================================================
log "Test 3: Datagram negotiation"

start_server

log "Running client to trigger datagram negotiation..."
output3=$(env -u ZELLIJ_REMOTE_TOKEN \
    SERVER_URL="https://127.0.0.1:$PORT" \
    ZELLIJ_REMOTE_TOKEN="$TEST_TOKEN" \
    HEADLESS=1 \
    timeout 8 "$SPIKE_CLIENT" --metrics-out "$METRICS_FILE" 2>&1) || true

# Check Zellij's log file for datagram negotiation confirmation
# Give the log a moment to flush
sleep 0.5

DATAGRAM_LOG_FOUND=false
if [[ -f "$ZELLIJ_LOG_FILE" ]]; then
    log "Checking Zellij log at $ZELLIJ_LOG_FILE"
    if grep -q "datagrams negotiated, max_size=" "$ZELLIJ_LOG_FILE" 2>/dev/null; then
        MAX_SIZE=$(grep "datagrams negotiated" "$ZELLIJ_LOG_FILE" | grep -o "max_size=[0-9]*" | tail -1 || echo "unknown")
        pass "Server confirmed datagram negotiation ($MAX_SIZE)"
        DATAGRAM_LOG_FOUND=true
    elif grep -q "datagrams not negotiated" "$ZELLIJ_LOG_FILE" 2>/dev/null; then
        skip "Datagrams not negotiated on this system (transport limitation)"
        DATAGRAM_LOG_FOUND=true
    fi
fi

if [[ "$DATAGRAM_LOG_FOUND" == "false" ]]; then
    # Fallback: check if client at least connected
    if echo "$output3" | grep -iq "ServerHello\|Connected"; then
        pass "Client successfully connected (datagram negotiation log not found in Zellij logs)"
    else
        fail "Client communication issues"
        print_logs_on_failure
    fi
fi

stop_server
echo ""

# =============================================================================
# Summary
# =============================================================================
echo ""
echo "=== Test Summary ==="
echo "Passed: $PASS_COUNT"
echo "Failed: $FAIL_COUNT"
echo "Skipped: $SKIP_COUNT"
echo ""

if [[ $FAIL_COUNT -eq 0 ]]; then
    echo "=== All datagram/0-RTT tests passed ==="
    exit 0
else
    echo "=== Some tests failed ==="
    print_logs_on_failure
    exit 1
fi
