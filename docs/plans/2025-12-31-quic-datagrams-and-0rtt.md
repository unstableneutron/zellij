# QUIC Datagrams & 0-RTT Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Reduce ZRP latency by using QUIC datagrams for screen deltas and enabling 0-RTT session resumption.

**Architecture:** Screen deltas are sent via unreliable QUIC datagrams when small enough (≤1200 bytes), with stream fallback for large deltas. 0-RTT enables instant data transmission on reconnect by reusing TLS session tickets.

**Tech Stack:** wtransport 0.6 (WebTransport), prost (protobuf), tokio (async runtime)

---

## Prerequisites

- Phase 7.5 ZRP integration complete ✅
- QUIC validated over Tailscale ✅  
- Connection migration working ✅

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
use zellij_remote_protocol::DatagramEnvelope;

/// Encode a DatagramEnvelope to bytes (no length prefix for datagrams)
pub fn encode_datagram_envelope(envelope: &DatagramEnvelope) -> Vec<u8> {
    use prost::Message;
    let mut buf = Vec::with_capacity(envelope.encoded_len());
    envelope.encode(&mut buf).expect("encode should not fail");
    buf
}
```

**Step 2: Commit**

```bash
git add zellij-remote-bridge/src/lib.rs
git commit -m "feat(bridge): add encode_datagram_envelope helper"
```

---

### Task 6: Send Deltas via Datagram When Possible

**Files:**
- Modify: `zellij-server/src/remote/thread.rs` (handle_instruction, FrameReady case)

**Step 1: Add datagram routing logic**

In the `FrameReady` handler, after encoding the delta, check size and route appropriately:

```rust
use zellij_remote_bridge::encode_datagram_envelope;
use zellij_remote_protocol::{datagram_envelope, DatagramEnvelope};

// Conservative max size for datagrams (safe for most paths)
const DATAGRAM_SIZE_LIMIT: usize = 1200;

// Inside handle_instruction, FrameReady case:
let updates_to_send: Vec<(u64, StreamEnvelope, Option<Vec<u8>>)> = {
    // ... existing delta computation ...
    
    clients
        .keys()
        .filter_map(|&remote_id| {
            state.manager.session_mut().get_render_update(remote_id).map(|update| {
                let stream_msg = match update {
                    RenderUpdate::Snapshot(snapshot) => StreamEnvelope {
                        msg: Some(stream_envelope::Msg::ScreenSnapshot(snapshot)),
                    },
                    RenderUpdate::Delta(delta) => StreamEnvelope {
                        msg: Some(stream_envelope::Msg::ScreenDeltaStream(delta)),
                    },
                };
                
                // Try to create datagram version for deltas
                let datagram_bytes = if let Some(stream_envelope::Msg::ScreenDeltaStream(ref delta)) = stream_msg.msg {
                    let dg_envelope = DatagramEnvelope {
                        msg: Some(datagram_envelope::Msg::ScreenDelta(delta.clone())),
                    };
                    let encoded = encode_datagram_envelope(&dg_envelope);
                    if encoded.len() <= DATAGRAM_SIZE_LIMIT {
                        Some(encoded)
                    } else {
                        None // Too large, use stream
                    }
                } else {
                    None // Snapshots always use stream
                };
                
                (remote_id, stream_msg, datagram_bytes)
            })
        })
        .collect()
};

