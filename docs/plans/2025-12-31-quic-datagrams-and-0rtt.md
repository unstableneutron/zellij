# QUIC Datagrams & 0-RTT Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Reduce ZRP latency by using QUIC datagrams for screen deltas and enabling 0-RTT session resumption.

**Architecture:** Screen deltas are sent via unreliable QUIC datagrams when small enough, with stream fallback for large deltas. Client must handle loss/reorder via base-state checking and request snapshot resync on repeated mismatches. 0-RTT enables faster reconnects by reusing TLS session tickets.

**Tech Stack:** wtransport 0.6 (WebTransport), prost (protobuf), tokio (async runtime), bytes (for Bytes type)

---

## Prerequisites

- Phase 7.5 ZRP integration complete ✅
- QUIC validated over Tailscale ✅  
- Connection migration working ✅

## Critical Design Decisions

### Datagram Eligibility Rule
```
usable_datagram = 
    transport_supported (connection.max_datagram_size().is_some()) 
    && client_advertised (ClientHello.capabilities.supports_datagrams)
    && negotiated_allows (ServerHello.negotiated_capabilities.supports_datagrams)
    && encoded_size <= min(connection.max_datagram_size(), CONSERVATIVE_LIMIT)
```

### Conservative Size Limit
- Use `min(connection.max_datagram_size(), 1200)` to stay safe across paths
- Account for QUIC/UDP/IP overhead in path MTU

### Loss/Mismatch Handling
1. Client drops datagrams where `delta.state_id <= last_applied_state_id` (duplicate/old)
2. Client drops datagrams where `delta.base_state_id != last_applied_state_id` (mismatch)
3. After 3 consecutive mismatches, client sends `RequestSnapshot` on stream
4. Server responds with snapshot, client resets mismatch counter

### Security Notes
- `with_no_cert_validation()` is TEST ONLY - production must use proper validation
- Resume token file should be written with 0600 permissions
- 0-RTT early data is replayable - only send idempotent messages in first flight

## Task Summary

| Task | Description | File(s) |
|------|-------------|---------|
| **Part 1: Datagrams** | | |
| 1 | Add datagram fields to ClientConnection | thread.rs |
| 2 | Update ConnectionEvent with Connection | thread.rs |
| 3 | Store Connection handle on connect | thread.rs |
| 4 | Pass Connection in ClientConnected event | thread.rs |
| 5 | Add encode_datagram_envelope helper | bridge/lib.rs |
| 6 | Client datagram receive loop | spike_client.rs |
| 7 | Client advertise datagram support | spike_client.rs |
| 8 | Client render metrics | spike_client.rs |
| 9 | Server send deltas via datagram | thread.rs |
| 10 | Server handle RequestSnapshot | thread.rs, session.rs |
| **Part 2: 0-RTT** | | |
| 11 | Reuse Endpoint across reconnects | spike_client.rs |
| 12 | Measure 0-RTT improvement | spike_client.rs |
| **Part 3: Testing** | | |
| 13 | Test datagrams over Tailscale | manual |
| 14 | Test 0-RTT reconnect | manual |

---

## Part 1: Datagrams for Screen Deltas

### Task 1: Add Datagram Capability Tracking to ClientConnection

**Files:**
- Modify: `zellij-server/src/remote/thread.rs:54-58`

**Step 1: Extend ClientConnection struct**

Add fields to track datagram capability per client:

```rust
/// Per-client WebTransport connection state (M1: uses channel instead of raw stream)
struct ClientConnection {
    sender: mpsc::Sender<StreamEnvelope>,
    #[allow(dead_code)]
    remote_id: u64,
    /// Handle to the connection for sending datagrams
    connection: wtransport::Connection,
    /// Maximum datagram size negotiated (None if datagrams unsupported)
    max_datagram_size: Option<usize>,
    /// Whether client advertised datagram support
    supports_datagrams: bool,
}
```

**Step 2: Commit**

```bash
git add zellij-server/src/remote/thread.rs
git commit -m "feat(remote): add datagram capability fields to ClientConnection"
```

---

### Task 2: Update ConnectionEvent to Pass Connection Handle

