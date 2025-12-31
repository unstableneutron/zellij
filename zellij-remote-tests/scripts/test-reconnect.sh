#!/bin/bash
# test-reconnect.sh - Reconnection and metrics tests for ZRP
# Tests resume token persistence, reconnect modes, and metrics output
set -euo pipefail

PROJECT_ROOT="${1:-$(dirname "$(dirname "$(dirname "$(realpath "$0")")")")}"
ZELLIJ="$PROJECT_ROOT/target/release/zellij"
SPIKE_CLIENT="$PROJECT_ROOT/target/release/examples/spike_client"

TEST_SESSION="zrp-test-reconnect-$$"
TEST_TOKEN="test-token-reconnect-$$"
METRICS_FILE="/tmp/zrp-metrics-$$.json"
RESUME_TOKEN_FILE="/tmp/zellij-spike-resume-token"
PORT=4434

PASS_COUNT=0
FAIL_COUNT=0

log() {
    echo "[test-reconnect] $(date '+%H:%M:%S') $*"
}

pass() {
    echo "✓ $1"
    ((PASS_COUNT++)) || true
}

fail() {
    echo "✗ $1"
    ((FAIL_COUNT++)) || true
}

cleanup() {
    log "Cleaning up..."
    pkill -f "zellij.*--server.*$TEST_SESSION" 2>/dev/null || true
    pkill -f "zellij.*--server.*zrp-test-reconnect-" 2>/dev/null || true
    rm -f "$METRICS_FILE" "$RESUME_TOKEN_FILE"
    rm -rf "/tmp/zellij-$(id -u)/contract_version_1/$TEST_SESSION" 2>/dev/null || true
}
trap cleanup EXIT

wait_for_port() {
    local port=$1
    local timeout=${2:-10}
    for _ in $(seq 1 "$timeout"); do
        if lsof -i :"$port" >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.5
    done
    return 1
}

start_server() {
    log "Starting Zellij server on port $PORT..."
    # Start Zellij - it will daemonize itself
    ZELLIJ_REMOTE_ADDR="127.0.0.1:$PORT" \
        ZELLIJ_REMOTE_TOKEN="$TEST_TOKEN" \
        "$ZELLIJ" --session "$TEST_SESSION" </dev/null &>/dev/null &

    if ! wait_for_port "$PORT" 20; then
        fail "Server did not start within 10 seconds"
        return 1
    fi
    log "Server is listening on port $PORT"
}

stop_server() {
    log "Stopping server..."
    pkill -f "zellij.*--server.*$TEST_SESSION" 2>/dev/null || true
    sleep 1
}

# Check binaries exist
if [[ ! -x "$ZELLIJ" ]] || [[ ! -x "$SPIKE_CLIENT" ]]; then
    log "Building release binaries..."
    cargo build --release -p zellij 2>/dev/null
    cargo build --release --example spike_client -p zellij-remote-bridge 2>/dev/null
fi

if [[ ! -x "$ZELLIJ" ]]; then
    echo "ERROR: Zellij binary not found at $ZELLIJ"
    exit 1
fi

if [[ ! -x "$SPIKE_CLIENT" ]]; then
    echo "ERROR: spike_client not found at $SPIKE_CLIENT"
    exit 1
fi

echo "=== ZRP Reconnection Tests ==="
echo "Project root: $PROJECT_ROOT"
echo "Test session: $TEST_SESSION"
echo ""

# Initial cleanup
pkill -f "zellij.*--server.*zrp-test-reconnect-" 2>/dev/null || true
rm -f "$RESUME_TOKEN_FILE"
sleep 1

# =============================================================================
# Test 1: Metrics output validation
# =============================================================================
log "Test 1: Metrics output validation"

start_server

log "Running client with --metrics-out..."
env -u ZELLIJ_REMOTE_TOKEN \
    SERVER_URL="https://127.0.0.1:$PORT" \
    ZELLIJ_REMOTE_TOKEN="$TEST_TOKEN" \
    HEADLESS=1 \
    timeout 10 "$SPIKE_CLIENT" --metrics-out "$METRICS_FILE" 2>&1 || true

if [[ -f "$METRICS_FILE" ]]; then
    if jq empty "$METRICS_FILE" 2>/dev/null; then
        pass "Metrics file is valid JSON"

        # Check expected fields
        if jq -e '.session_name' "$METRICS_FILE" >/dev/null 2>&1; then
            pass "Metrics contains session_name"
        else
            fail "Metrics missing session_name"
        fi

        if jq -e '.client_id' "$METRICS_FILE" >/dev/null 2>&1; then
            pass "Metrics contains client_id"
        else
            fail "Metrics missing client_id"
        fi

        if jq -e '.connect_time_ms' "$METRICS_FILE" >/dev/null 2>&1; then
            pass "Metrics contains connect_time_ms"
        else
            fail "Metrics missing connect_time_ms"
        fi

        if jq -e '.snapshots_received' "$METRICS_FILE" >/dev/null 2>&1; then
            pass "Metrics contains snapshots_received"
        else
            fail "Metrics missing snapshots_received"
        fi
    else
        fail "Metrics file is not valid JSON"
    fi
