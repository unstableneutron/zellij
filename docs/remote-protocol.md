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
# Connect to localhost
cargo run --example spike_client -p zellij-remote-bridge

# Connect to remote server
SERVER_URL="https://100.69.153.168:4433" cargo run --example spike_client -p zellij-remote-bridge
```

## Crates

### zellij-remote-protocol
Protobuf message definitions for the ZRP protocol.

```rust
use zellij_remote_protocol::{ClientHello, ServerHello, ScreenDelta, ScreenSnapshot};
```

Key messages:
- `ClientHello` / `ServerHello` - Handshake and capability negotiation
- `ScreenSnapshot` - Full screen state (sent on connect, resync)
- `ScreenDelta` - Incremental updates (row patches)
- `InputEvent` / `InputAck` - Keyboard/mouse input with acknowledgment
- `ControllerLease` - Resize control coordination

### zellij-remote-core
Core state management for efficient multi-client rendering.

```rust
use zellij_remote_core::{FrameStore, DeltaEngine, StyleTable};

let mut store = FrameStore::new(80, 24);
store.update_row(0, |row| {
    row.set_cell(0, Cell { codepoint: 'H' as u32, width: 1, style_id: 0 });
});
store.advance_state();

let delta = DeltaEngine::compute_delta(&baseline, &current, &mut styles, base_id, current_id);
```

Features:
- `Arc<Row>` sharing - unchanged rows share memory across clients
- O(1) delta detection via `Arc::ptr_eq()`
- Cumulative deltas from per-client baselines (no delta chains)

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
- Deltas computed from client's baseline (cumulative, not chained)
- Datagram loss handled gracefully - just send next delta from same baseline

### Capability Negotiation
```
Client                          Server
  |                               |
  |------- ClientHello --------->|  (version, capabilities)
  |<------ ServerHello ----------|  (negotiated caps, client_id, lease)
  |                               |
  |------- AttachRequest ------->|  (mode, desired_role)
  |<------ AttachResponse -------|  (ok, will_send_snapshot)
  |<------ ScreenSnapshot -------|  (full state)
  |                               |
  |------- InputEvent ---------->|  (key/mouse, seq)
  |<------ InputAck -------------|  (acked_seq)
  |<------ ScreenDelta ----------|  (row patches)
```

## Testing

```bash
# Run all tests
cargo test -p zellij-remote-protocol -p zellij-remote-core -p zellij-remote-bridge

# Run with logging
RUST_LOG=debug cargo test -p zellij-remote-bridge -- --nocapture
```

## Implementation Status

See [docs/plans/2024-12-31-zrp-implementation-status.md](plans/2024-12-31-zrp-implementation-status.md) for current status.

- ✅ Protocol definitions
- ✅ Core state management
- ✅ WebTransport server skeleton
- ✅ Handshake flow
- 🔲 Zellij integration
- 🔲 Input handling
- 🔲 Controller lease
- 🔲 Client-side prediction