**Files:**
- Modify: `zellij-server/src/remote/thread.rs:70-87`

**Step 1: Extend ClientConnected event**

```rust
/// Message from connection handlers to the main loop
enum ConnectionEvent {
    ClientConnected {
        remote_id: u64,
        send: wtransport::SendStream,
        connection: wtransport::Connection,  // Add this
    },
    // ... rest unchanged
}
```

**Step 2: Commit**

```bash
git add zellij-server/src/remote/thread.rs
git commit -m "feat(remote): pass Connection handle in ClientConnected event"
```

---

### Task 3: Store Connection Handle When Client Connects

**Files:**
- Modify: `zellij-server/src/remote/thread.rs` (handle_connection_event function, around line 330-367)

**Step 1: Update ClientConnected handler**

When a client connects, query `max_datagram_size()` and store it:

```rust
ConnectionEvent::ClientConnected { remote_id, send, connection } => {
    // Query datagram support
    let max_datagram_size = connection.max_datagram_size();
    let supports_datagrams = max_datagram_size.is_some();
    
    if supports_datagrams {
        log::info!(
            "Client {} supports datagrams, max_size={}",
            remote_id,
            max_datagram_size.unwrap()
        );
    } else {
        log::info!("Client {} does not support datagrams", remote_id);
    }

    let (tx, rx) = mpsc::channel(CLIENT_CHANNEL_SIZE);
    clients.insert(
        remote_id,
        ClientConnection {
            sender: tx,
            remote_id,
            connection,
            max_datagram_size,
            supports_datagrams,
        },
    );
    // ... spawn sender task as before
}
```

**Step 2: Commit**

```bash
git add zellij-server/src/remote/thread.rs
git commit -m "feat(remote): store datagram capability when client connects"
```

---

### Task 4: Update handle_connection to Pass Connection in Event

**Files:**
- Modify: `zellij-server/src/remote/thread.rs` (handle_connection function)

**Step 1: Find where ClientConnected is sent**

Locate the line that sends `ConnectionEvent::ClientConnected` and add the connection handle:

```rust
conn_event_tx
    .send(ConnectionEvent::ClientConnected {
        remote_id,
        send,
        connection: connection.clone(),  // Add this
    })
    .await?;
```

**Step 2: Commit**

```bash
git add zellij-server/src/remote/thread.rs
git commit -m "feat(remote): send connection handle with ClientConnected event"
```

---

### Task 5: Add encode_datagram_envelope Helper

**Files:**
- Modify: `zellij-remote-bridge/src/lib.rs`

**Step 1: Add encoding function for DatagramEnvelope**

```rust
use bytes::Bytes;
use zellij_remote_protocol::DatagramEnvelope;

/// Encode a DatagramEnvelope to Bytes (no length prefix for datagrams)
/// Returns Bytes for compatibility with wtransport send_datagram
pub fn encode_datagram_envelope(envelope: &DatagramEnvelope) -> Bytes {
    use prost::Message;
    let mut buf = Vec::with_capacity(envelope.encoded_len());
    envelope.encode(&mut buf).expect("Vec write cannot fail");
    Bytes::from(buf)
}
```

**Step 2: Add bytes dependency if not present**

Check `zellij-remote-bridge/Cargo.toml` already has `bytes = "1.5"`.

**Step 3: Commit**

```bash
git add zellij-remote-bridge/src/lib.rs
git commit -m "feat(bridge): add encode_datagram_envelope helper returning Bytes"
```

---

### Task 6: Implement Client Datagram Receive Loop (BEFORE Server Send)

**Files:**
- Modify: `zellij-remote-bridge/examples/spike_client.rs`

**Rationale:** Client must be able to receive and handle datagrams BEFORE server starts sending them.

**Step 1: Add datagram receive imports and types**

```rust
use bytes::Bytes;
use zellij_remote_protocol::{datagram_envelope, DatagramEnvelope};

struct ClientState {
    last_applied_state_id: u64,
    base_mismatch_count: u32,
    snapshot_received: bool,
    // ... other fields
}
```

**Step 2: Spawn datagram receiver task after connection**

In `run_client_loop`, after establishing the connection:

