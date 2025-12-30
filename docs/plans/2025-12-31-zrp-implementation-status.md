# ZRP Implementation Status

**Last Updated:** 2024-12-31

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
| Phase 6 | Client-side Prediction | 🔲 Not Started |
| Phase 7 | Mobile Client Library | 🔲 Not Started |

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
    ├── session.rs        # RemoteSession (aggregates all state)
    └── tests/            # 91 tests including proptest

zellij-remote-bridge/     # WebTransport server
├── examples/
│   ├── spike_server.rs   # Test server with full input handling
│   └── spike_client.rs   # Interactive client with keyboard input
└── src/
    ├── framing.rs        # Length-prefixed protobuf framing
    ├── handshake.rs      # Generic over AsyncRead/AsyncWrite
    ├── server.rs         # wtransport-based server
    └── config.rs
```

## Test Coverage

**Total: 201+ tests**

| Package | Unit Tests | Integration Tests | Property-Based |
|---------|------------|-------------------|----------------|
| zellij-remote-protocol | 89 | - | - |
| zellij-remote-core | 85 | - | 6 (proptest) |
| zellij-remote-bridge | 15 | 6 | - |

### Key Test Categories

- **Protocol roundtrip**: All message types encode/decode correctly
- **Frame store**: Arc sharing, dirty tracking, resize edge cases
- **Delta engine**: Array length invariants, size mismatch handling
- **Backpressure**: Window tracking, ack handling, snapshot forcing
- **Lease**: State machine transitions, policies, viewer mode
- **Input**: Sequencing, deduplication, controller gating
- **RTT**: EWMA smoothing, RTO calculation
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

## Next Steps

### Immediate (High Value)

#### 1. Zellij Integration
Connect to real Zellij sessions:
- Hook into existing render pipeline output
- Parse ANSI sequences into FrameStore
- Route input events to PTY
- Attach to existing sessions by name

#### 2. Resume Tokens
True Mosh-style resumption:
- Server sends resume_token in ServerHello
- Client stores and sends on reconnect
- Server sends delta from last-acked state (not full snapshot)
- Requires: state history buffer

### Future

#### 3. Client-side Prediction (Phase 6)
Local echo for low-latency feel:
- Predict character echo
- Reconcile with server state
- Handle mispredictions gracefully

#### 4. Mobile Client Library (Phase 7)
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
