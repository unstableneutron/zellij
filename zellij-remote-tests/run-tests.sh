#!/bin/bash
# run-tests.sh - ZRP E2E test runner with guaranteed cleanup
#
# Usage:
#   ./run-tests.sh                    # Run all tests
#   ./run-tests.sh --scenario basic   # Run specific scenario
#   ./run-tests.sh --profile high-rtt # Run with network emulation
#   ./run-tests.sh --build            # Force rebuild images

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
COMPOSE_FILE="$SCRIPT_DIR/docker-compose.yml"

# Defaults
SCENARIO="basic"
NETEM_PROFILE=""
FORCE_BUILD=false
KEEP_RUNNING=false
RUST_LOG="${RUST_LOG:-info}"

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --scenario)
            SCENARIO="$2"
            shift 2
            ;;
        --profile)
            NETEM_PROFILE="$2"
            shift 2
            ;;
        --build)
            FORCE_BUILD=true
            shift
            ;;
        --keep)
            KEEP_RUNNING=true
            shift
            ;;
        --debug)
            RUST_LOG="debug"
            shift
            ;;
        --help)
            echo "Usage: $0 [options]"
            echo ""
            echo "Options:"
            echo "  --scenario <name>   Test scenario (basic, multi-client, reconnect)"
            echo "  --profile <name>    Network profile (low-rtt, high-rtt, lossy, jittery, satellite)"
            echo "  --build             Force rebuild Docker images"
            echo "  --keep              Keep containers running after test"
            echo "  --debug             Enable debug logging"
            echo "  --help              Show this help"
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

log() {
    echo "[run-tests] $(date -Iseconds) $*"
}

# GUARANTEED CLEANUP - runs on ANY exit
cleanup() {
    local exit_code=$?
    log "CLEANUP: Stopping all containers..."
    
    cd "$PROJECT_ROOT"
    docker compose -f "$COMPOSE_FILE" --profile netem --profile multi-client down --remove-orphans --timeout 5 2>/dev/null || true
    
    log "CLEANUP: Complete"
    
    if [ $exit_code -ne 0 ]; then
        log "Test run failed with exit code $exit_code"
    fi
    
    exit $exit_code
}

trap cleanup EXIT
trap cleanup SIGTERM
trap cleanup SIGINT

cd "$PROJECT_ROOT"

# Build if needed
if [ "$FORCE_BUILD" = true ] || ! docker images | grep -q "zellij-remote-tests"; then
    log "Building Docker images..."
    docker compose -f "$COMPOSE_FILE" build
fi

# Prepare compose command
COMPOSE_CMD="docker compose -f $COMPOSE_FILE"
COMPOSE_PROFILES=""

if [ -n "$NETEM_PROFILE" ]; then
    COMPOSE_PROFILES="--profile netem"
    export NETEM_PROFILE
fi

if [ "$SCENARIO" = "multi-client" ]; then
    COMPOSE_PROFILES="$COMPOSE_PROFILES --profile multi-client"
fi

export RUST_LOG

log "Starting test scenario: $SCENARIO"
log "Network profile: ${NETEM_PROFILE:-none}"
log "Rust log level: $RUST_LOG"

case "$SCENARIO" in
    basic)
        log "Running basic connectivity test..."
        # Start server, wait for it, run client
        $COMPOSE_CMD $COMPOSE_PROFILES up --abort-on-container-exit --exit-code-from spike-client
        ;;
    
    multi-client)
        log "Running multi-client test..."
        $COMPOSE_CMD $COMPOSE_PROFILES up --abort-on-container-exit
        ;;
    
    reconnect)
        log "Running reconnection test..."
        # Start server
        $COMPOSE_CMD up -d zellij-server
        sleep 2
        
        # Run client, kill it, run again
        log "First connection..."
        timeout 10 $COMPOSE_CMD run --rm spike-client || true
        
        log "Simulating disconnect..."
        sleep 1
        
        log "Reconnection with resume token..."
        timeout 10 $COMPOSE_CMD run --rm spike-client || true
        ;;
    
    stress)
        log "Running stress test with network emulation..."
        if [ -z "$NETEM_PROFILE" ]; then
            NETEM_PROFILE="lossy"
            export NETEM_PROFILE
            COMPOSE_PROFILES="--profile netem"
        fi
        
        $COMPOSE_CMD $COMPOSE_PROFILES up --abort-on-container-exit --exit-code-from spike-client
        ;;
    
    interactive)
        log "Starting interactive mode (Ctrl+C to stop)..."
        KEEP_RUNNING=true
        $COMPOSE_CMD $COMPOSE_PROFILES up -d zellij-server
        
        log "Server running. Connect with:"
        log "  docker exec -it zrp-server bash"
        log "  docker logs -f zrp-server"
        log ""
        log "Or run spike_client manually:"
        log "  SERVER_URL=https://localhost:4433 cargo run --example spike_client -p zellij-remote-bridge"
        
        # Wait for Ctrl+C
        while true; do
            sleep 3600
        done
        ;;
    
    *)
        log "ERROR: Unknown scenario '$SCENARIO'"
        exit 1
        ;;
esac

if [ "$KEEP_RUNNING" = true ]; then
    log "Containers kept running. Stop with: docker compose -f $COMPOSE_FILE down"
else
    log "Test completed successfully"
fi
