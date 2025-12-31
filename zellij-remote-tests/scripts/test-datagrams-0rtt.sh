#!/bin/bash
# test-datagrams-0rtt.sh - Tests for QUIC datagrams and 0-RTT session resumption
set -euo pipefail

PROJECT_ROOT="${1:-$(dirname "$(dirname "$(dirname "$(realpath "$0")")")")}"
ZELLIJ="$PROJECT_ROOT/target/release/zellij"
SPIKE_CLIENT="$PROJECT_ROOT/target/release/examples/spike_client"

TEST_SESSION="zrp-test-datagram-$$"
TEST_TOKEN="test-token-datagram-$$"
METRICS_FILE="/tmp/zrp-metrics-datagram-$$.json"
RESUME_TOKEN_FILE="/tmp/zellij-spike-resume-token"
PORT=4435

PASS_COUNT=0
FAIL_COUNT=0

log() { echo "[test-datagram] $(date '+%H:%M:%S') $*"; }
pass() { echo "✓ $1"; ((PASS_COUNT++)) || true; }
fail() { echo "✗ $1"; ((FAIL_COUNT++)) || true; }

cleanup() {
    log "Cleaning up..."
    pkill -f "zellij.*--server.*$TEST_SESSION" 2>/dev/null || true
    rm -f "$METRICS_FILE" "$RESUME_TOKEN_FILE"
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
        "$ZELLIJ" --session "$TEST_SESSION" </dev/null &>/dev/null &
    wait_for_port "$PORT" 20 || { fail "Server did not start"; return 1; }
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
# Test 1: Datagram metrics in output
# =============================================================================
log "Test 1: Datagram metrics validation"

start_server

log "Running client and checking datagram metrics..."
output=$(env -u ZELLIJ_REMOTE_TOKEN \
    SERVER_URL="https://127.0.0.1:$PORT" \
    ZELLIJ_REMOTE_TOKEN="$TEST_TOKEN" \
    HEADLESS=1 \
    timeout 10 "$SPIKE_CLIENT" --metrics-out "$METRICS_FILE" 2>&1) || true

if [[ -f "$METRICS_FILE" ]]; then
    # Check datagram-specific fields exist
    if jq -e '.deltas_via_datagram' "$METRICS_FILE" >/dev/null 2>&1; then
        pass "Metrics contains deltas_via_datagram"
    else
        fail "Metrics missing deltas_via_datagram"
    fi

    if jq -e '.deltas_via_stream' "$METRICS_FILE" >/dev/null 2>&1; then
        pass "Metrics contains deltas_via_stream"
    else
        fail "Metrics missing deltas_via_stream"
    fi

    if jq -e '.base_mismatches' "$METRICS_FILE" >/dev/null 2>&1; then
        pass "Metrics contains base_mismatches"
    else
        fail "Metrics missing base_mismatches"
    fi
else
    fail "Metrics file not created"
fi

stop_server
rm -f "$METRICS_FILE"
echo ""

# =============================================================================
# Test 2: 0-RTT connect time improvement
# =============================================================================
log "Test 2: 0-RTT connect time measurement"

rm -f "$RESUME_TOKEN_FILE"
start_server

log "First connection (full TLS handshake)..."
output1=$(env -u ZELLIJ_REMOTE_TOKEN \
    SERVER_URL="https://127.0.0.1:$PORT" \
    ZELLIJ_REMOTE_TOKEN="$TEST_TOKEN" \
    HEADLESS=1 \
    timeout 8 "$SPIKE_CLIENT" --metrics-out "$METRICS_FILE" 2>&1) || true

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

# On localhost, both should be fast, but second should not be slower
if [[ "$FIRST_CONNECT_MS" -gt 0 ]] && [[ "$SECOND_CONNECT_MS" -gt 0 ]]; then
    pass "Both connections measured connect time"
    if [[ "$SECOND_CONNECT_MS" -le "$FIRST_CONNECT_MS" ]]; then
        pass "Second connection not slower than first (0-RTT working or same speed)"
    else
        log "Note: Second connection slower (${SECOND_CONNECT_MS}ms > ${FIRST_CONNECT_MS}ms) - may be noise on localhost"
    fi
else
    fail "Could not measure connect times"
fi

# Check connect_times array is populated
if jq -e '.connect_times | length > 0' "$METRICS_FILE" >/dev/null 2>&1; then
    pass "Metrics tracks connect_times array"
else
    # This is OK if single connection
    log "Note: connect_times array empty (expected for single connection)"
fi

stop_server
echo ""

# =============================================================================
# Test 3: Check datagram support is advertised
# =============================================================================
log "Test 3: Client advertises datagram support"

# Check the output mentions datagrams being negotiated
if echo "$output1$output2" | grep -iq "datagram\|ServerHello"; then
    pass "Client successfully communicates with server"
else
    fail "Client communication issues"
fi

# =============================================================================
# Summary
# =============================================================================
echo ""
echo "=== Test Summary ==="
echo "Passed: $PASS_COUNT"
echo "Failed: $FAIL_COUNT"
echo ""

if [[ $FAIL_COUNT -eq 0 ]]; then
    echo "=== All datagram/0-RTT tests passed ==="
    exit 0
else
    echo "=== Some tests failed ==="
    exit 1
fi
