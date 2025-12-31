#!/bin/bash
# test-auth.sh - Test authentication for ZRP
set -euo pipefail

PROJECT_ROOT="${1:-$(dirname "$(dirname "$(dirname "$(realpath "$0")")")")}"
ZELLIJ="$PROJECT_ROOT/target/release/zellij"
SPIKE_CLIENT="$PROJECT_ROOT/target/release/examples/spike_client"

# Use a unique session name to avoid killing user's real sessions
TEST_SESSION="zrp-test-auth-$$"

echo "=== Testing authentication ==="
echo "Project root: $PROJECT_ROOT"
echo "Test session: $TEST_SESSION"

# Cleanup function - only kills our test session
cleanup() {
    echo "Cleaning up test session..."
    # Kill only the server process for our specific test session
    pkill -f "zellij.*--server.*$TEST_SESSION" 2>/dev/null || true
    # Also clean up any orphaned test sessions from previous runs
    pkill -f "zellij.*--server.*zrp-test-auth-" 2>/dev/null || true
    rm -rf "/tmp/zellij-$(id -u)/contract_version_1/$TEST_SESSION" 2>/dev/null || true
}
trap cleanup EXIT

# Initial cleanup of old test sessions only
pkill -f "zellij.*--server.*zrp-test-auth-" 2>/dev/null || true
sleep 1

# Start server with token required
echo "Starting Zellij server with token authentication..."
ZELLIJ_REMOTE_ADDR=127.0.0.1:4433 \
    ZELLIJ_REMOTE_TOKEN=secret-token-xyz \
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

TESTS_PASSED=0
TESTS_FAILED=0

run_client_test() {
    local name="$1"
    local token="$2"
    local expect_success="$3"

    echo ""
    echo "Test: $name"

    local output
    if [ -n "$token" ]; then
        output=$(env -u ZELLIJ_REMOTE_TOKEN \
            SERVER_URL=https://127.0.0.1:4433 \
            ZELLIJ_REMOTE_TOKEN="$token" \
            HEADLESS=1 \
            timeout 5 "$SPIKE_CLIENT" 2>&1) || true
    else
        output=$(env -u ZELLIJ_REMOTE_TOKEN \
            SERVER_URL=https://127.0.0.1:4433 \
            HEADLESS=1 \
            timeout 5 "$SPIKE_CLIENT" 2>&1) || true
    fi

    # Check if we got "ServerHello:" which indicates successful auth
    if echo "$output" | grep -q "ServerHello:"; then
        if [ "$expect_success" = "true" ]; then
            echo "  ✓ PASS: Got ServerHello as expected"
            TESTS_PASSED=$((TESTS_PASSED + 1))
        else
            echo "  ✗ FAIL: Got ServerHello but should have been rejected"
            echo "  Output: $output"
            TESTS_FAILED=$((TESTS_FAILED + 1))
        fi
    else
        if [ "$expect_success" = "false" ]; then
            echo "  ✓ PASS: Connection rejected (no ServerHello)"
            TESTS_PASSED=$((TESTS_PASSED + 1))
        else
            echo "  ✗ FAIL: No ServerHello but expected success"
            echo "  Output: $output"
            TESTS_FAILED=$((TESTS_FAILED + 1))
        fi
    fi
}

# Run tests
run_client_test "No token (should fail)" "" "false"
run_client_test "Wrong token (should fail)" "wrong-token" "false"
run_client_test "Correct token (should succeed)" "secret-token-xyz" "true"

echo ""
echo "=== Results ==="
echo "Passed: $TESTS_PASSED"
echo "Failed: $TESTS_FAILED"

if [ "$TESTS_FAILED" -gt 0 ]; then
    exit 1
fi

echo "=== All auth tests passed ==="
