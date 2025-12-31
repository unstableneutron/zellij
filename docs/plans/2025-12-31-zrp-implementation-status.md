# ZRP Implementation Status

**Last Updated:** 2025-01-01 (Delta Optimization & Client Routing Fix)

## Overview

The Zellij Remote Protocol (ZRP) enables Mosh-style remote terminal access over WebTransport/QUIC. This document tracks implementation progress and learnings.

## Implementation Status

| Phase | Description | Status |
|-------|-------------|--------|
| Phase 0 | Repository & Build Foundations | ✅ Complete |
| Phase 1 | Core State Management | ✅ Complete |
| Phase 2 | WebTransport Server | ✅ Complete |
| Phase 2.5 | End-to-End Render Demo | ✅ Complete |
| Phase 3 | Backpressure & Flow Control | ✅ Complete |
| Phase 4 | Controller Lease | ✅ Complete |
| Phase 5 | Input Handling | ✅ Complete |
| Phase 6 | Client-side Prediction | ✅ Complete |
| Phase 7 | Zellij Integration | ✅ Complete |
| Phase 7.5 | Full E2E Wiring | ✅ Complete |
| Phase 7.6 | E2E Testing Infrastructure | ✅ Complete |
| Phase 7.7 | Delta Optimization | ✅ Complete |
| Phase 8 | Mobile Client Library | 🔲 Not Started |

## Crate Structure

```
zellij-remote-protocol/   # Protobuf definitions (prost)
├── proto/zellij_remote.proto
├── build.rs              # prost-build codegen
└── src/
    ├── lib.rs
    └── tests.rs          # 89 roundtrip tests

zellij-remote-core/       # State management
└── src/
    ├── frame.rs          # FrameStore with Arc<Row> sharing
    ├── style_table.rs    # O(1) style lookup
    ├── delta.rs          # DeltaEngine (cumulative deltas)
    ├── render_seq.rs     # Latest-wins datagram semantics
    ├── backpressure.rs   # RenderWindow for flow control
    ├── client_state.rs   # ClientRenderState (per-client baselines)
    ├── lease.rs          # LeaseManager (controller lease state machine)
    ├── input.rs          # InputReceiver/InputSender (reliable input)
    ├── rtt.rs            # RttEstimator (EWMA RTT estimation)
    ├── prediction.rs     # PredictionEngine (local echo, reconciliation)
    ├── session.rs        # RemoteSession (aggregates all state)
    └── tests/            # 134 tests including proptest

zellij-remote-bridge/     # WebTransport server
├── examples/
│   ├── spike_server.rs   # Test server with full input handling
│   └── spike_client.rs   # Interactive client with CLI, metrics, reconnection
└── src/
    ├── framing.rs        # Length-prefixed protobuf framing
    ├── handshake.rs      # Generic over AsyncRead/AsyncWrite
    ├── server.rs         # wtransport-based server
    └── config.rs

zellij-remote-tests/      # E2E testing infrastructure
├── Makefile              # Test runner
├── scripts/
│   ├── test-basic.sh     # Basic connectivity test
│   ├── test-auth.sh      # Authentication tests
│   ├── test-reconnect.sh # Reconnection and metrics tests
│   └── netem-wrapper.sh  # Network emulation with cleanup
├── docker-compose.yml    # Docker-based testing
└── Dockerfile            # Multi-stage build for containers
```

## Test Coverage

**Total: 244 tests**

| Package | Unit Tests | Integration Tests | Property-Based |
|---------|------------|-------------------|----------------|
| zellij-remote-protocol | 89 | - | - |
| zellij-remote-core | 134 | - | 6 (proptest) |
| zellij-remote-bridge | 15 | 6 | - |

### Key Test Categories

- **Protocol roundtrip**: All message types encode/decode correctly
- **Frame store**: Arc sharing, dirty tracking, resize edge cases
- **Delta engine**: Array length invariants, size mismatch handling
- **Backpressure**: Window tracking, ack handling, snapshot forcing
- **Lease**: State machine transitions, policies, viewer mode
- **Input**: Sequencing, deduplication, controller gating
- **RTT**: EWMA smoothing, RTO calculation
- **Prediction**: Local echo, ack reconciliation, misprediction correction
- **Session**: Multi-client, baseline advancement
- **Framing**: Partial reads, multiple frames, corruption handling
- **Handshake**: Success, errors, capability negotiation

## Verified Scenarios