```rust
// Clone connection for datagram receiver
let connection_for_datagrams = connection.clone();
let (datagram_tx, mut datagram_rx) = mpsc::channel::<DatagramEnvelope>(64);

tokio::spawn(async move {
    loop {
        match connection_for_datagrams.receive_datagram().await {
            Ok(datagram) => {
                match DatagramEnvelope::decode(datagram.payload()) {
                    Ok(envelope) => {
                        if datagram_tx.send(envelope).await.is_err() {
                            log::debug!("Datagram channel closed, stopping receiver");
                            break;
                        }
                    }
                    Err(e) => {
                        log::warn!("Failed to decode DatagramEnvelope: {}", e);
                    }
                }
            }
            Err(e) => {
                log::debug!("Datagram receive ended: {:?}", e);
                break;
            }
        }
    }
});
```

**Step 3: Handle datagrams in main select loop**

Add branch to handle incoming datagram deltas with proper mismatch handling:

```rust
Some(envelope) = datagram_rx.recv() => {
    match envelope.msg {
        Some(datagram_envelope::Msg::ScreenDelta(delta)) => {
            if !state.snapshot_received {
                log::trace!("Ignoring datagram delta before snapshot");
                continue;
            }
            
            // Drop duplicates/old deltas
            if delta.state_id <= state.last_applied_state_id {
                log::trace!("Dropping old datagram delta: {} <= {}", 
                    delta.state_id, state.last_applied_state_id);
                continue;
            }
            
            // Check base state matches
            if delta.base_state_id != state.last_applied_state_id {
                state.base_mismatch_count += 1;
                log::debug!(
                    "Datagram base mismatch #{}: expected {}, got {}",
                    state.base_mismatch_count,
                    state.last_applied_state_id,
                    delta.base_state_id
                );
                
                if state.base_mismatch_count >= 3 {
                    // Request snapshot resync via stream
                    log::info!("Requesting snapshot after {} mismatches", state.base_mismatch_count);
                    let request = StreamEnvelope {
                        msg: Some(stream_envelope::Msg::RequestSnapshot(RequestSnapshot {
                            reason: request_snapshot::Reason::BaseMismatch as i32,
                            known_state_id: state.last_applied_state_id,
                        })),
                    };
                    let encoded = encode_envelope(&request)?;
                    send.write_all(&encoded).await?;
                    state.base_mismatch_count = 0;
                }
                continue;
            }
            
            // Apply delta successfully
            confirmed_screen.apply_delta(&delta);
            state.last_applied_state_id = delta.state_id;
            state.base_mismatch_count = 0;
            metrics.deltas_via_datagram += 1;
            
            let display = confirmed_screen.clone_with_overlay(&prediction_engine);
            render_screen(&display, prediction_engine.pending_count())?;
        }
        Some(datagram_envelope::Msg::Pong(pong)) => {
            // Handle RTT measurement pong
            log::trace!("Received pong via datagram: ping_id={}", pong.ping_id);
        }
        _ => {}
    }
}
```

**Step 4: Commit**

```bash
git add zellij-remote-bridge/examples/spike_client.rs
git commit -m "feat(spike_client): add datagram receive loop with mismatch handling"
```

---

### Task 7: Update spike_client ClientHello to Advertise Datagrams

**Files:**
- Modify: `zellij-remote-bridge/examples/spike_client.rs`

**Step 1: Enable datagram support in capabilities**

Find the `ClientHello` creation and set `supports_datagrams: true`:

```rust
let client_hello = ClientHello {
    version: Some(ProtocolVersion {
        major: zellij_remote_protocol::ZRP_VERSION_MAJOR,
        minor: zellij_remote_protocol::ZRP_VERSION_MINOR,
    }),
    capabilities: Some(Capabilities {
        supports_datagrams: true,  // Changed from false
        max_datagram_bytes: 1200,  // Conservative limit
        supports_style_dictionary: true,
        supports_styled_underlines: false,
        supports_prediction: true,
        supports_images: false,
        supports_clipboard: false,
        supports_hyperlinks: false,
    }),
    client_name: "spike_client".to_string(),
    bearer_token: bearer_token.clone().unwrap_or_default(),
    resume_token: load_resume_token(),
};
```