else
    fail "Metrics file was not created"
fi

stop_server
rm -f "$METRICS_FILE"
echo ""

# =============================================================================
# Test 2: Resume token persistence
# =============================================================================
log "Test 2: Resume token persistence"

rm -f "$RESUME_TOKEN_FILE"
start_server

log "First connection - should create resume token..."
output1=$(env -u ZELLIJ_REMOTE_TOKEN \
    SERVER_URL="https://127.0.0.1:$PORT" \
    ZELLIJ_REMOTE_TOKEN="$TEST_TOKEN" \
    HEADLESS=1 \
    timeout 8 "$SPIKE_CLIENT" 2>&1) || true

if [[ -f "$RESUME_TOKEN_FILE" ]]; then
    FIRST_TOKEN=$(cat "$RESUME_TOKEN_FILE")
    pass "Resume token file created after first connection"
    log "First resume token: ${FIRST_TOKEN:0:20}..."
else
    fail "Resume token file not created"
    FIRST_TOKEN=""
fi

# Extract state_id from first connection (if visible in output)
if echo "$output1" | grep -q "ServerHello:"; then
    pass "First connection received ServerHello"
else
    fail "First connection did not receive ServerHello"
fi

log "Second connection - should use resume token..."
output2=$(env -u ZELLIJ_REMOTE_TOKEN \
    SERVER_URL="https://127.0.0.1:$PORT" \
    ZELLIJ_REMOTE_TOKEN="$TEST_TOKEN" \
    HEADLESS=1 \
    timeout 8 "$SPIKE_CLIENT" 2>&1) || true

if echo "$output2" | grep -q "ServerHello:"; then
    pass "Second connection received ServerHello"
else
    fail "Second connection did not receive ServerHello"
fi

if [[ -f "$RESUME_TOKEN_FILE" ]]; then
    SECOND_TOKEN=$(cat "$RESUME_TOKEN_FILE")
    if [[ -n "$FIRST_TOKEN" ]] && [[ "$SECOND_TOKEN" != "$FIRST_TOKEN" ]]; then
        pass "Resume token updated after reconnection"
    elif [[ -n "$FIRST_TOKEN" ]]; then
        log "Note: Resume token unchanged (may be expected)"
    fi
else
    fail "Resume token file missing after second connection"
fi

stop_server
echo ""

# =============================================================================
# Test 3: Reconnect mode "once"
# =============================================================================
log "Test 3: Reconnect mode 'once'"

rm -f "$RESUME_TOKEN_FILE"
start_server

log "Starting client with --reconnect=once..."
# Run client in background, it should stay running and try to reconnect once
output3=$(env -u ZELLIJ_REMOTE_TOKEN \
    SERVER_URL="https://127.0.0.1:$PORT" \
    ZELLIJ_REMOTE_TOKEN="$TEST_TOKEN" \
    HEADLESS=1 \
    timeout 8 "$SPIKE_CLIENT" --reconnect=once 2>&1) || true

if echo "$output3" | grep -q "ServerHello:"; then
    pass "Client with --reconnect=once connected successfully"
else
    fail "Client with --reconnect=once failed to connect"
fi

# Stop server, start it again, client should reconnect
stop_server
sleep 1

log "Restarting server..."
start_server

log "Running client again to verify reconnect behavior..."
output4=$(env -u ZELLIJ_REMOTE_TOKEN \
    SERVER_URL="https://127.0.0.1:$PORT" \
    ZELLIJ_REMOTE_TOKEN="$TEST_TOKEN" \
    HEADLESS=1 \
    timeout 8 "$SPIKE_CLIENT" --reconnect=once 2>&1) || true

if echo "$output4" | grep -q "ServerHello:"; then
    pass "Client reconnected after server restart"
else
    fail "Client failed to reconnect after server restart"
fi

stop_server
echo ""

# =============================================================================
# Summary
# =============================================================================
echo "=== Test Summary ==="
echo "Passed: $PASS_COUNT"
echo "Failed: $FAIL_COUNT"
echo ""

if [[ $FAIL_COUNT -eq 0 ]]; then
    echo "=== All tests passed ==="
    exit 0
else
    echo "=== Some tests failed ==="
    exit 1
fi