// Send updates
for (remote_id, stream_msg, datagram_bytes) in updates_to_send {
    if let Some(client) = clients.get(&remote_id) {
        // Try datagram first if available and supported
        if let Some(dg_bytes) = datagram_bytes {
            if client.supports_datagrams {
                if let Some(max_size) = client.max_datagram_size {
                    if dg_bytes.len() <= max_size {
                        match client.connection.send_datagram(&dg_bytes) {
                            Ok(()) => {
                                log::trace!("Sent delta via datagram to client {}", remote_id);
                                continue; // Success, skip stream
                            }
                            Err(e) => {
                                log::debug!("Datagram send failed for client {}: {:?}, falling back to stream", remote_id, e);
                            }
                        }
                    }
                }
            }
        }
        
        // Fallback: send via stream
        if let Err(mpsc::error::TrySendError::Full(_)) = client.sender.try_send(stream_msg) {
            log::warn!("Client {} channel full, dropping render update", remote_id);
        }
    }
}
```

**Step 2: Commit**

```bash
git add zellij-server/src/remote/thread.rs
git commit -m "feat(remote): send deltas via datagram when possible, stream fallback"
```

---

### Task 7: Update spike_client to Advertise Datagram Support

**Files:**
- Modify: `zellij-remote-bridge/examples/spike_client.rs`

**Step 1: Set supports_datagrams in ClientHello**

Find where `ClientHello` is created and ensure datagrams are advertised:

```rust
let client_hello = ClientHello {
    version: Some(ProtocolVersion {
        major: zellij_remote_protocol::ZRP_VERSION_MAJOR,
        minor: zellij_remote_protocol::ZRP_VERSION_MINOR,
    }),
    capabilities: Some(Capabilities {
        supports_datagrams: true,  // Enable datagram support
        max_datagram_bytes: 1200,
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

### Task 8: Add Datagram Receive Loop to spike_client

**Files:**
- Modify: `zellij-remote-bridge/examples/spike_client.rs`

**Step 1: Add datagram receive task**

After connection is established, spawn a task to receive datagrams:

```rust
use zellij_remote_protocol::{datagram_envelope, DatagramEnvelope};

// In run_client_loop, after connection established:
let connection_clone = connection.clone();
let (datagram_tx, mut datagram_rx) = mpsc::channel::<DatagramEnvelope>(64);

// Spawn datagram receiver
tokio::spawn(async move {
    loop {
        match connection_clone.receive_datagram().await {
            Ok(datagram) => {
                match DatagramEnvelope::decode(datagram.payload()) {
                    Ok(envelope) => {
                        if datagram_tx.send(envelope).await.is_err() {
                            break; // Channel closed
                        }
                    }
                    Err(e) => {
                        log::warn!("Failed to decode datagram: {}", e);
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

**Step 2: Handle datagram deltas in main select loop**

Add a branch to handle incoming datagrams:

```rust
// In the main select! loop:
Some(envelope) = datagram_rx.recv() => {
    match envelope.msg {
        Some(datagram_envelope::Msg::ScreenDelta(delta)) => {
            if !snapshot_received {
                continue;
            }
            
            // Check base state matches
            if delta.base_state_id != last_applied_state_id {
                log::debug!(
                    "Datagram delta base mismatch: expected {}, got {}",
                    last_applied_state_id,
                    delta.base_state_id
                );
                base_mismatch_count += 1;
                if base_mismatch_count >= 3 {
                    // Request snapshot resync
                    // TODO: send RequestSnapshot on stream
                    base_mismatch_count = 0;
                }
                continue;
            }
            
            // Apply delta
            confirmed_screen.apply_delta(&delta);
            last_applied_state_id = delta.state_id;
            base_mismatch_count = 0;
            
            let display = confirmed_screen.clone_with_overlay(&prediction_engine);
            render_screen(&display, prediction_engine.pending_count())?;
        }
        _ => {}
    }
}
```

**Step 3: Commit**

```bash
git add zellij-remote-bridge/examples/spike_client.rs
git commit -m "feat(spike_client): receive and apply deltas from datagrams"
```

---

### Task 9: Add Datagram Metrics

**Files:**
- Modify: `zellij-remote-bridge/examples/spike_client.rs`

**Step 1: Track datagram vs stream deltas**

Add counters to track delivery method:

```rust
struct RenderMetrics {
    deltas_via_datagram: u64,
    deltas_via_stream: u64,
    snapshots_received: u64,
    base_mismatches: u64,
}

// Increment appropriately in handlers:
// - datagram delta: metrics.deltas_via_datagram += 1
// - stream delta: metrics.deltas_via_stream += 1
// - snapshot: metrics.snapshots_received += 1
// - base mismatch: metrics.base_mismatches += 1
```

**Step 2: Print metrics on exit**

```rust
println!("\n=== Render Metrics ===");
println!("Deltas via datagram: {}", metrics.deltas_via_datagram);
println!("Deltas via stream:   {}", metrics.deltas_via_stream);
println!("Snapshots received:  {}", metrics.snapshots_received);
println!("Base mismatches:     {}", metrics.base_mismatches);
```

**Step 3: Commit**

```bash
git add zellij-remote-bridge/examples/spike_client.rs
git commit -m "feat(spike_client): add datagram vs stream metrics"
```

---

## Part 2: 0-RTT Session Resumption

### Task 10: Reuse Endpoint Across Reconnects in spike_client

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

### Task 11: Measure 0-RTT Improvement

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

### Task 12: Test Datagrams Over Tailscale

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

### Task 13: Test 0-RTT Reconnect

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