**Step 2: Commit**

```bash
git add zellij-remote-bridge/examples/spike_client.rs
git commit -m "feat(spike_client): advertise datagram support in ClientHello"
```

---

### Task 8: Add Render Metrics to spike_client

**Files:**
- Modify: `zellij-remote-bridge/examples/spike_client.rs`

**Step 1: Define metrics struct**

```rust
struct RenderMetrics {
    deltas_via_datagram: u64,
    deltas_via_stream: u64,
    snapshots_received: u64,
    base_mismatches: u64,
    resync_requests: u64,
    datagram_decode_errors: u64,
}

impl RenderMetrics {
    fn new() -> Self {
        Self {
            deltas_via_datagram: 0,
            deltas_via_stream: 0,
            snapshots_received: 0,
            base_mismatches: 0,
            resync_requests: 0,
            datagram_decode_errors: 0,
        }
    }
    
    fn print_summary(&self) {
        println!("\n=== Render Metrics ===");
        println!("Deltas via datagram:  {}", self.deltas_via_datagram);
        println!("Deltas via stream:    {}", self.deltas_via_stream);
        println!("Snapshots received:   {}", self.snapshots_received);
        println!("Base mismatches:      {}", self.base_mismatches);
        println!("Resync requests:      {}", self.resync_requests);
        println!("Datagram errors:      {}", self.datagram_decode_errors);
        if self.deltas_via_datagram + self.deltas_via_stream > 0 {
            let datagram_pct = 100.0 * self.deltas_via_datagram as f64 
                / (self.deltas_via_datagram + self.deltas_via_stream) as f64;
            println!("Datagram usage:       {:.1}%", datagram_pct);
        }
        println!("======================\n");
    }
}
```

**Step 2: Increment metrics in handlers**

- Datagram delta applied: `metrics.deltas_via_datagram += 1`
- Stream delta applied: `metrics.deltas_via_stream += 1`
- Snapshot received: `metrics.snapshots_received += 1`
- Base mismatch: `metrics.base_mismatches += 1`
- Resync request sent: `metrics.resync_requests += 1`

**Step 3: Print on exit**

```rust
// Before returning from run_client_loop:
metrics.print_summary();
```

**Step 4: Commit**

```bash
git add zellij-remote-bridge/examples/spike_client.rs
git commit -m "feat(spike_client): add render metrics tracking"
```

---

### Task 9: Send Deltas via Datagram When Possible (Server)

**Files:**
- Modify: `zellij-server/src/remote/thread.rs` (handle_instruction, FrameReady case)

**Step 1: Add helper function for datagram eligibility**

```rust
use bytes::Bytes;
use zellij_remote_bridge::encode_datagram_envelope;
use zellij_remote_protocol::{datagram_envelope, DatagramEnvelope};

/// Conservative max size for datagrams (safe for most Internet paths)
const DATAGRAM_SIZE_LIMIT: usize = 1200;

/// Check if we can send this delta via datagram to this client
fn can_send_datagram(client: &ClientConnection, encoded_size: usize) -> bool {
    client.supports_datagrams
        && client.max_datagram_size.map_or(false, |max| {
            encoded_size <= max.min(DATAGRAM_SIZE_LIMIT)
        })
}
```

**Step 2: Update FrameReady handler**

Modify the existing `FrameReady` handler. Key change: compute datagram bytes, try datagram send first, fall back to stream on failure.