### Local Testing
```bash
# Terminal 1 - Server
RUST_LOG=info cargo run --example spike_server -p zellij-remote-bridge

# Terminal 2 - Interactive client with keyboard input
cargo run --example spike_client -p zellij-remote-bridge

# Or headless mode for testing
HEADLESS=1 cargo run --example spike_client -p zellij-remote-bridge
```

### Cross-Machine (Tailscale)
Successfully tested Mac → Ubuntu aarch64 over Tailscale:

```bash
# On remote Linux (sjc3)
LISTEN_ADDR=0.0.0.0:4433 ./spike_server

# On local Mac
SERVER_URL="https://100.69.153.168:4433" cargo run --example spike_client -p zellij-remote-bridge
```

**Result:** Full render + input pipeline works over Tailscale mesh.

### Network Resilience Testing

| Scenario | Result |
|----------|--------|
| Client disconnect mid-stream | ✅ Server continues, logs warning |
| Reconnection after disconnect | ✅ Client gets current state (higher state_id) |
| Session persistence | ✅ Background updates continue without clients |
| Multiple clients | ✅ Each gets unique client_id, viewers receive updates |
| Cross-machine reconnect | ✅ Mac → sjc3, state_id 6→19 after 3s gap |
| Input from controller | ✅ Echoed to screen |
| Input from viewer | ✅ Rejected with NotController error |

## Build Requirements

### Local Development
- Rust 1.70+
- No additional dependencies (prost-build bundles protoc)

### Remote/Cross-Compilation
Building on remote Linux machines requires:
```bash
apt-get install protobuf-compiler  # For prost-build
```

## Key Learnings

### 1. WebTransport over Tailscale Works
- QUIC/UDP passes through Tailscale's WireGuard tunnel
- Direct connections established (not DERP relay in our test)
- Self-signed certs work with `with_no_cert_validation()`

### 2. Testable Architecture
- Handshake extracted to generic `run_handshake<R, W>()` function
- Testable with `tokio::io::duplex()` without real network
- Framing logic separated from transport

### 3. Arc<Row> Sharing
- Unchanged rows share Arc pointers across snapshots
- Delta computation uses `Arc::ptr_eq()` for O(1) comparison
- Copy-on-write via `Arc::make_mut()` on modification

### 4. Ack-Driven Baselines (Critical Fix)
- Delta baselines are only advanced on StateAck receipt
- Prevents "delta chain" issues when datagrams are lost
- Pending frames tracked until acknowledged

### 5. Controller Lease Model
- Only one client can control resize/input at a time
- ExplicitOnly vs LastWriterWins policies
- Viewers receive updates but cannot send input
- Lease expiration without keepalive

### 6. Per-Client Input Tracking
- Each client has independent input sequence numbers
- Controller gating prevents unauthorized input
- RTT estimation via echoed timestamps

### 7. Resume Tokens
- Server generates unique resume tokens per client
- Tokens sent in ServerHello for client storage
- ClientHello includes token for session resumption
- Enables reconnection to same session with state continuity

### 8. Client-side Prediction
- Local echo for printable characters provides instant feedback
- Pending predictions tracked until server acknowledgment
- Ack-driven reconciliation clears confirmed predictions
- Misprediction detection with automatic correction
- Non-echoing modes (password prompts) disable prediction

## Phase 7.5: Full E2E Wiring (Completed)

Added the final pieces to connect everything end-to-end:

### WebTransport Server in Remote Thread
- Remote thread now spawns WebTransport server on `ZELLIJ_REMOTE_ADDR`
- Handles client connections with handshake (ClientHello/ServerHello)
- Manages multiple concurrent WebTransport clients

### Input Routing to Zellij
- Input events from WebTransport clients are translated via `input_translate.rs`
- Translated `Action::Write` is sent as `ScreenInstruction::WriteCharacter`
- Uses `to_screen` sender passed via `RemoteConfig`

### StyleTable Consistency
- `StyleTable` is now included in `RemoteInstruction::FrameReady`
- Remote thread receives fresh style mappings with each frame
- Enables proper delta computation with consistent style IDs

### Running with Real Zellij

```bash
# Terminal 1 - Start Zellij with remote enabled (localhost)
ZELLIJ_REMOTE_ADDR=127.0.0.1:4433 cargo run --features remote

# Terminal 2 - Connect with spike_client
cargo run --example spike_client -p zellij-remote-bridge
```

## Post-Implementation Review Fixes (2024-12-31)

Following an Oracle review, all HIGH and MEDIUM priority issues were addressed:

### Security Fixes (HIGH Priority)

