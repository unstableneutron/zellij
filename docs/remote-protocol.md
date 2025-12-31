# Zellij Remote Protocol (ZRP)

ZRP enables Mosh-style remote terminal access to Zellij sessions over WebTransport/QUIC.

## Quick Start

### Run the Test Server
```bash
# Default: localhost:4433
cargo run --example spike_server -p zellij-remote-bridge

# Custom address (e.g., for Tailscale)
LISTEN_ADDR=0.0.0.0:4433 cargo run --example spike_server -p zellij-remote-bridge
```

### Run the Test Client
```bash
# Connect to localhost (interactive with keyboard input)
cargo run --example spike_client -p zellij-remote-bridge

# Connect to remote server with authentication
cargo run --example spike_client -p zellij-remote-bridge -- \
  --server-url https://100.69.153.168:4433 \
  --token "$ZELLIJ_REMOTE_TOKEN"

# Headless mode with metrics output
cargo run --example spike_client -p zellij-remote-bridge -- \
  --headless --metrics-out /tmp/metrics.json

# With reconnection support
cargo run --example spike_client -p zellij-remote-bridge -- \
  --reconnect=always --token "$ZELLIJ_REMOTE_TOKEN"

# View datagram metrics on exit
# === Render Metrics ===
# Deltas via datagram: 150
# Deltas via stream:   2
# Snapshots received:  1
# Base mismatches:     0
```

## Crates

### zellij-remote-protocol
Protobuf message definitions for the ZRP protocol.

```rust
use zellij_remote_protocol::{ClientHello, ServerHello, ScreenDelta, ScreenSnapshot, InputEvent, InputAck};
```

Key messages:
- `ClientHello` / `ServerHello` - Handshake and capability negotiation
- `ScreenSnapshot` - Full screen state (sent on connect, resync)
- `ScreenDelta` - Incremental updates (row patches)
- `InputEvent` / `InputAck` - Keyboard/mouse input with acknowledgment
- `ControllerLease` - Resize control coordination
- `StateAck` - Client acknowledges applied render state

### zellij-remote-core
Core state management for efficient multi-client rendering.

```rust
use zellij_remote_core::{RemoteSession, RenderUpdate, LeaseResult, InputError};

// Create a session
let mut session = RemoteSession::new(80, 24);

// Add a client
session.add_client(client_id, 4 /* render window size */);

// Grant control to client
if let LeaseResult::Granted(lease) = session.lease_manager.request_control(client_id, None, false) {
    // Client is now controller
}

// Process input from controller
match session.process_input(client_id, &input_event) {
    Ok(ack) => { /* send ack to client */ }
    Err(InputError::NotController) => { /* reject */ }
    Err(e) => { /* handle error */ }
}

// Get render update for client
match session.get_render_update(client_id) {
    Some(RenderUpdate::Snapshot(snapshot)) => { /* send snapshot */ }
    Some(RenderUpdate::Delta(delta)) => { /* send delta */ }
    None => { /* nothing to send */ }
}
```

Key components:
- `RemoteSession` - Aggregates all session state
- `FrameStore` - Screen buffer with `Arc<Row>` sharing
- `DeltaEngine` - Computes cumulative deltas
- `LeaseManager` - Controller lease state machine
- `RenderWindow` - Backpressure/flow control
- `InputReceiver/InputSender` - Reliable input handling
- `RttEstimator` - RTT estimation for latency tracking
- `PredictionEngine` - Client-side local echo with reconciliation

### zellij-remote-bridge
WebTransport server implementation.

```rust
use zellij_remote_bridge::{BridgeConfig, RemoteBridge};

let config = BridgeConfig {
    listen_addr: "0.0.0.0:4433".parse().unwrap(),
    session_name: "my-session".to_string(),
    ..Default::default()
};
let bridge = RemoteBridge::new(config);
bridge.run().await?;
```

## Protocol Design

### Transport
- **WebTransport over QUIC** - Low latency, multiplexed streams
- **Input**: Reliable streams for exactly-once, in-order delivery
- **Render**: 
  - **Datagrams** for small deltas (≤1200 bytes) - lower latency, unreliable
  - **Streams** for large deltas and snapshots - reliable delivery
  - Client handles datagram loss via base mismatch detection
  - After 3 consecutive mismatches, client requests snapshot resync

### Datagram Handling
- Server checks `transport_supported && client_advertised && server_negotiated`
- Conservative size limit: `min(connection.max_datagram_size, 1200)` bytes
- Client filters old/duplicate datagrams by `state_id`
- Client tracks `base_state_id` mismatches and requests resync if needed

### 0-RTT Session Resumption
- Client reuses `Endpoint` across reconnections for TLS session ticket reuse
- First connection: Full TLS handshake (~1.5 RTT)
- Subsequent connections: 0-RTT early data (~0.5 RTT)
- **Security note**: Early data is replayable - only idempotent messages in first flight

