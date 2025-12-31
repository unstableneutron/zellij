#!/bin/bash
# netem-wrapper.sh - Network emulation with GUARANTEED cleanup
#
# This script applies tc netem rules and ensures they are removed on exit,
# regardless of how the script terminates (normal exit, SIGTERM, SIGINT, etc.)
#
# Usage:
#   netem-wrapper.sh <interface> <profile> [duration_seconds]
#
# Profiles:
#   clean      - No emulation (restore to normal)
#   low-rtt    - 5ms delay, no loss (simulates local network)
#   high-rtt   - 150ms delay, 0.1% loss (simulates cross-continent)
#   lossy      - 50ms delay, 2% loss, 1% reorder
#   jittery    - 30ms delay ±20ms jitter
#   satellite  - 300ms delay, 0.5% loss (simulates satellite link)
#   custom     - Read from NETEM_OPTS environment variable

set -euo pipefail

INTERFACE="${1:-eth0}"
PROFILE="${2:-clean}"
DURATION="${3:-0}"  # 0 = run until killed

# Track if we've already cleaned up
CLEANED_UP=false

log() {
    echo "[netem-wrapper] $(date -Iseconds) $*"
}

cleanup() {
    if [ "$CLEANED_UP" = true ]; then
        return
    fi
    CLEANED_UP=true
    
    log "CLEANUP: Removing all tc qdisc rules from $INTERFACE"
    
    # Remove any existing qdisc - ignore errors if none exists
    tc qdisc del dev "$INTERFACE" root 2>/dev/null || true
    
    # Verify cleanup succeeded
    if tc qdisc show dev "$INTERFACE" | grep -q "netem"; then
        log "ERROR: Failed to remove netem rules!"
        exit 1
    fi
    
    log "CLEANUP: Complete - $INTERFACE restored to normal"
}

# Register cleanup for ALL exit scenarios
trap cleanup EXIT
trap cleanup SIGTERM
trap cleanup SIGINT
trap cleanup SIGHUP
trap cleanup SIGQUIT

apply_profile() {
    local profile="$1"
    
    # Always clean first
    tc qdisc del dev "$INTERFACE" root 2>/dev/null || true
    
    case "$profile" in
        clean)
            log "Profile 'clean': No emulation applied"
            ;;
        low-rtt)
            log "Profile 'low-rtt': 5ms delay, no loss"
            tc qdisc add dev "$INTERFACE" root netem delay 5ms
            ;;
        high-rtt)
            log "Profile 'high-rtt': 150ms delay, 0.1% loss"
            tc qdisc add dev "$INTERFACE" root netem delay 150ms 10ms loss 0.1%
            ;;
        lossy)
            log "Profile 'lossy': 50ms delay, 2% loss, 1% reorder"
            tc qdisc add dev "$INTERFACE" root netem delay 50ms 5ms loss 2% reorder 1% 50%
            ;;
        jittery)
            log "Profile 'jittery': 30ms delay ±20ms jitter"
            tc qdisc add dev "$INTERFACE" root netem delay 30ms 20ms distribution normal
            ;;
        satellite)
            log "Profile 'satellite': 300ms delay, 0.5% loss"
            tc qdisc add dev "$INTERFACE" root netem delay 300ms 20ms loss 0.5%
            ;;
        custom)
            if [ -z "${NETEM_OPTS:-}" ]; then
                log "ERROR: Profile 'custom' requires NETEM_OPTS environment variable"
                exit 1
            fi
            log "Profile 'custom': $NETEM_OPTS"
            # shellcheck disable=SC2086
            tc qdisc add dev "$INTERFACE" root netem $NETEM_OPTS
            ;;
        *)
            log "ERROR: Unknown profile '$profile'"
            log "Available profiles: clean, low-rtt, high-rtt, lossy, jittery, satellite, custom"
            exit 1
            ;;
    esac
    
    # Show current state
    log "Current qdisc configuration:"
    tc qdisc show dev "$INTERFACE"
}

# Verify we have permissions
if ! tc qdisc show dev "$INTERFACE" >/dev/null 2>&1; then
    log "ERROR: Cannot access interface $INTERFACE or insufficient permissions"
    log "This script requires NET_ADMIN capability"
    exit 1
fi

log "Starting netem wrapper on interface $INTERFACE"
log "Profile: $PROFILE, Duration: ${DURATION}s (0=forever)"

apply_profile "$PROFILE"

if [ "$DURATION" -gt 0 ]; then
    log "Will run for $DURATION seconds then cleanup"
    sleep "$DURATION"
    log "Duration complete"
else
    log "Running until terminated (Ctrl+C or SIGTERM)"
    # Wait forever - cleanup will happen on signal
    while true; do
        sleep 3600
    done
fi