| Issue | Fix |
|-------|-----|
| **No authentication** | Added bearer token auth via `ZELLIJ_REMOTE_TOKEN` env var |
| **Bind address ignored** | Changed to `with_bind_address()` to respect full IP:port |
| **Lease not enforced** | Added lease check before processing input; non-controllers get `LEASE_DENIED` |
| **Unbounded frame buffering** | Added `MAX_FRAME_SIZE = 1MB`; oversized frames rejected |
| **Hardcoded client_id=1** | Track active Zellij client; route input correctly |

### Architecture Fixes (MEDIUM Priority)

| Issue | Fix |
|-------|-----|
| **Head-of-line blocking** | Per-client send tasks with bounded `mpsc` channels |
| **Lock contention** | Clone data before releasing lock; no I/O while holding locks |
| **Blocking recv per-iteration** | Single dedicated blocking thread forwarding to async channel |
| **Errors swallowed** | Log all errors; track failed sends; disconnect after 3 failures |
| **Cleanup gaps** | Added `ClientGuard` with `Drop` for automatic cleanup |
| **Architecture duplication** | Removed duplicate `RemoteSession`; all access via `RemoteManager` |

### Environment Variables

| Variable | Description |
|----------|-------------|
| `ZELLIJ_REMOTE_ADDR` | Address to bind (e.g., `127.0.0.1:4433` or `0.0.0.0:4433`) |
| `ZELLIJ_REMOTE_TOKEN` | Bearer token for authentication (recommended for non-loopback) |
| `ZELLIJ_REMOTE_ENABLE` | Enable remote without specifying address (uses default) |

## Phase 7.6: E2E Testing Infrastructure (Completed)

Added comprehensive E2E testing infrastructure:

### spike_client CLI Enhancements
- Full CLI argument parsing with clap (replaces env vars)
- `--server-url`, `--token`, `--headless`, `--clear-token`
- `--script <file>`: Deterministic input replay
- `--metrics-out <file>`: JSON metrics output
- `--reconnect <mode>`: `none`, `once`, `always`, `after=Ns`

### Server-Side Test Knobs
Environment variables for fault injection:
- `ZELLIJ_REMOTE_DROP_DELTA_NTH=N`: Drop every Nth delta
- `ZELLIJ_REMOTE_DELAY_SEND_MS=N`: Add latency to sends
- `ZELLIJ_REMOTE_FORCE_SNAPSHOT_EVERY=N`: Force snapshots
- `ZELLIJ_REMOTE_LOG_FRAME_STATS=1`: Log frame statistics

### Test Scripts
All tests use unique session names (`zrp-test-*-$$`) to avoid killing user sessions:
- `test-basic.sh`: Basic connectivity
- `test-auth.sh`: Authentication (valid/invalid/no token)
- `test-reconnect.sh`: Reconnection, resume tokens, metrics validation

### Running E2E Tests
```bash
cd zellij-remote-tests
make build        # Build binaries
make test-all     # Run all tests
```

## Phase 7.7: Delta Optimization (Completed 2025-01-01)

Reduced screen delta sizes from ~9KB to fit within QUIC datagrams (<1200 bytes).

### Optimizations Implemented

1. **dirty_rows tracking**: Only process rows marked dirty by FrameStore, instead of comparing all 24 rows
2. **Intra-row diffing**: Emit sparse `CellRun`s containing only changed columns, instead of full 80-cell rows
3. **dirty_rows caching**: Cache dirty_rows per state_id in RemoteSession so all clients reuse the same set

### Critical Integration Fix

During E2E testing, discovered `RemoteInstruction::ClientConnected` was never sent from screen.rs, causing `active_zellij_client` to remain `None` and all remote input to be dropped.

**Fix:** Added notifications in `zellij-server/src/screen.rs`:
- `Screen::add_client()` sends `RemoteInstruction::ClientConnected`
- `Screen::remove_client()` sends `RemoteInstruction::ClientDisconnected` and auto-selects next client

### E2E Validation Results

```json
{
  "deltas_received": 3,
  "deltas_via_datagram": 2,
  "deltas_via_stream": 1
}
```

- ✅ 2 out of 3 deltas fit in QUIC datagrams
- ✅ Large output correctly falls back to stream
- ✅ 141 unit tests pass

### StateAck Implementation (2025-01-01)

Fixed base_mismatch issue causing excessive snapshot resyncs:

**Root Cause:** Server's `acked_baseline_state_id` never advanced because:
1. Client (spike_client) never sent StateAck after applying deltas
2. Server never received datagrams to process StateAck

**Fixes:**
- Client: Added `send_state_ack()` after snapshot/delta application
- Server: Added `spawn_datagram_receive_task()` to receive and route StateAck
- Added monotonicity guard for stream deltas (prevent state regression)
- Added task lifecycle management (abort on client disconnect)
- Use `try_send` for ack forwarding (non-blocking)

