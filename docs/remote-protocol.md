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

# Connect to remote server
SERVER_URL="https://100.69.153.168:4433" cargo run --example spike_client -p zellij-remote-bridge

# Headless mode for testing
HEADLESS=1 cargo run --example spike_client -p zellij-remote-bridge
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
- **Render**: Datagrams for small deltas (lossy OK), streams for snapshots

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

```bash
# Run all tests
cargo test -p zellij-remote-protocol -p zellij-remote-core -p zellij-remote-bridge

# Run with logging
RUST_LOG=debug cargo test -p zellij-remote-bridge -- --nocapture

# Test specific category
cargo test -p zellij-remote-core -- lease_tests
cargo test -p zellij-remote-core -- input_tests
cargo test -p zellij-remote-core -- backpressure_tests
```

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
- 🔲 Zellij integration
- 🔲 Resume tokens
- 🔲 Client-side prediction
- 🔲 Mobile client library