### State Sync
- Server maintains authoritative screen state in `FrameStore`
- Each client has a baseline `state_id` representing last-acked state
- Deltas computed from client's acked baseline (cumulative, not chained)
- Baselines only advance on StateAck - prevents issues with lost datagrams

### Controller Lease
- Only one client can control resize/input at a time
- `ExplicitOnly` policy: explicit request required for takeover
- `LastWriterWins` policy: new client can take over
- Viewers receive render updates but cannot send input
- Lease expires without keepalive

### Message Flow
```
Client                          Server
  |                               |
  |------- ClientHello --------->|  (version, capabilities)
  |<------ ServerHello ----------|  (negotiated caps, client_id, lease)
  |                               |
  |------- RequestControl ------>|  (request lease)
  |<------ GrantControl ---------|  (lease granted)
  |                               |
  |<------ ScreenSnapshot -------|  (full state)
  |                               |
  |------- InputEvent ---------->|  (key/mouse, seq)
  |<------ InputAck -------------|  (acked_seq)
  |<------ ScreenDelta ----------|  (row patches)
  |------- StateAck ------------>|  (acknowledge render)
  |                               |
  |------- KeepAliveLease ------>|  (extend lease)
```

## Testing

### Unit Tests
```bash
# Run all unit tests
cargo test -p zellij-remote-protocol -p zellij-remote-core -p zellij-remote-bridge

# Run with logging
RUST_LOG=debug cargo test -p zellij-remote-bridge -- --nocapture

# Test specific category
cargo test -p zellij-remote-core -- lease_tests
cargo test -p zellij-remote-core -- input_tests
cargo test -p zellij-remote-core -- backpressure_tests
```

### E2E Tests
See [zellij-remote-tests/README.md](../zellij-remote-tests/README.md) for full documentation.

```bash
cd zellij-remote-tests

# Run all E2E tests
make test-all

# Individual tests
make test-local           # Basic connectivity
make test-local-auth      # Authentication (valid/invalid/no token)
make test-local-reconnect # Reconnection and metrics
make test-local-datagram  # Datagram negotiation and 0-RTT
```

### Server-Side Test Knobs
Environment variables for fault injection during testing:

| Variable | Description |
|----------|-------------|
| `ZELLIJ_REMOTE_DROP_DELTA_NTH=N` | Drop every Nth delta |
| `ZELLIJ_REMOTE_DELAY_SEND_MS=N` | Add N ms delay to sends |
| `ZELLIJ_REMOTE_FORCE_SNAPSHOT_EVERY=N` | Force snapshot every N frames |
| `ZELLIJ_REMOTE_LOG_FRAME_STATS=1` | Log frame statistics |

## Implementation Status

See [docs/plans/2024-12-31-zrp-implementation-status.md](plans/2024-12-31-zrp-implementation-status.md) for current status.

- ✅ Protocol definitions (protobuf)
- ✅ Core state management (FrameStore, DeltaEngine)
- ✅ WebTransport server
- ✅ Handshake flow
- ✅ ScreenSnapshot / ScreenDelta rendering
- ✅ Session persistence across reconnections
- ✅ Cross-machine verification (Tailscale)
- ✅ Backpressure & flow control
- ✅ Controller lease
- ✅ Input handling with acknowledgment
- ✅ RTT estimation
- ✅ Resume tokens
- ✅ Client-side prediction
- ✅ Zellij integration (Phase 7 + 7.5)
- ✅ QUIC datagrams for screen deltas
- ✅ 0-RTT session resumption
- 🔲 Mobile client library

## Running with Zellij

### Basic (localhost only)
```bash
# Start Zellij with remote support (localhost only, no auth needed)
ZELLIJ_REMOTE_ADDR=127.0.0.1:4433 cargo run --features remote

# Connect with spike_client
cargo run --example spike_client -p zellij-remote-bridge
```

### Network Access (with authentication)
```bash
# Generate a secure token
export ZELLIJ_REMOTE_TOKEN=$(openssl rand -hex 32)

# Start Zellij with remote support on all interfaces
ZELLIJ_REMOTE_ADDR=0.0.0.0:4433 cargo run --features remote

# Connect with spike_client using token
cargo run --example spike_client -p zellij-remote-bridge -- \
  --server-url https://your-server:4433 \
  --token "$ZELLIJ_REMOTE_TOKEN"
```

## Security

The remote server includes several security features:

- **Bearer Token Authentication**: Set `ZELLIJ_REMOTE_TOKEN` to require clients to authenticate
- **Bind Address Validation**: Critical warning if binding to non-loopback without authentication
- **Controller Lease Enforcement**: Only the lease holder can send input; non-controllers receive `LEASE_DENIED` errors
- **Frame Size Limits**: Maximum 1MB frame size to prevent memory exhaustion attacks
- **Per-Client Send Queues**: Bounded queues prevent slow clients from blocking others