**E2E Validation Results:**
| Metric | Before | After |
|--------|--------|-------|
| base_mismatches | 3 | 0 |
| deltas_received | 3 | 12 |
| deltas_via_datagram | 2 | 11 |
| snapshots_received | 3 | 1 |

### Resize Handling Wiring (2025-01-01)

Wired up previously unwired RemoteInstruction variants:

**Issues Fixed:**
1. `RemoteInstruction::ClientResize` - now sent from screen.rs on TerminalResize
2. `SetControllerSize` from remote clients - now handled (was ignored)
3. `RemoteInstruction::Shutdown` - now sent on KillSession

**Design Decisions:**
- ClientResize/SetControllerSize don't resize frame_store directly - they're notifications
- FrameReady handler detects dimension changes and does full copy
- This prevents race conditions where resize happens before content arrives
- SetControllerSize dimensions clamped to 500x500 max to prevent DoS
- Resize is a viewport hint; doesn't update lease current_size

## Cross-Machine Validation Checklist

For validating ZRP over real networks (Tailscale, WAN):

### Setup
```bash
# Server (remote Linux)
ZELLIJ_REMOTE_ADDR=0.0.0.0:4433 ZELLIJ_REMOTE_TOKEN=<token> \
  cargo run --release --features remote

# Client (local Mac)
cargo run --release --example spike_client -p zellij-remote-bridge -- \
  --server-url https://<tailscale-ip>:4433 \
  --token <token> \
  --metrics-out /tmp/cross-machine-metrics.json
```

### Validation Points
| Check | Success Criteria |
|-------|------------------|
| Datagrams viable | `deltas_via_datagram` >> `deltas_via_stream` |
| No excessive resyncs | `base_mismatches` ≈ 0, `snapshots_received` ≈ 1 |
| 0-RTT works | Reconnect time < 100ms on `--reconnect=once` |
| Delta sizing | Keystroke deltas < 1200 bytes |
| Input latency | RTT samples stable, no stalls |

### Network Considerations
- Tailscale MTU is lower than LAN; 1200-byte datagram cap is conservative
- DERP relay vs direct affects latency
- UDP may be degraded on some enterprise networks

### Cross-Machine Test Results (2025-01-01)

Tested Mac (local) → sjc3 (Oracle Cloud aarch64) over Tailscale:

```json
{
  "connect_time_ms": 559,
  "rtt_min_ms": 183,
  "rtt_avg_ms": 183.3,
  "deltas_received": 51,
  "base_mismatches": 0,
  "snapshots_received": 1,
  "inputs_sent": 27,
  "inputs_acked": 27
}
```

**Validated:**
- ✅ Connection over Tailscale (direct, not DERP)
- ✅ StateAck working (base_mismatches: 0)
- ✅ Stable RTT (~183ms to Oracle Cloud)
- ✅ Input routing works over WAN
- ✅ Delta streaming with no resyncs

**Note:** spike_server sends deltas via stream only; datagram delivery requires full Zellij server.

## Authentication UX Improvements (2025-01-01)

Enhanced authentication flow for better security and usability:

**Server-side:**
- Sends structured `ProtocolError` before closing on auth failure
- Constant-time token comparison using `subtle` crate (prevents timing attacks)
- Empty `ZELLIJ_REMOTE_TOKEN` treated as no auth (with warning)
- Stream properly flushed before connection close

**Client-side (spike_client):**
- New `--token-file` flag with Unix permission check (requires 0600)
- Token precedence: `--token` > `--token-file` > `ZELLIJ_REMOTE_TOKEN`
- Clear error message on auth failure: "Check your --token, --token-file, or ZELLIJ_REMOTE_TOKEN"
- Handles `ProtocolError` messages from server

## Next Steps

### Phase 8: Mobile Client Library (Future)

UniFFI bindings for iOS/Android:
- Swift/Kotlin wrappers
- Native UI rendering
- Background connection handling

## Architecture Decisions

See [2024-12-30-zellij-remote-protocol-v2.md](./2024-12-30-zellij-remote-protocol-v2.md) for detailed design rationale.

Key decisions:
- **Input**: Reliable QUIC streams (not datagrams) for exactly-once delivery
- **Render**: Datagrams for small deltas, stream fallback for large
- **State**: Per-client ack-driven baselines, cumulative deltas
- **Lease**: Controller model for resize/input coordination
- **Prediction**: Deferred until correctness proven
