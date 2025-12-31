#!/bin/bash
# shellcheck disable=SC2329  # cleanup() is invoked via trap
# test-basic.sh - Basic connectivity test for ZRP
# Uses unique session names to avoid killing user's real Zellij sessions
set -euo pipefail

PROJECT_ROOT="${1:-$(dirname "$(dirname "$(dirname "$(realpath "$0")")")")}"
ZELLIJ="$PROJECT_ROOT/target/release/zellij"
SPIKE_CLIENT="$PROJECT_ROOT/target/release/examples/spike_client"

# Use a unique session name with PID to avoid conflicts
TEST_SESSION="zrp-test-basic-$$"
TEST_TOKEN="test-token-$$"

echo "=== Starting local E2E test ==="
echo "Project root: $PROJECT_ROOT"
echo "Test session: $TEST_SESSION"

# Cleanup function - only kills our test session
cleanup() {
    echo "Cleaning up test session..."
    pkill -f "zellij.*--server.*$TEST_SESSION" 2>/dev/null || true
    pkill -f "zellij.*--server.*zrp-test-basic-" 2>/dev/null || true
    rm -rf "/tmp/zellij-$(id -u)/contract_version_1/$TEST_SESSION" 2>/dev/null || true
}
trap cleanup EXIT

# Initial cleanup of old test sessions only
pkill -f "zellij.*--server.*zrp-test-basic-" 2>/dev/null || true
sleep 1

# Start server
echo "Starting Zellij server..."
ZELLIJ_REMOTE_ADDR=127.0.0.1:4433 \
    ZELLIJ_REMOTE_TOKEN="$TEST_TOKEN" \
    "$ZELLIJ" --session "$TEST_SESSION" &

# Wait for server to be ready
echo "Waiting for server to start..."
for _ in {1..10}; do
    if lsof -i :4433 >/dev/null 2>&1; then
        echo "Server is listening on port 4433"
        break
    fi
    sleep 0.5
done

if ! lsof -i :4433 >/dev/null 2>&1; then
    echo "ERROR: Server did not start within 5 seconds"
    exit 1
fi

# Run headless client
echo "Running client..."
output=$(env -u ZELLIJ_REMOTE_TOKEN \
    SERVER_URL=https://127.0.0.1:4433 \
    ZELLIJ_REMOTE_TOKEN="$TEST_TOKEN" \
    HEADLESS=1 \
    timeout 15 "$SPIKE_CLIENT" 2>&1) || true

echo "$output"

# Check for success
if echo "$output" | grep -q "ServerHello:"; then
    echo ""
    echo "✓ Client connected successfully"
    if echo "$output" | grep -q "ScreenSnapshot:"; then
        echo "✓ Received screen snapshot"
    fi
    echo "=== Test completed successfully ==="
    exit 0
else
    echo ""
    echo "✗ Client failed to connect"
    exit 1
fi