```rust
RemoteInstruction::FrameReady { client_id: _, frame_store, style_table } => {
    // Collect updates while holding lock
    let updates_to_send: Vec<(u64, RenderUpdate)> = {
        let mut state = shared_state.write().await;
        state.current_frame = Some(frame_store.clone());
        *state.manager.style_table_mut() = style_table;
        
        // ... existing frame store update code ...
        
        clients
            .keys()
            .filter_map(|&remote_id| {
                state.manager.session_mut().get_render_update(remote_id)
                    .map(|update| (remote_id, update))
            })
            .collect()
    };
    // Lock released here - NO I/O while holding lock

    // Send updates (outside lock)
    for (remote_id, update) in updates_to_send {
        if let Some(client) = clients.get(&remote_id) {
            match update {
                RenderUpdate::Snapshot(snapshot) => {
                    // Snapshots always go via stream (reliable)
                    let msg = StreamEnvelope {
                        msg: Some(stream_envelope::Msg::ScreenSnapshot(snapshot)),
                    };
                    let _ = client.sender.try_send(msg);
                }
                RenderUpdate::Delta(delta) => {
                    // Try datagram first
                    let dg_envelope = DatagramEnvelope {
                        msg: Some(datagram_envelope::Msg::ScreenDelta(delta.clone())),
                    };
                    let dg_bytes = encode_datagram_envelope(&dg_envelope);
                    
                    if can_send_datagram(client, dg_bytes.len()) {
                        match client.connection.send_datagram(dg_bytes) {
                            Ok(()) => {
                                log::trace!("Delta via datagram to client {}", remote_id);
                                continue; // Success
                            }
                            Err(e) => {
                                log::debug!("Datagram failed for {}: {:?}, using stream", remote_id, e);
                            }
                        }
                    }
                    
                    // Fallback to stream
                    let msg = StreamEnvelope {
                        msg: Some(stream_envelope::Msg::ScreenDeltaStream(delta)),
                    };
                    if let Err(mpsc::error::TrySendError::Full(_)) = client.sender.try_send(msg) {
                        log::warn!("Client {} channel full, dropping delta", remote_id);
                    }
                }
            }
        }
    }
}
```

**Step 3: Commit**

```bash
git add zellij-server/src/remote/thread.rs
git commit -m "feat(remote): send deltas via datagram with stream fallback"
```

---

### Task 10: Handle RequestSnapshot from Client

**Files:**
- Modify: `zellij-server/src/remote/thread.rs` (handle_connection_event or add new event type)

**Step 1: Add RequestSnapshot to ConnectionEvent**

```rust
enum ConnectionEvent {
    // ... existing variants ...
    RequestSnapshot {
        remote_id: u64,
        request: zellij_remote_protocol::RequestSnapshot,
    },
}
```

**Step 2: Parse RequestSnapshot in client stream handler**

In the connection handler that reads from the client stream:

```rust
Some(stream_envelope::Msg::RequestSnapshot(request)) => {
    log::info!("Client {} requested snapshot: reason={:?}", remote_id, request.reason);
    conn_event_tx.send(ConnectionEvent::RequestSnapshot { remote_id, request }).await?;
}
```

**Step 3: Handle in event loop - force snapshot for client**

```rust
ConnectionEvent::RequestSnapshot { remote_id, request } => {
    log::info!(
        "Processing snapshot request from {}: reason={}, known_state={}",
        remote_id, request.reason, request.known_state_id
    );
    
    let mut state = shared_state.write().await;
    // Force the client's baseline to 0 so next render produces a snapshot
    state.manager.session_mut().force_snapshot_for_client(remote_id);
}
```

**Step 4: Add force_snapshot_for_client to RemoteSession (if not exists)**

In `zellij-remote-core/src/session.rs`:

```rust
impl RemoteSession {
    pub fn force_snapshot_for_client(&mut self, client_id: u64) {
        if let Some(client_state) = self.client_states.get_mut(&client_id) {
            client_state.reset_baseline();
        }
    }
}
```

**Step 5: Commit**

```bash
git add zellij-server/src/remote/thread.rs zellij-remote-core/src/session.rs
git commit -m "feat(remote): handle RequestSnapshot from client for resync"
```

---

## Part 2: 0-RTT Session Resumption

### Task 11: Reuse Endpoint Across Reconnects in spike_client

**Files:**
- Modify: `zellij-remote-bridge/examples/spike_client.rs`

**Step 1: Restructure to keep Endpoint alive**

Currently spike_client likely creates a new Endpoint per connection. Restructure to keep it alive:

```rust
async fn main() -> Result<()> {
    // Create endpoint ONCE at startup
    let config = ClientConfig::builder()
        .with_bind_default()
        .with_no_cert_validation()
        .build();
    
    let endpoint = Endpoint::client(config)?;
    
    // Reconnect loop (if RECONNECT env var set)
    let reconnect_mode = env::var("RECONNECT").is_ok();
    
    loop {
        match run_session(&endpoint, &server_url).await {
            Ok(()) => {
                if !reconnect_mode {
                    break;
                }
                log::info!("Session ended, reconnecting in 1s...");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            Err(e) => {
                log::error!("Session error: {}", e);
                if !reconnect_mode {
                    return Err(e);
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
    
    Ok(())
}

async fn run_session(endpoint: &Endpoint<Client>, server_url: &str) -> Result<()> {
    let connection = endpoint.connect(server_url).await?;
    // ... rest of session logic
}
```

**Step 2: Commit**

```bash
git add zellij-remote-bridge/examples/spike_client.rs
git commit -m "feat(spike_client): reuse Endpoint across reconnects for 0-RTT"
```

---

### Task 12: Measure 0-RTT Improvement

**Files:**
- Modify: `zellij-remote-bridge/examples/spike_client.rs`

**Step 1: Track connect time across reconnects**

```rust
struct SessionMetrics {
    connect_time_ms: f64,
    time_to_first_snapshot_ms: f64,
    is_0rtt: bool,  // Whether 0-RTT was used (inferred from fast connect)
}

// In run_session:
let connect_start = Instant::now();
let connection = endpoint.connect(server_url).await?;
let connect_time = connect_start.elapsed();

// Log with reconnect indicator
static CONNECT_COUNT: AtomicU64 = AtomicU64::new(0);
let count = CONNECT_COUNT.fetch_add(1, Ordering::Relaxed);
let likely_0rtt = count > 0 && connect_time.as_millis() < 100; // Heuristic

log::info!(
    "Connect #{}: {:.2}ms {}",
    count,
    connect_time.as_secs_f64() * 1000.0,
    if likely_0rtt { "(likely 0-RTT)" } else { "" }
);
```

**Step 2: Commit**

```bash
git add zellij-remote-bridge/examples/spike_client.rs
git commit -m "feat(spike_client): measure and log 0-RTT connect improvement"
```

---

## Part 3: Testing

### Task 13: Test Datagrams Over Tailscale

**Files:** None (manual testing)

**Step 1: Build and deploy**

```bash
# Build for Linux aarch64
cross build --release --target aarch64-unknown-linux-gnu \
    --example spike_client -p zellij-remote-bridge

# Copy to sjc3
scp target/aarch64-unknown-linux-gnu/release/examples/spike_client sjc3:~/spike_client_new
```

**Step 2: Start Zellij with remote on sjc3**

```bash
ssh sjc3
chmod +x spike_client_new
ZELLIJ_REMOTE_ADDR=0.0.0.0:4433 ZELLIJ_REMOTE_TOKEN=test123 ./zellij
```

**Step 3: Connect from local with datagram support**

```bash
SERVER_URL="https://100.69.153.168:4433" RUST_LOG=info \
    cargo run --release --example spike_client -p zellij-remote-bridge
```

**Step 4: Verify metrics show datagram usage**

Expected output on exit:
```
=== Render Metrics ===
Deltas via datagram: 150
Deltas via stream:   2    (for oversized deltas)
Snapshots received:  1
Base mismatches:     0
```

---

### Task 14: Test 0-RTT Reconnect

**Step 1: Run with RECONNECT mode**

```bash
SERVER_URL="https://100.69.153.168:4433" RECONNECT=1 RUST_LOG=info \
    cargo run --release --example spike_client -p zellij-remote-bridge
```

**Step 2: Observe connect times**

```
Connect #0: 552.07ms           (first connect, full handshake)
Connect #1: 185.23ms (likely 0-RTT)  (reconnect, 1 RTT saved)
Connect #2: 182.45ms (likely 0-RTT)
```

Expected: ~1 RTT reduction (~180ms to sjc3) on reconnects.

---

## Verification Checklist

- [ ] Datagrams used for small deltas (< 1200 bytes)
- [ ] Stream fallback for large deltas
- [ ] No base mismatches under normal operation
- [ ] Metrics show datagram >> stream for typical usage
- [ ] 0-RTT reduces reconnect time by ~1 RTT
- [ ] Connection migration still works
- [ ] No regressions in existing tests: `cargo xtask test`
