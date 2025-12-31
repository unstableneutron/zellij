# ZRP End-to-End Testing

Testing infrastructure for the Zellij Remote Protocol, supporting both local and Docker-based testing.

## Quick Start

```bash
cd zellij-remote-tests

# Build binaries
make build

# Run all tests
make test-all

# Individual tests
make test-local           # Basic connectivity
make test-local-auth      # Authentication (valid/invalid/no token)
make test-local-reconnect # Reconnection and metrics
```

## Docker Testing (for network emulation)

```bash
# Build Docker images
./run-tests.sh --build

# Run with high-latency network emulation
./run-tests.sh --profile high-rtt

# Interactive mode
./run-tests.sh --scenario interactive
```

## Guaranteed Cleanup

**All network emulation is automatically cleaned up**, regardless of how tests exit:
- Normal completion
- Ctrl+C / SIGINT
- Test failures
- Container crashes
- `docker compose down`

The cleanup is implemented via:
1. `trap` handlers in all shell scripts
2. Docker container lifecycle (stopping removes network namespace)
3. The `netem-wrapper.sh` script which cleans on any exit signal

## Test Scenarios

| Scenario | Description |
|----------|-------------|
| `basic` | Single client connects, receives updates, exits |
| `multi-client` | Two clients connect simultaneously |
| `reconnect` | Client connects, disconnects, reconnects with resume token |
| `stress` | Client under adverse network conditions |
| `interactive` | Server runs indefinitely for manual testing |

## Network Profiles

| Profile | Latency | Loss | Use Case |
|---------|---------|------|----------|
| `low-rtt` | 5ms | 0% | Local network simulation |
| `high-rtt` | 150ms ±10ms | 0.1% | Cross-continent (like sjc3) |
| `lossy` | 50ms ±5ms | 2% | Poor WiFi / mobile |
| `jittery` | 30ms ±20ms | 0% | Unstable connection |
| `satellite` | 300ms ±20ms | 0.5% | Satellite/extreme latency |

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                   Docker Network (172.28.0.0/24)            │
│                                                             │
│  ┌─────────────────┐         ┌─────────────────┐           │
│  │  zellij-server  │◄───────►│  spike-client   │           │
│  │  172.28.0.10    │  QUIC   │  172.28.0.20    │           │
│  │                 │         │                 │           │
│  │  - Zellij       │         │  - spike_client │           │
│  │  - Remote feat  │         │  - Headless     │           │
│  └─────────────────┘         └────────┬────────┘           │
│                                       │                     │
│                              ┌────────┴────────┐           │
│                              │  netem-sidecar  │ (optional) │
│                              │  (NET_ADMIN)    │           │
│                              │                 │           │
│                              │  tc netem rules │           │
│                              └─────────────────┘           │
└─────────────────────────────────────────────────────────────┘
```

## Manual Testing

### Connect to running server

```bash
# Start server in background
./run-tests.sh --scenario interactive

# In another terminal, run spike_client against the container
SERVER_URL=https://localhost:4433 \
ZELLIJ_REMOTE_TOKEN=test-token-12345 \
cargo run --example spike_client -p zellij-remote-bridge
```

### Debug container state

```bash
# View server logs
docker logs -f zrp-server

# Shell into server container
docker exec -it zrp-server bash

# Check network emulation status
docker exec zrp-netem tc qdisc show
```

## spike_client CLI

The test client supports various options for testing:

```bash
spike_client [OPTIONS]

OPTIONS:
    -s, --server-url <URL>     Server URL [default: https://127.0.0.1:4433]
    -t, --token <TOKEN>        Bearer token for authentication
        --headless             Run without terminal UI
        --script <FILE>        Script file for deterministic input
        --metrics-out <FILE>   Write JSON metrics on exit
        --reconnect <MODE>     Reconnect mode: none, once, always, after=Ns
        --clear-token          Clear stored resume token
```

### Script Format

```
# Comment
sleep 100        # Sleep for 100ms
type hello       # Type characters
key enter        # Send special key
key ctrl+c       # Key with modifier
reconnect        # Force reconnection
quit             # Exit client
```

### Metrics Output

```json
{
  "session_name": "test-session",
  "client_id": 1,
  "connect_time_ms": 45,
  "rtt_samples": [5, 6, 4],
  "snapshots_received": 1,
  "deltas_received": 10,
  "reconnect_count": 0
}
```

## Server-Side Test Knobs

Environment variables for fault injection:

| Variable | Description |
|----------|-------------|
| `ZELLIJ_REMOTE_DROP_DELTA_NTH=N` | Drop every Nth delta |
| `ZELLIJ_REMOTE_DELAY_SEND_MS=N` | Add N ms delay to sends |
| `ZELLIJ_REMOTE_FORCE_SNAPSHOT_EVERY=N` | Force snapshot every N frames |
| `ZELLIJ_REMOTE_LOG_FRAME_STATS=1` | Log frame statistics |

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `ZELLIJ_REMOTE_TOKEN` | `test-token-12345` | Bearer token for auth |
| `RUST_LOG` | `info` | Log level (debug, info, warn, error) |
| `NETEM_PROFILE` | (none) | Network emulation profile |

## Troubleshooting

### "Cannot connect to server"

1. Check server is running: `docker ps | grep zrp-server`
2. Check server logs: `docker logs zrp-server`
3. Verify network: `docker network inspect zellij-remote-tests_zrp-test-net`

### "Permission denied for tc"

The netem sidecar requires `CAP_NET_ADMIN`. This is set in docker-compose.yml.
If running manually, use: `docker run --cap-add NET_ADMIN ...`

### Cleanup didn't happen

Force cleanup with:
```bash
docker compose -f zellij-remote-tests/docker-compose.yml \
  --profile netem --profile multi-client \
  down --remove-orphans --volumes
```
