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
| Phase 3 | Backpressure & Flow Control | 🔲 Not Started |
| Phase 4 | Controller Lease | 🔲 Not Started |
| Phase 5 | Input Handling | 🔲 Not Started |
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
    └── tests/            # 41 tests including proptest

zellij-remote-bridge/     # WebTransport server
├── examples/
│   ├── spike_server.rs   # Test server (configurable via LISTEN_ADDR)
│   └── spike_client.rs   # Test client (configurable via SERVER_URL)
└── src/
    ├── framing.rs        # Length-prefixed protobuf framing
    ├── handshake.rs      # Generic over AsyncRead/AsyncWrite
    ├── server.rs         # wtransport-based server
    └── config.rs
```

## Test Coverage

**Total: 151 tests**

| Package | Unit Tests | Integration Tests | Property-Based |
|---------|------------|-------------------|----------------|
| zellij-remote-protocol | 89 | - | - |
| zellij-remote-core | 35 | - | 6 (proptest) |
| zellij-remote-bridge | 15 | 6 | - |

### Key Test Categories

- **Protocol roundtrip**: All message types encode/decode correctly
- **Frame store**: Arc sharing, dirty tracking, resize edge cases
- **Delta engine**: Array length invariants, size mismatch handling
- **Framing**: Partial reads, multiple frames, corruption handling
- **Handshake**: Success, errors, capability negotiation

## Verified Scenarios

### Local Testing
```bash
# Terminal 1 - Server
RUST_LOG=info cargo run --example spike_server -p zellij-remote-bridge

# Terminal 2 - Interactive client (renders to terminal)
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

**Result:** Full render pipeline works over Tailscale mesh.

### Network Resilience Testing

| Scenario | Result |
|----------|--------|
| Client disconnect mid-stream | ✅ Server continues, logs warning |
| Reconnection after disconnect | ✅ Client gets current state (higher state_id) |
| Session persistence | ✅ Background updates continue without clients |
| Multiple clients | ✅ Each gets unique client_id |
| Cross-machine reconnect | ✅ Mac → sjc3, state_id 6→19 after 3s gap |

**Test output example:**
```
First connection:  client_id=1, state_id=6
(disconnect, wait 3 seconds)
Second connection: client_id=2, state_id=19  ← state persisted!
```

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

### 4. Cumulative Deltas (No Chains)
- Each delta is computed from client's last-acked baseline
- Datagram loss doesn't break delta chain (there is no chain)
- Client can skip intermediate states safely

## Next Steps

### Immediate (High Value)

#### 1. Input Handling (Phase 5)
Enable bidirectional communication:
- Client sends `InputEvent` (keyboard/mouse)
- Server routes to session, sends `InputAck`
- Enables interactive terminal use

#### 2. Zellij Integration
Connect to real Zellij sessions:
- Hook into existing render pipeline output
- Parse ANSI sequences into FrameStore
- Route input events to PTY
- Attach to existing sessions by name

### Medium Term

#### 3. Resume Tokens
True Mosh-style resumption:
- Server sends resume_token in ServerHello
- Client stores and sends on reconnect
- Server sends delta from last-acked state (not full snapshot)
- Requires: state history buffer

#### 4. Controller Lease (Phase 4)
Multi-client resize coordination:
- Lease acquisition/release protocol
- Handle lease timeouts and takeover
- Smallest-client-wins or explicit control

#### 5. Backpressure (Phase 3)
Flow control for slow clients:
- Render window (max unacked state_ids)
- StateAck message handling
- Snapshot fallback when window exhausted

### Future

#### 6. Client-side Prediction (Phase 6)
Local echo for low-latency feel:
- Predict character echo
- Reconcile with server state
- Handle mispredictions gracefully

#### 7. Mobile Client Library (Phase 7)
UniFFI bindings for iOS/Android:
- Swift/Kotlin wrappers
- Native UI rendering
- Background connection handling

## Architecture Decisions

See [2024-12-30-zellij-remote-protocol-v2.md](./2024-12-30-zellij-remote-protocol-v2.md) for detailed design rationale.

Key decisions:
- **Input**: Reliable QUIC streams (not datagrams) for exactly-once delivery
- **Render**: Datagrams for small deltas, stream fallback for large
- **State**: Per-client baselines, cumulative deltas
- **Prediction**: Deferred until correctness proven
