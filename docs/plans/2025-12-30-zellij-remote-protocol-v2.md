# Zellij Remote Protocol (ZRP) Implementation Plan v2

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement a Mosh-style remote protocol for Zellij enabling low-latency mobile/web clients with client-side prediction, loss-tolerant state sync, and a controller lease model.

**Architecture:** WebTransport/QUIC-based protocol with reliable stream input (exactly-once, in-order), cumulative delta renders from per-client baselines (no delta chains), and a FrameStore with shared `Arc<Row>` for memory-efficient multi-client scaling. The bridge reuses Zellij's existing render pipeline output to avoid semantic drift.

**Tech Stack:** Rust, prost (protobuf via build.rs), wtransport (WebTransport), tokio, UniFFI (mobile)

**Key Design Decisions:**
- **Input:** Reliable QUIC stream (not datagrams) for exactly-once, in-order delivery
- **Render:** Datagrams for small deltas, streams for snapshots; cumulative deltas from client baseline
- **State:** Persistent frames with `Arc<Row>` sharing; dirty-row tracking from Zellij render events
- **Prediction:** Deferred until correctness-first sync is proven

---

## Phase 0: Repository & Build Foundations

### Task 0.1: Create Crate Structure with Correct Codegen

**Files:**
- Create: `zellij-remote-protocol/Cargo.toml`
- Create: `zellij-remote-protocol/build.rs`
- Create: `zellij-remote-protocol/src/lib.rs`
- Create: `zellij-remote-protocol/proto/zellij_remote.proto`
- Modify: `Cargo.toml` (workspace members)

**Step 1: Create crate directory structure**

```bash
mkdir -p zellij-remote-protocol/src zellij-remote-protocol/proto
```

**Step 2: Create Cargo.toml**

Create `zellij-remote-protocol/Cargo.toml`:
```toml
[package]
name = "zellij-remote-protocol"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
prost = { workspace = true }
bytes = "1.5"

[build-dependencies]
prost-build = "0.11.9"

[dev-dependencies]
proptest = "1.4"
```

**Step 3: Create build.rs (CRITICAL: correct codegen path)**

Create `zellij-remote-protocol/build.rs`:
```rust
use std::io::Result;

fn main() -> Result<()> {
    // prost-build outputs to OUT_DIR, file named after proto package
    // For package "zellij.remote.v1", generates "zellij.remote.v1.rs"
    prost_build::compile_protos(
        &["proto/zellij_remote.proto"],
        &["proto/"],
    )?;
    Ok(())
}
```

**Step 4: Create lib.rs with correct include**

Create `zellij-remote-protocol/src/lib.rs`:
```rust
// Include generated code from OUT_DIR (set by cargo during build)
// prost generates filename based on proto package name
pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/zellij.remote.v1.rs"));
}

pub use proto::*;

pub const ZRP_VERSION_MAJOR: u32 = 1;
pub const ZRP_VERSION_MINOR: u32 = 0;
pub const DEFAULT_MAX_DATAGRAM_BYTES: u32 = 1200;
pub const DEFAULT_RENDER_WINDOW: u32 = 4;
```

**Step 5: Create proto file with ALL required messages**

Create `zellij-remote-protocol/proto/zellij_remote.proto`:
```proto
syntax = "proto3";

package zellij.remote.v1;

// =============================================================================
// VERSION & CAPABILITIES
// =============================================================================

message ProtocolVersion {
  uint32 major = 1;
  uint32 minor = 2;
}

message Capabilities {
  bool supports_datagrams = 1;
  uint32 max_datagram_bytes = 2;
  bool supports_style_dictionary = 3;
  bool supports_styled_underlines = 4;
  bool supports_prediction = 5;
  bool supports_images = 6;       // sixel/kitty images
  bool supports_clipboard = 7;    // OSC52
  bool supports_hyperlinks = 8;
}

// =============================================================================
// HANDSHAKE & AUTH
// =============================================================================

message ClientHello {
  ProtocolVersion version = 1;
  Capabilities capabilities = 2;
  string client_name = 3;         // "ios", "android", "web"
  bytes bearer_token = 4;         // auth token
  bytes resume_token = 5;         // optional fast-resume
}

message ServerHello {
  ProtocolVersion negotiated_version = 1;
  Capabilities negotiated_capabilities = 2;
  uint64 client_id = 3;
  string session_name = 4;
  SessionState session_state = 5;
  ControllerLease lease = 6;
  bytes resume_token = 7;
  uint32 snapshot_interval_ms = 8;
  uint32 max_inflight_inputs = 9;
  uint32 render_window = 10;      // max unacked state_ids
}

enum SessionState {
  SESSION_STATE_UNSPECIFIED = 0;
  SESSION_STATE_RUNNING = 1;
  SESSION_STATE_CREATED = 2;
  SESSION_STATE_RESURRECTED = 3;
}

// =============================================================================
// ATTACH & RESUME
// =============================================================================

enum AttachMode {
  ATTACH_MODE_UNSPECIFIED = 0;
  ATTACH_MODE_RESUME = 1;         // try delta from last_applied_state_id
  ATTACH_MODE_FRESH = 2;          // force snapshot
}

enum ClientRole {
  CLIENT_ROLE_UNSPECIFIED = 0;
  CLIENT_ROLE_VIEWER = 1;
  CLIENT_ROLE_CONTROLLER = 2;
}

message AttachRequest {
  AttachMode mode = 1;
  uint64 last_applied_state_id = 2;
  uint64 last_acked_input_seq = 3;
  ClientRole desired_role = 4;
  DisplaySize desired_size = 5;
  bool read_only = 6;
  bool force_snapshot = 7;
}

message AttachResponse {
  bool ok = 1;
  string error_message = 2;
  ControllerLease lease = 3;
  uint64 current_state_id = 4;
  bool will_send_snapshot = 5;
}

// =============================================================================
// CONTROLLER LEASE (tmux-like resize control)
// =============================================================================

enum ControllerPolicy {
  CONTROLLER_POLICY_UNSPECIFIED = 0;
  CONTROLLER_POLICY_EXPLICIT_ONLY = 1;
  CONTROLLER_POLICY_LAST_WRITER_WINS = 2;
}

message ControllerLease {
  uint64 lease_id = 1;
  uint64 owner_client_id = 2;
  ControllerPolicy policy = 3;
  DisplaySize current_size = 4;
  uint32 remaining_ms = 5;
  uint32 duration_ms = 6;
}

message RequestControl {
  string reason = 1;
  DisplaySize desired_size = 2;
  bool force = 3;
}

message GrantControl {
  ControllerLease lease = 1;
}

message DenyControl {
  string reason = 1;
  ControllerLease lease = 2;
}

message ReleaseControl {
  uint64 lease_id = 1;
}

message SetControllerSize {
  DisplaySize size = 1;
  bool request_snapshot = 2;
}

message KeepAliveLease {
  uint64 lease_id = 1;
  uint32 client_time_ms = 2;
}

message LeaseRevoked {
  uint64 lease_id = 1;
  string reason = 2;              // "timeout", "takeover", "disconnect"
}

// =============================================================================
// INPUT (reliable stream, exactly-once in-order)
// =============================================================================

message KeyModifiers {
  uint32 bits = 1;                // SHIFT=1, ALT=2, CTRL=4, SUPER=8
}

enum SpecialKey {
  SPECIAL_KEY_UNSPECIFIED = 0;
  SPECIAL_KEY_ENTER = 1;
  SPECIAL_KEY_ESCAPE = 2;
  SPECIAL_KEY_BACKSPACE = 3;
  SPECIAL_KEY_TAB = 4;
  SPECIAL_KEY_LEFT = 10;
  SPECIAL_KEY_RIGHT = 11;
  SPECIAL_KEY_UP = 12;
  SPECIAL_KEY_DOWN = 13;
  SPECIAL_KEY_HOME = 20;
  SPECIAL_KEY_END = 21;
  SPECIAL_KEY_PAGE_UP = 22;
  SPECIAL_KEY_PAGE_DOWN = 23;
  SPECIAL_KEY_INSERT = 24;
  SPECIAL_KEY_DELETE = 25;
  SPECIAL_KEY_F1 = 40;
  SPECIAL_KEY_F2 = 41;
  SPECIAL_KEY_F3 = 42;
  SPECIAL_KEY_F4 = 43;
  SPECIAL_KEY_F5 = 44;
  SPECIAL_KEY_F6 = 45;
  SPECIAL_KEY_F7 = 46;
  SPECIAL_KEY_F8 = 47;
  SPECIAL_KEY_F9 = 48;
  SPECIAL_KEY_F10 = 49;
  SPECIAL_KEY_F11 = 50;
  SPECIAL_KEY_F12 = 51;
}

message KeyEvent {
  KeyModifiers modifiers = 1;
  oneof key {
    uint32 unicode_scalar = 2;
    SpecialKey special = 3;
  }
}

enum MouseKind {
  MOUSE_KIND_UNSPECIFIED = 0;
  MOUSE_KIND_MOVE = 1;
  MOUSE_KIND_DOWN = 2;
  MOUSE_KIND_UP = 3;
  MOUSE_KIND_SCROLL = 4;
}

enum MouseButton {
  MOUSE_BUTTON_UNSPECIFIED = 0;
  MOUSE_BUTTON_LEFT = 1;
  MOUSE_BUTTON_MIDDLE = 2;
  MOUSE_BUTTON_RIGHT = 3;
}

message MouseEvent {
  MouseKind kind = 1;
  uint32 col = 2;
  uint32 row = 3;
  MouseButton button = 4;
  int32 scroll_delta = 5;
  KeyModifiers modifiers = 6;
}

message InputEvent {
  uint64 input_seq = 1;
  uint32 client_time_ms = 2;
  oneof payload {
    bytes text_utf8 = 10;         // IME/paste
    KeyEvent key = 11;
    bytes raw_bytes = 12;         // escape sequences
    MouseEvent mouse = 13;
  }
}

message InputAck {
  uint64 acked_seq = 1;           // cumulative: all <= acked_seq delivered
  uint64 rtt_sample_seq = 2;
  uint32 echoed_client_time_ms = 3;
}

// =============================================================================
// RENDER: SCREEN STATE SYNC
// =============================================================================

message DisplaySize {
  uint32 cols = 1;
  uint32 rows = 2;
}

message DefaultColor {}

message Rgb {
  uint32 r = 1;
  uint32 g = 2;
  uint32 b = 3;
}

message Color {
  oneof value {
    DefaultColor default_color = 1;
    uint32 ansi256 = 2;
    Rgb rgb = 3;
  }
}

enum UnderlineStyle {
  UNDERLINE_STYLE_UNSPECIFIED = 0;
  UNDERLINE_STYLE_NONE = 1;
  UNDERLINE_STYLE_SINGLE = 2;
  UNDERLINE_STYLE_DOUBLE = 3;
  UNDERLINE_STYLE_DOTTED = 4;
  UNDERLINE_STYLE_DASHED = 5;
  UNDERLINE_STYLE_CURLY = 6;
}

message Style {
  Color fg = 1;
  Color bg = 2;
  bool bold = 3;
  bool dim = 4;
  bool italic = 5;
  bool reverse = 6;
  bool hidden = 7;
  bool strike = 8;
  bool blink_slow = 9;
  bool blink_fast = 10;
  UnderlineStyle underline = 11;
  Color underline_color = 12;
}

message StyleDef {
  uint32 style_id = 1;
  Style style = 2;
}

enum CursorShape {
  CURSOR_SHAPE_UNSPECIFIED = 0;
  CURSOR_SHAPE_BLOCK = 1;
  CURSOR_SHAPE_BEAM = 2;
  CURSOR_SHAPE_UNDERLINE = 3;
}

message CursorState {
  uint32 row = 1;
  uint32 col = 2;
  bool visible = 3;
  bool blink = 4;
  CursorShape shape = 5;
}

message RowData {
  uint32 row = 1;
  repeated uint32 codepoints = 2 [packed = true];
  repeated uint32 widths = 3 [packed = true];
  repeated uint32 style_ids = 4 [packed = true];
}

message CellRun {
  uint32 col_start = 1;
  repeated uint32 codepoints = 2 [packed = true];
  repeated uint32 widths = 3 [packed = true];
  repeated uint32 style_ids = 4 [packed = true];
}

message RowPatch {
  uint32 row = 1;
  repeated CellRun runs = 2;
}

message ScreenDelta {
  uint64 base_state_id = 1;       // client must have this applied
  uint64 state_id = 2;            // resulting state after apply
  repeated StyleDef styles_added = 3;
  repeated RowPatch row_patches = 4;
  CursorState cursor = 5;
  uint64 delivered_input_watermark = 6;  // for prediction reconciliation
}

message ScreenSnapshot {
  uint64 state_id = 1;
  DisplaySize size = 2;
  bool style_table_reset = 3;
  repeated StyleDef styles = 4;
  repeated RowData rows = 5;
  CursorState cursor = 6;
  uint64 delivered_input_watermark = 7;
}

message StateAck {
  uint64 last_applied_state_id = 1;
  uint64 last_received_state_id = 2;
  uint32 client_time_ms = 3;
  uint32 estimated_loss_ppm = 4;
  uint32 srtt_ms = 5;
}

// =============================================================================
// RESYNC & ERRORS
// =============================================================================

message RequestSnapshot {
  enum Reason {
    REASON_UNSPECIFIED = 0;
    REASON_BASE_MISMATCH = 1;
    REASON_PERIODIC = 2;
    REASON_DECODE_ERROR = 3;
    REASON_USER_REQUEST = 4;
  }
  Reason reason = 1;
  uint64 known_state_id = 2;
}

message ProtocolError {
  enum Code {
    CODE_UNSPECIFIED = 0;
    CODE_UNAUTHORIZED = 1;
    CODE_BAD_VERSION = 2;
    CODE_BAD_MESSAGE = 3;
    CODE_FLOW_CONTROL = 4;
    CODE_SESSION_NOT_FOUND = 5;
    CODE_LEASE_DENIED = 6;
    CODE_INTERNAL = 7;
  }
  Code code = 1;
  string message = 2;
  bool fatal = 3;
}

// =============================================================================
// KEEPALIVE / RTT
// =============================================================================

message Ping {
  uint64 ping_id = 1;
  uint32 client_time_ms = 2;
}

message Pong {
  uint64 ping_id = 1;
  uint32 echoed_client_time_ms = 2;
  uint32 server_time_ms = 3;
}

// =============================================================================
// UNSUPPORTED FEATURE CONTRACTS
// =============================================================================

message UnsupportedFeatureNotice {
  string feature = 1;             // "images", "clipboard", "hyperlinks"
  string behavior = 2;            // "ignored", "placeholder", "stripped"
}

// =============================================================================
// ENVELOPES (stream vs datagram routing)
// =============================================================================

// Reliable streams: control, input, large renders
message StreamEnvelope {
  oneof msg {
    // Handshake
    ClientHello client_hello = 1;
    ServerHello server_hello = 2;
    AttachRequest attach_request = 3;
    AttachResponse attach_response = 4;
    
    // Lease
    RequestControl request_control = 10;
    GrantControl grant_control = 11;
    DenyControl deny_control = 12;
    ReleaseControl release_control = 13;
    SetControllerSize set_controller_size = 14;
    KeepAliveLease keep_alive_lease = 15;
    LeaseRevoked lease_revoked = 16;
    
    // Resync
    RequestSnapshot request_snapshot = 20;
    
    // Errors & keepalive
    Ping ping = 30;
    Pong pong = 31;
    ProtocolError protocol_error = 32;
    UnsupportedFeatureNotice unsupported_notice = 33;
    
    // Render (large)
    ScreenSnapshot screen_snapshot = 40;
    ScreenDelta screen_delta_stream = 41;  // when too big for datagram
    
    // Input (reliable stream path - MVP)
    InputEvent input_event = 50;
    InputAck input_ack = 51;
  }
}

// Datagrams: latency-sensitive, loss-tolerant
message DatagramEnvelope {
  oneof msg {
    ScreenDelta screen_delta = 10;
    StateAck state_ack = 11;
    Ping ping = 30;
    Pong pong = 31;
  }
}
```

**Step 6: Add to workspace**

Modify root `Cargo.toml`, add to members array:
```toml
    "zellij-remote-protocol",
```

**Step 7: Build and verify**

```bash
cargo build -p zellij-remote-protocol
```
Expected: Build succeeds, no codegen path errors

**Step 8: Commit**

```bash
git add zellij-remote-protocol/ Cargo.toml
git commit -m "feat(remote): add zellij-remote-protocol with correct build.rs codegen"
```

---

### Task 0.2: Add Encoding Helpers and Stream Framing

**Files:**
- Create: `zellij-remote-protocol/src/encoding.rs`
- Create: `zellij-remote-protocol/src/error.rs`
- Create: `zellij-remote-protocol/src/framing.rs`
- Modify: `zellij-remote-protocol/src/lib.rs`
- Create: `zellij-remote-protocol/src/tests/mod.rs`
- Create: `zellij-remote-protocol/src/tests/framing_tests.rs`

**Step 1: Write failing test for stream framing with partial reads**

Create `zellij-remote-protocol/src/tests/mod.rs`:
```rust
mod framing_tests;
```

Create `zellij-remote-protocol/src/tests/framing_tests.rs`:
```rust
use crate::framing::{StreamFramer, FrameResult};
use crate::{StreamEnvelope, Ping, stream_envelope};
use bytes::BytesMut;

#[test]
fn test_frame_roundtrip() {
    let mut framer = StreamFramer::new();
    
    let envelope = StreamEnvelope {
        msg: Some(stream_envelope::Msg::Ping(Ping {
            ping_id: 42,
            client_time_ms: 1000,
        })),
    };
    
    let encoded = framer.encode(&envelope).unwrap();
    let decoded = framer.decode_complete(&encoded).unwrap();
    
    assert_eq!(envelope, decoded);
}

#[test]
fn test_partial_read_buffering() {
    let mut framer = StreamFramer::new();
    
    let envelope = StreamEnvelope {
        msg: Some(stream_envelope::Msg::Ping(Ping {
            ping_id: 42,
            client_time_ms: 1000,
        })),
    };
    
    let encoded = framer.encode(&envelope).unwrap();
    
    // Feed bytes one at a time
    let mut buffer = BytesMut::new();
    for (i, byte) in encoded.iter().enumerate() {
        buffer.extend_from_slice(&[*byte]);
        let result = framer.decode(&mut buffer);
        
        if i < encoded.len() - 1 {
            // Should need more data
            assert!(matches!(result, Ok(None)));
        } else {
            // Final byte should complete the frame
            let decoded = result.unwrap().unwrap();
            assert_eq!(envelope, decoded);
        }
    }
}

#[test]
fn test_corrupted_length_rejected() {
    let mut framer = StreamFramer::new();
    
    // Varint claiming huge length
    let mut bad_data = BytesMut::from(&[0xFF, 0xFF, 0xFF, 0xFF, 0x0F][..]);
    let result = framer.decode(&mut bad_data);
    
    assert!(result.is_err());
}
```

**Step 2: Run test to verify it fails**

```bash
cargo test -p zellij-remote-protocol -- framing_tests
```
Expected: FAIL

**Step 3: Create error module**

Create `zellij-remote-protocol/src/error.rs`:
```rust
use std::fmt;

#[derive(Debug)]
pub enum ProtocolError {
    EncodingError(prost::EncodeError),
    DecodingError(prost::DecodeError),
    FrameTooLarge { size: usize, max: usize },
    InvalidVarint,
    EmptyEnvelope,
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EncodingError(e) => write!(f, "encoding error: {}", e),
            Self::DecodingError(e) => write!(f, "decoding error: {}", e),
            Self::FrameTooLarge { size, max } => {
                write!(f, "frame too large: {} bytes (max: {})", size, max)
            }
            Self::InvalidVarint => write!(f, "invalid varint in frame header"),
            Self::EmptyEnvelope => write!(f, "empty envelope"),
        }
    }
}

impl std::error::Error for ProtocolError {}

impl From<prost::EncodeError> for ProtocolError {
    fn from(e: prost::EncodeError) -> Self {
        Self::EncodingError(e)
    }
}

impl From<prost::DecodeError> for ProtocolError {
    fn from(e: prost::DecodeError) -> Self {
        Self::DecodingError(e)
    }
}
```

**Step 4: Create framing module**

Create `zellij-remote-protocol/src/framing.rs`:
```rust
use bytes::{Buf, BufMut, BytesMut};
use prost::Message;
use crate::error::ProtocolError;
use crate::{StreamEnvelope, DatagramEnvelope};

const MAX_FRAME_SIZE: usize = 16 * 1024 * 1024; // 16MB

pub struct StreamFramer {
    max_frame_size: usize,
}

impl StreamFramer {
    pub fn new() -> Self {
        Self {
            max_frame_size: MAX_FRAME_SIZE,
        }
    }

    pub fn with_max_size(max_frame_size: usize) -> Self {
        Self { max_frame_size }
    }

    pub fn encode(&self, envelope: &StreamEnvelope) -> Result<Vec<u8>, ProtocolError> {
        let len = envelope.encoded_len();
        let mut buf = BytesMut::with_capacity(len + 5);
        prost::encoding::encode_varint(len as u64, &mut buf);
        envelope.encode(&mut buf)?;
        Ok(buf.to_vec())
    }

    pub fn decode(&self, buf: &mut BytesMut) -> Result<Option<StreamEnvelope>, ProtocolError> {
        if buf.is_empty() {
            return Ok(None);
        }

        // Try to read varint length without consuming
        let mut peek = &buf[..];
        let len = match prost::encoding::decode_varint(&mut peek) {
            Ok(len) => len as usize,
            Err(_) => {
                // Could be incomplete varint or invalid
                if buf.len() >= 10 {
                    // Varint is at most 10 bytes; if we have 10 and still fail, it's invalid
                    return Err(ProtocolError::InvalidVarint);
                }
                return Ok(None); // Need more data
            }
        };

        if len > self.max_frame_size {
            return Err(ProtocolError::FrameTooLarge {
                size: len,
                max: self.max_frame_size,
            });
        }

        let varint_len = buf.len() - peek.len();
        let total_len = varint_len + len;

        if buf.len() < total_len {
            return Ok(None); // Need more data
        }

        // Consume varint
        buf.advance(varint_len);
        
        // Consume and decode message
        let msg_bytes = buf.split_to(len);
        let envelope = StreamEnvelope::decode(msg_bytes)?;
        
        Ok(Some(envelope))
    }

    pub fn decode_complete(&self, data: &[u8]) -> Result<StreamEnvelope, ProtocolError> {
        let mut buf = BytesMut::from(data);
        self.decode(&mut buf)?
            .ok_or(ProtocolError::DecodingError(prost::DecodeError::new("incomplete frame")))
    }
}

impl Default for StreamFramer {
    fn default() -> Self {
        Self::new()
    }
}

pub fn encode_datagram(envelope: &DatagramEnvelope) -> Result<Vec<u8>, ProtocolError> {
    let mut buf = Vec::with_capacity(envelope.encoded_len());
    envelope.encode(&mut buf)?;
    Ok(buf)
}

pub fn decode_datagram(bytes: &[u8]) -> Result<DatagramEnvelope, ProtocolError> {
    DatagramEnvelope::decode(bytes).map_err(Into::into)
}
```

**Step 5: Update lib.rs**

Modify `zellij-remote-protocol/src/lib.rs`:
```rust
pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/zellij.remote.v1.rs"));
}

pub mod error;
pub mod framing;

#[cfg(test)]
mod tests;

pub use proto::*;
pub use error::ProtocolError;
pub use framing::{StreamFramer, encode_datagram, decode_datagram};

pub const ZRP_VERSION_MAJOR: u32 = 1;
pub const ZRP_VERSION_MINOR: u32 = 0;
pub const DEFAULT_MAX_DATAGRAM_BYTES: u32 = 1200;
pub const DEFAULT_RENDER_WINDOW: u32 = 4;
pub const DEFAULT_SNAPSHOT_INTERVAL_MS: u32 = 5000;
```

**Step 6: Run tests**

```bash
cargo test -p zellij-remote-protocol -- framing_tests
```
Expected: PASS

**Step 7: Commit**

```bash
git add zellij-remote-protocol/src/
git commit -m "feat(remote): add stream framing with partial read support"
```

---

## Phase 1: Plumbing Spike (Prove Integration Early)

> **Goal:** Prove we can (a) accept QUIC connection, (b) authenticate, (c) attach to Zellij session, (d) send something to client. This de-risks integration before building complex logic.

### Task 1.1: Create zellij-remote-bridge Crate with WebTransport Server

**Files:**
- Create: `zellij-remote-bridge/Cargo.toml`
- Create: `zellij-remote-bridge/src/lib.rs`
- Create: `zellij-remote-bridge/src/config.rs`
- Create: `zellij-remote-bridge/src/server.rs`
- Modify: `Cargo.toml` (workspace)

**Step 1: Create crate structure**

```bash
mkdir -p zellij-remote-bridge/src
```

**Step 2: Create Cargo.toml**

Create `zellij-remote-bridge/Cargo.toml`:
```toml
[package]
name = "zellij-remote-bridge"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
zellij-remote-protocol = { path = "../zellij-remote-protocol" }
zellij-utils = { workspace = true }

tokio = { workspace = true }
wtransport = "0.5"
anyhow = { workspace = true }
log = { workspace = true }
thiserror = { workspace = true }
bytes = "1.5"
dashmap = "5.5"
rustls = "0.23"
rcgen = "0.13"

[dev-dependencies]
tokio-test = "0.4"
tempfile = { workspace = true }
```

**Step 3: Create config**

Create `zellij-remote-bridge/src/config.rs`:
```rust
use std::net::SocketAddr;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct BridgeConfig {
    pub listen_addr: SocketAddr,
    pub tls_cert: Option<PathBuf>,
    pub tls_key: Option<PathBuf>,
    pub max_clients_per_session: usize,
    pub render_window: u32,
    pub controller_lease_duration_ms: u32,
}

impl Default for BridgeConfig {
    fn default() -> Self {
        Self {
            listen_addr: "127.0.0.1:4433".parse().unwrap(),
            tls_cert: None,
            tls_key: None,
            max_clients_per_session: 10,
            render_window: 4,
            controller_lease_duration_ms: 30000,
        }
    }
}
```

**Step 4: Create server skeleton**

Create `zellij-remote-bridge/src/server.rs`:
```rust
use std::sync::Arc;
use anyhow::{Context, Result};
use wtransport::{Endpoint, ServerConfig};
use wtransport::tls::Certificate;
use bytes::BytesMut;

use zellij_remote_protocol::{
    StreamFramer, StreamEnvelope, ServerHello, ClientHello,
    ProtocolVersion, Capabilities, SessionState, ControllerLease,
    stream_envelope, ControllerPolicy,
};
use crate::config::BridgeConfig;

pub struct RemoteBridge {
    config: BridgeConfig,
}

impl RemoteBridge {
    pub fn new(config: BridgeConfig) -> Self {
        Self { config }
    }

    pub async fn run(&self) -> Result<()> {
        let tls_config = self.build_tls_config()?;
        
        let config = ServerConfig::builder()
            .with_bind_address(self.config.listen_addr)
            .with_certificate(tls_config)
            .build();

        let server = Endpoint::server(config)?;
        
        log::info!("WebTransport server listening on {}", self.config.listen_addr);

        loop {
            let incoming = server.accept().await;
            let session_request = incoming.await?;
            
            log::info!("Incoming connection from {}", session_request.authority());
            
            // Accept the WebTransport session
            let connection = session_request.accept().await?;
            
            tokio::spawn(async move {
                if let Err(e) = Self::handle_connection(connection).await {
                    log::error!("Connection error: {}", e);
                }
            });
        }
    }

    async fn handle_connection(connection: wtransport::Connection) -> Result<()> {
        // Accept the control stream (bidirectional)
        let (mut send, mut recv) = connection
            .accept_bi()
            .await?
            .ok_or_else(|| anyhow::anyhow!("connection closed before control stream"))?;

        let framer = StreamFramer::new();
        let mut buffer = BytesMut::new();

        // Read ClientHello
        loop {
            let mut chunk = [0u8; 1024];
            let n = recv.read(&mut chunk).await?.unwrap_or(0);
            if n == 0 {
                anyhow::bail!("connection closed during handshake");
            }
            buffer.extend_from_slice(&chunk[..n]);

            if let Some(envelope) = framer.decode(&mut buffer)? {
                match envelope.msg {
                    Some(stream_envelope::Msg::ClientHello(hello)) => {
                        log::info!("Received ClientHello from {}", hello.client_name);
                        
                        // Send ServerHello
                        let response = Self::build_server_hello(&hello);
                        let encoded = framer.encode(&response)?;
                        send.write_all(&encoded).await?;
                        
                        log::info!("Sent ServerHello, handshake complete (spike)");
                        
                        // For spike: just keep connection alive
                        // Real implementation will proceed to attach
                        tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
                        return Ok(());
                    }
                    _ => {
                        anyhow::bail!("expected ClientHello, got other message");
                    }
                }
            }
        }
    }

    fn build_server_hello(client_hello: &ClientHello) -> StreamEnvelope {
        let negotiated_caps = Capabilities {
            supports_datagrams: client_hello
                .capabilities
                .as_ref()
                .map(|c| c.supports_datagrams)
                .unwrap_or(false),
            max_datagram_bytes: zellij_remote_protocol::DEFAULT_MAX_DATAGRAM_BYTES,
            supports_style_dictionary: true,
            supports_styled_underlines: false,
            supports_prediction: true,
            supports_images: false,
            supports_clipboard: false,
            supports_hyperlinks: false,
        };

        StreamEnvelope {
            msg: Some(stream_envelope::Msg::ServerHello(ServerHello {
                negotiated_version: Some(ProtocolVersion {
                    major: zellij_remote_protocol::ZRP_VERSION_MAJOR,
                    minor: zellij_remote_protocol::ZRP_VERSION_MINOR,
                }),
                negotiated_capabilities: Some(negotiated_caps),
                client_id: 1,
                session_name: "spike-session".to_string(),
                session_state: SessionState::Running.into(),
                lease: Some(ControllerLease {
                    lease_id: 0,
                    owner_client_id: 0,
                    policy: ControllerPolicy::LastWriterWins.into(),
                    current_size: None,
                    remaining_ms: 0,
                    duration_ms: 30000,
                }),
                resume_token: vec![],
                snapshot_interval_ms: zellij_remote_protocol::DEFAULT_SNAPSHOT_INTERVAL_MS,
                max_inflight_inputs: 256,
                render_window: zellij_remote_protocol::DEFAULT_RENDER_WINDOW,
            })),
        }
    }

    fn build_tls_config(&self) -> Result<Certificate> {
        match (&self.config.tls_cert, &self.config.tls_key) {
            (Some(cert_path), Some(key_path)) => {
                let cert_pem = std::fs::read_to_string(cert_path)
                    .context("failed to read TLS certificate")?;
                let key_pem = std::fs::read_to_string(key_path)
                    .context("failed to read TLS key")?;
                Certificate::new(vec![cert_pem], key_pem)
                    .map_err(|e| anyhow::anyhow!("invalid certificate: {}", e))
            }
            _ => {
                // Generate self-signed cert for development
                log::warn!("No TLS cert configured, generating self-signed certificate");
                let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
                    .context("failed to generate self-signed cert")?;
                let cert_pem = cert.cert.pem();
                let key_pem = cert.key_pair.serialize_pem();
                Certificate::new(vec![cert_pem], key_pem)
                    .map_err(|e| anyhow::anyhow!("invalid certificate: {}", e))
            }
        }
    }
}
```

**Step 5: Create lib.rs**

Create `zellij-remote-bridge/src/lib.rs`:
```rust
pub mod config;
pub mod server;

pub use config::BridgeConfig;
pub use server::RemoteBridge;
```

**Step 6: Add to workspace and build**

Modify root `Cargo.toml`, add to members:
```toml
    "zellij-remote-bridge",
```

```bash
cargo build -p zellij-remote-bridge
```
Expected: Build succeeds

**Step 7: Commit**

```bash
git add zellij-remote-bridge/ Cargo.toml
git commit -m "feat(remote): add WebTransport server skeleton for plumbing spike"
```

---

### Task 1.2: Add Minimal Test Client for Spike

**Files:**
- Create: `zellij-remote-bridge/examples/spike_client.rs`
- Create: `zellij-remote-bridge/examples/spike_server.rs`

**Step 1: Create spike server binary**

Create `zellij-remote-bridge/examples/spike_server.rs`:
```rust
use anyhow::Result;
use zellij_remote_bridge::{BridgeConfig, RemoteBridge};

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();
    
    let config = BridgeConfig::default();
    let bridge = RemoteBridge::new(config);
    
    println!("Starting spike server on 127.0.0.1:4433");
    bridge.run().await
}
```

**Step 2: Create spike client**

Create `zellij-remote-bridge/examples/spike_client.rs`:
```rust
use anyhow::Result;
use bytes::BytesMut;
use wtransport::{Endpoint, ClientConfig};
use zellij_remote_protocol::{
    StreamFramer, StreamEnvelope, ClientHello, ProtocolVersion, Capabilities,
    stream_envelope,
};

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();
    
    // Accept any certificate for testing
    let config = ClientConfig::builder()
        .with_bind_default()
        .with_no_cert_validation()
        .build();

    let connection = Endpoint::client(config)?
        .connect("https://127.0.0.1:4433")
        .await?;

    println!("Connected to server");

    // Open control stream
    let (mut send, mut recv) = connection.open_bi().await?;
    let framer = StreamFramer::new();

    // Send ClientHello
    let hello = StreamEnvelope {
        msg: Some(stream_envelope::Msg::ClientHello(ClientHello {
            version: Some(ProtocolVersion { major: 1, minor: 0 }),
            capabilities: Some(Capabilities {
                supports_datagrams: true,
                max_datagram_bytes: 1200,
                supports_style_dictionary: true,
                supports_styled_underlines: false,
                supports_prediction: true,
                supports_images: false,
                supports_clipboard: false,
                supports_hyperlinks: false,
            }),
            client_name: "spike-client".to_string(),
            bearer_token: vec![],
            resume_token: vec![],
        })),
    };

    let encoded = framer.encode(&hello)?;
    send.write_all(&encoded).await?;
    println!("Sent ClientHello");

    // Read ServerHello
    let mut buffer = BytesMut::new();
    loop {
        let mut chunk = [0u8; 1024];
        let n = recv.read(&mut chunk).await?.unwrap_or(0);
        if n == 0 {
            anyhow::bail!("connection closed");
        }
        buffer.extend_from_slice(&chunk[..n]);

        if let Some(envelope) = framer.decode(&mut buffer)? {
            match envelope.msg {
                Some(stream_envelope::Msg::ServerHello(hello)) => {
                    println!("Received ServerHello!");
                    println!("  Session: {}", hello.session_name);
                    println!("  Client ID: {}", hello.client_id);
                    println!("SPIKE SUCCESS: Handshake complete!");
                    return Ok(());
                }
                other => {
                    println!("Unexpected message: {:?}", other);
                }
            }
        }
    }
}
```

**Step 3: Update Cargo.toml for examples**

Add to `zellij-remote-bridge/Cargo.toml`:
```toml
[[example]]
name = "spike_server"

[[example]]
name = "spike_client"

[dev-dependencies]
tokio-test = "0.4"
tempfile = { workspace = true }
env_logger = "0.10"
```

**Step 4: Test the spike**

Terminal 1:
```bash
RUST_LOG=info cargo run -p zellij-remote-bridge --example spike_server
```

Terminal 2:
```bash
RUST_LOG=info cargo run -p zellij-remote-bridge --example spike_client
```

Expected: Client prints "SPIKE SUCCESS: Handshake complete!"

**Step 5: Commit**

```bash
git add zellij-remote-bridge/
git commit -m "feat(remote): add spike client/server to prove WebTransport plumbing"
```

---

### Task 1.3: Integrate with Zellij Session IPC

**Files:**
- Create: `zellij-remote-bridge/src/session_attach.rs`
- Modify: `zellij-remote-bridge/src/server.rs`
- Modify: `zellij-remote-bridge/src/lib.rs`

**Step 1: Create session attach module**

Create `zellij-remote-bridge/src/session_attach.rs`:
```rust
use std::path::PathBuf;
use anyhow::{Context, Result};
use zellij_utils::ipc::{ClientToServerMsg, ServerToClientMsg};
use zellij_utils::channels::{SenderWithContext, bounded};

/// Represents an attachment to a running Zellij session
pub struct SessionAttachment {
    session_name: String,
    socket_path: PathBuf,
    // Will hold IPC channels when fully implemented
}

impl SessionAttachment {
    /// List available Zellij sessions
    pub fn list_sessions() -> Result<Vec<String>> {
        let sessions_dir = zellij_utils::consts::ZELLIJ_SOCK_DIR.clone();
        
        if !sessions_dir.exists() {
            return Ok(vec![]);
        }

        let mut sessions = Vec::new();
        for entry in std::fs::read_dir(&sessions_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    sessions.push(name.to_string());
                }
            }
        }
        Ok(sessions)
    }

    /// Attach to an existing session by name
    pub fn attach(session_name: &str) -> Result<Self> {
        let sessions_dir = zellij_utils::consts::ZELLIJ_SOCK_DIR.clone();
        let socket_path = sessions_dir.join(session_name);

        if !socket_path.exists() {
            anyhow::bail!("session '{}' not found", session_name);
        }

        log::info!("Attaching to session '{}' at {:?}", session_name, socket_path);

        Ok(Self {
            session_name: session_name.to_string(),
            socket_path,
        })
    }

    pub fn session_name(&self) -> &str {
        &self.session_name
    }

    /// Check if the session is still alive
    pub fn is_alive(&self) -> bool {
        self.socket_path.exists()
    }
}
```

**Step 2: Update server to use session attach**

Modify `zellij-remote-bridge/src/server.rs`, update `handle_connection`:
```rust
use crate::session_attach::SessionAttachment;

// In handle_connection, after receiving ClientHello:
// ... existing code ...

// Add session attach logic (commented for spike, will enable in Phase 2):
// let sessions = SessionAttachment::list_sessions()?;
// log::info!("Available sessions: {:?}", sessions);

// For now, just verify we can import the module
let _ = SessionAttachment::list_sessions();
```

**Step 3: Update lib.rs**

Modify `zellij-remote-bridge/src/lib.rs`:
```rust
pub mod config;
pub mod server;
pub mod session_attach;

pub use config::BridgeConfig;
pub use server::RemoteBridge;
pub use session_attach::SessionAttachment;
```

**Step 4: Build and test**

```bash
cargo build -p zellij-remote-bridge
```

**Step 5: Commit**

```bash
git add zellij-remote-bridge/src/
git commit -m "feat(remote): add session attachment module for Zellij IPC integration"
```

---

## Phase 2: Frame Store & State Sync (Correctness-First)

### Task 2.1: Create zellij-remote-core Crate with Frame Store

**Files:**
- Create: `zellij-remote-core/Cargo.toml`
- Create: `zellij-remote-core/src/lib.rs`
- Create: `zellij-remote-core/src/frame.rs`
- Create: `zellij-remote-core/src/style_table.rs`
- Modify: `Cargo.toml` (workspace)

**Step 1: Create crate**

```bash
mkdir -p zellij-remote-core/src
```

**Step 2: Create Cargo.toml**

Create `zellij-remote-core/Cargo.toml`:
```toml
[package]
name = "zellij-remote-core"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
zellij-remote-protocol = { path = "../zellij-remote-protocol" }
unicode-width = { workspace = true }

[dev-dependencies]
proptest = "1.4"
```

**Step 3: Create style table with O(1) lookup**

Create `zellij-remote-core/src/style_table.rs`:
```rust
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use zellij_remote_protocol::Style;

/// Key for reverse lookup - hashable representation of Style
#[derive(Clone, PartialEq, Eq)]
pub struct StyleKey {
    // Serialized form for hashing
    bytes: Vec<u8>,
}

impl StyleKey {
    pub fn from_style(style: &Style) -> Self {
        use prost::Message;
        let mut bytes = Vec::new();
        style.encode(&mut bytes).unwrap();
        Self { bytes }
    }
}

impl Hash for StyleKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.bytes.hash(state);
    }
}

/// Style table with O(1) insertion and lookup
pub struct StyleTable {
    forward: Vec<Style>,              // style_id -> Style
    reverse: HashMap<StyleKey, u16>,  // Style -> style_id
    epoch: u32,
}

impl StyleTable {
    pub fn new() -> Self {
        let mut table = Self {
            forward: Vec::new(),
            reverse: HashMap::new(),
            epoch: 0,
        };
        // ID 0 = default style
        table.forward.push(Style::default());
        table.reverse.insert(StyleKey::from_style(&Style::default()), 0);
        table
    }

    pub fn get_or_insert(&mut self, style: &Style) -> u16 {
        let key = StyleKey::from_style(style);
        
        if let Some(&id) = self.reverse.get(&key) {
            return id;
        }

        // Check if we're near u16 exhaustion
        if self.forward.len() >= (u16::MAX - 1000) as usize {
            self.reset();
            return self.get_or_insert(style);
        }

        let id = self.forward.len() as u16;
        self.forward.push(style.clone());
        self.reverse.insert(key, id);
        id
    }

    pub fn get(&self, id: u16) -> Option<&Style> {
        self.forward.get(id as usize)
    }

    pub fn epoch(&self) -> u32 {
        self.epoch
    }

    pub fn needs_reset(&self) -> bool {
        self.forward.len() >= (u16::MAX - 1000) as usize
    }

    pub fn reset(&mut self) {
        self.forward.clear();
        self.reverse.clear();
        self.epoch += 1;
        // Re-add default
        self.forward.push(Style::default());
        self.reverse.insert(StyleKey::from_style(&Style::default()), 0);
    }

    pub fn iter(&self) -> impl Iterator<Item = (u16, &Style)> {
        self.forward.iter().enumerate().map(|(i, s)| (i as u16, s))
    }

    pub fn len(&self) -> usize {
        self.forward.len()
    }
}

impl Default for StyleTable {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zellij_remote_protocol::{Color, Rgb, color};

    #[test]
    fn test_get_or_insert_returns_same_id() {
        let mut table = StyleTable::new();
        
        let style = Style {
            fg: Some(Color {
                value: Some(color::Value::Rgb(Rgb { r: 255, g: 0, b: 0 })),
            }),
            ..Default::default()
        };
        
        let id1 = table.get_or_insert(&style);
        let id2 = table.get_or_insert(&style);
        
        assert_eq!(id1, id2);
    }

    #[test]
    fn test_different_styles_get_different_ids() {
        let mut table = StyleTable::new();
        
        let style1 = Style {
            bold: true,
            ..Default::default()
        };
        let style2 = Style {
            italic: true,
            ..Default::default()
        };
        
        let id1 = table.get_or_insert(&style1);
        let id2 = table.get_or_insert(&style2);
        
        assert_ne!(id1, id2);
    }
}
```

**Step 4: Create frame module with Arc<Row>**

Create `zellij-remote-core/src/frame.rs`:
```rust
use std::sync::Arc;
use zellij_remote_protocol::{DisplaySize, CursorState, CursorShape};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cell {
    pub codepoint: u32,
    pub width: u8,
    pub style_id: u16,
}

impl Cell {
    pub fn empty() -> Self {
        Self {
            codepoint: ' ' as u32,
            width: 1,
            style_id: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RowData {
    pub cells: Vec<Cell>,
}

impl RowData {
    pub fn new(cols: usize) -> Self {
        Self {
            cells: vec![Cell::empty(); cols],
        }
    }
}

/// A single terminal row wrapped in Arc for sharing
pub type Row = Arc<RowData>;

#[derive(Debug, Clone)]
pub struct Cursor {
    pub row: u32,
    pub col: u32,
    pub visible: bool,
    pub blink: bool,
    pub shape: CursorShape,
}

impl Default for Cursor {
    fn default() -> Self {
        Self {
            row: 0,
            col: 0,
            visible: true,
            blink: true,
            shape: CursorShape::Block,
        }
    }
}

impl From<&Cursor> for CursorState {
    fn from(c: &Cursor) -> Self {
        CursorState {
            row: c.row,
            col: c.col,
            visible: c.visible,
            blink: c.blink,
            shape: c.shape.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FrameData {
    pub rows: Vec<Row>,
    pub cols: usize,
    pub cursor: Cursor,
    pub style_epoch: u32,
}

impl FrameData {
    pub fn new(cols: usize, rows: usize) -> Self {
        Self {
            rows: (0..rows).map(|_| Arc::new(RowData::new(cols))).collect(),
            cols,
            cursor: Cursor::default(),
            style_epoch: 0,
        }
    }

    pub fn size(&self) -> DisplaySize {
        DisplaySize {
            cols: self.cols as u32,
            rows: self.rows.len() as u32,
        }
    }
}

/// Immutable frame wrapper with state_id
pub type Frame = Arc<FrameData>;

/// Ring buffer of recent frames for delta computation
pub struct FrameStore {
    frames: Vec<(u64, Frame)>,  // (state_id, frame)
    max_frames: usize,
    current_state_id: u64,
}

impl FrameStore {
    pub fn new(max_frames: usize) -> Self {
        Self {
            frames: Vec::with_capacity(max_frames),
            max_frames,
            current_state_id: 0,
        }
    }

    pub fn current_state_id(&self) -> u64 {
        self.current_state_id
    }

    pub fn current(&self) -> Option<&Frame> {
        self.frames.last().map(|(_, f)| f)
    }

    pub fn get(&self, state_id: u64) -> Option<&Frame> {
        self.frames.iter().find(|(id, _)| *id == state_id).map(|(_, f)| f)
    }

    pub fn push(&mut self, frame: Frame) -> u64 {
        self.current_state_id += 1;
        let state_id = self.current_state_id;

        if self.frames.len() >= self.max_frames {
            self.frames.remove(0);
        }
        self.frames.push((state_id, frame));

        state_id
    }

    pub fn oldest_state_id(&self) -> Option<u64> {
        self.frames.first().map(|(id, _)| *id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_frame_store_ring_buffer() {
        let mut store = FrameStore::new(3);
        
        for _ in 0..5 {
            let frame = Arc::new(FrameData::new(80, 24));
            store.push(frame);
        }

        assert_eq!(store.frames.len(), 3);
        assert_eq!(store.current_state_id(), 5);
        assert_eq!(store.oldest_state_id(), Some(3));
    }

    #[test]
    fn test_row_arc_sharing() {
        let row1: Row = Arc::new(RowData::new(80));
        let row2 = Arc::clone(&row1);
        
        // Both point to same data
        assert!(Arc::ptr_eq(&row1, &row2));
    }
}
```

**Step 5: Create lib.rs**

Create `zellij-remote-core/src/lib.rs`:
```rust
pub mod frame;
pub mod style_table;

pub use frame::{Cell, RowData, Row, Cursor, FrameData, Frame, FrameStore};
pub use style_table::StyleTable;
```

**Step 6: Add to workspace**

Modify root `Cargo.toml`:
```toml
    "zellij-remote-core",
```

**Step 7: Build and test**

```bash
cargo test -p zellij-remote-core
```
Expected: All tests pass

**Step 8: Commit**

```bash
git add zellij-remote-core/ Cargo.toml
git commit -m "feat(remote): add frame store with Arc<Row> sharing and O(1) style table"
```

---

### Task 2.2: Implement Delta Engine with Arc Pointer Equality

**Files:**
- Create: `zellij-remote-core/src/delta.rs`
- Create: `zellij-remote-core/src/tests/mod.rs`
- Create: `zellij-remote-core/src/tests/delta_tests.rs`
- Modify: `zellij-remote-core/src/lib.rs`

**Step 1: Write failing test**

Create `zellij-remote-core/src/tests/mod.rs`:
```rust
mod delta_tests;
```

Create `zellij-remote-core/src/tests/delta_tests.rs`:
```rust
use std::sync::Arc;
use crate::frame::{FrameData, RowData, Row, Cell};
use crate::delta::DeltaEngine;

#[test]
fn test_identical_frames_no_patches() {
    let frame1 = Arc::new(FrameData::new(80, 24));
    let frame2 = Arc::clone(&frame1);
    
    let delta = DeltaEngine::compute(&frame1, 1, &frame2, 2);
    
    // Same Arc = no changes
    assert!(delta.row_patches.is_empty());
}

#[test]
fn test_single_row_change() {
    let mut frame1 = FrameData::new(80, 24);
    
    // Create frame2 with one modified row
    let mut new_row = RowData::new(80);
    new_row.cells[0] = Cell {
        codepoint: 'X' as u32,
        width: 1,
        style_id: 0,
    };
    
    let mut frame2 = frame1.clone();
    frame2.rows[0] = Arc::new(new_row);
    
    let delta = DeltaEngine::compute(
        &Arc::new(frame1), 1,
        &Arc::new(frame2), 2,
    );
    
    assert_eq!(delta.row_patches.len(), 1);
    assert_eq!(delta.row_patches[0].row, 0);
}

#[test]
fn test_arc_pointer_equality_skips_unchanged() {
    let shared_row: Row = Arc::new(RowData::new(80));
    
    let frame1 = Arc::new(FrameData {
        rows: vec![Arc::clone(&shared_row); 24],
        cols: 80,
        cursor: Default::default(),
        style_epoch: 0,
    });
    
    // frame2 shares most rows but changes row 5
    let mut new_rows = frame1.rows.clone();
    let mut changed_row = RowData::new(80);
    changed_row.cells[0].codepoint = 'A' as u32;
    new_rows[5] = Arc::new(changed_row);
    
    let frame2 = Arc::new(FrameData {
        rows: new_rows,
        cols: 80,
        cursor: Default::default(),
        style_epoch: 0,
    });
    
    let delta = DeltaEngine::compute(&frame1, 1, &frame2, 2);
    
    // Only row 5 should be patched
    assert_eq!(delta.row_patches.len(), 1);
    assert_eq!(delta.row_patches[0].row, 5);
}
```

**Step 2: Run test to verify it fails**

```bash
cargo test -p zellij-remote-core -- delta_tests
```
Expected: FAIL

**Step 3: Implement delta engine**

Create `zellij-remote-core/src/delta.rs`:
```rust
use std::sync::Arc;
use crate::frame::{Frame, FrameData, Row, Cell};
use crate::style_table::StyleTable;
use zellij_remote_protocol::{
    ScreenDelta, ScreenSnapshot, RowPatch, CellRun, RowData as ProtoRowData,
    StyleDef, DisplaySize,
};

pub struct DeltaEngine;

impl DeltaEngine {
    /// Compute delta from base to current using Arc pointer equality for fast row comparison
    pub fn compute(
        base: &Frame,
        base_state_id: u64,
        current: &Frame,
        current_state_id: u64,
    ) -> ScreenDelta {
        let mut row_patches = Vec::new();

        let rows_to_compare = base.rows.len().min(current.rows.len());

        for row_idx in 0..rows_to_compare {
            let base_row = &base.rows[row_idx];
            let current_row = &current.rows[row_idx];

            // Fast path: Arc pointer equality means identical row
            if Arc::ptr_eq(base_row, current_row) {
                continue;
            }

            // Slow path: compute cell-level diff
            if let Some(patch) = Self::diff_row(row_idx, base_row, current_row) {
                row_patches.push(patch);
            }
        }

        // Handle added rows
        for row_idx in rows_to_compare..current.rows.len() {
            let patch = Self::row_to_full_patch(row_idx, &current.rows[row_idx]);
            row_patches.push(patch);
        }

        ScreenDelta {
            base_state_id,
            state_id: current_state_id,
            styles_added: vec![], // Caller must populate if needed
            row_patches,
            cursor: Some((&current.cursor).into()),
            delivered_input_watermark: 0,
        }
    }

    fn diff_row(row_idx: usize, base: &Row, current: &Row) -> Option<RowPatch> {
        let mut runs = Vec::new();
        let mut run_start: Option<usize> = None;
        let mut run_cells: Vec<&Cell> = Vec::new();

        let cols = base.cells.len().min(current.cells.len());

        for col in 0..cols {
            let base_cell = &base.cells[col];
            let current_cell = &current.cells[col];

            if base_cell != current_cell {
                if run_start.is_none() {
                    run_start = Some(col);
                }
                run_cells.push(current_cell);
            } else if let Some(start) = run_start.take() {
                runs.push(Self::cells_to_run(start, &run_cells));
                run_cells.clear();
            }
        }

        // Handle extended columns
        for col in cols..current.cells.len() {
            if run_start.is_none() {
                run_start = Some(col);
            }
            run_cells.push(&current.cells[col]);
        }

        if let Some(start) = run_start {
            runs.push(Self::cells_to_run(start, &run_cells));
        }

        if runs.is_empty() {
            None
        } else {
            Some(RowPatch {
                row: row_idx as u32,
                runs,
            })
        }
    }

    fn row_to_full_patch(row_idx: usize, row: &Row) -> RowPatch {
        let run = CellRun {
            col_start: 0,
            codepoints: row.cells.iter().map(|c| c.codepoint).collect(),
            widths: row.cells.iter().map(|c| c.width as u32).collect(),
            style_ids: row.cells.iter().map(|c| c.style_id as u32).collect(),
        };
        RowPatch {
            row: row_idx as u32,
            runs: vec![run],
        }
    }

    fn cells_to_run(col_start: usize, cells: &[&Cell]) -> CellRun {
        CellRun {
            col_start: col_start as u32,
            codepoints: cells.iter().map(|c| c.codepoint).collect(),
            widths: cells.iter().map(|c| c.width as u32).collect(),
            style_ids: cells.iter().map(|c| c.style_id as u32).collect(),
        }
    }

    /// Create a full snapshot
    pub fn create_snapshot(
        frame: &Frame,
        state_id: u64,
        style_table: &StyleTable,
    ) -> ScreenSnapshot {
        let rows: Vec<ProtoRowData> = frame
            .rows
            .iter()
            .enumerate()
            .map(|(idx, row)| ProtoRowData {
                row: idx as u32,
                codepoints: row.cells.iter().map(|c| c.codepoint).collect(),
                widths: row.cells.iter().map(|c| c.width as u32).collect(),
                style_ids: row.cells.iter().map(|c| c.style_id as u32).collect(),
            })
            .collect();

        let styles: Vec<StyleDef> = style_table
            .iter()
            .map(|(id, style)| StyleDef {
                style_id: id as u32,
                style: Some(style.clone()),
            })
            .collect();

        ScreenSnapshot {
            state_id,
            size: Some(frame.size()),
            style_table_reset: true,
            styles,
            rows,
            cursor: Some((&frame.cursor).into()),
            delivered_input_watermark: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snapshot_contains_all_rows() {
        let frame = Arc::new(FrameData::new(80, 24));
        let style_table = StyleTable::new();
        
        let snapshot = DeltaEngine::create_snapshot(&frame, 1, &style_table);
        
        assert_eq!(snapshot.rows.len(), 24);
        assert_eq!(snapshot.size.unwrap().cols, 80);
    }
}
```

**Step 4: Update lib.rs**

Modify `zellij-remote-core/src/lib.rs`:
```rust
pub mod frame;
pub mod style_table;
pub mod delta;

#[cfg(test)]
mod tests;

pub use frame::{Cell, RowData, Row, Cursor, FrameData, Frame, FrameStore};
pub use style_table::StyleTable;
pub use delta::DeltaEngine;
```

**Step 5: Run tests**

```bash
cargo test -p zellij-remote-core
```
Expected: All tests pass

**Step 6: Commit**

```bash
git add zellij-remote-core/src/
git commit -m "feat(remote): add delta engine with Arc pointer equality optimization"
```

---

### Task 2.3: Implement State Sync Invariants and Apply Logic

**Files:**
- Create: `zellij-remote-core/src/sync.rs`
- Create: `zellij-remote-core/src/tests/sync_tests.rs`
- Modify: `zellij-remote-core/src/lib.rs`
- Modify: `zellij-remote-core/src/tests/mod.rs`

**Step 1: Write failing tests for invariants**

Add to `zellij-remote-core/src/tests/mod.rs`:
```rust
mod sync_tests;
```

Create `zellij-remote-core/src/tests/sync_tests.rs`:
```rust
use std::sync::Arc;
use crate::frame::{FrameData, RowData, Cell};
use crate::sync::{ClientSyncState, ApplyResult};
use crate::delta::DeltaEngine;
use zellij_remote_protocol::ScreenDelta;

#[test]
fn test_apply_delta_requires_matching_base() {
    let mut client = ClientSyncState::new(80, 24);
    client.set_state_id(5);
    
    // Delta with wrong base
    let delta = ScreenDelta {
        base_state_id: 3,  // Client has 5, not 3
        state_id: 6,
        styles_added: vec![],
        row_patches: vec![],
        cursor: None,
        delivered_input_watermark: 0,
    };
    
    let result = client.apply_delta(&delta);
    assert!(matches!(result, ApplyResult::BaseMismatch { expected: 5, got: 3 }));
}

#[test]
fn test_apply_delta_advances_state() {
    let mut client = ClientSyncState::new(80, 24);
    client.set_state_id(5);
    
    let delta = ScreenDelta {
        base_state_id: 5,
        state_id: 6,
        styles_added: vec![],
        row_patches: vec![],
        cursor: None,
        delivered_input_watermark: 0,
    };
    
    let result = client.apply_delta(&delta);
    assert!(matches!(result, ApplyResult::Applied));
    assert_eq!(client.state_id(), 6);
}

#[test]
fn test_duplicate_delta_is_idempotent() {
    let mut client = ClientSyncState::new(80, 24);
    client.set_state_id(5);
    
    let delta = ScreenDelta {
        base_state_id: 5,
        state_id: 6,
        styles_added: vec![],
        row_patches: vec![],
        cursor: None,
        delivered_input_watermark: 0,
    };
    
    client.apply_delta(&delta);
    
    // Apply same delta again (duplicate)
    let result = client.apply_delta(&delta);
    assert!(matches!(result, ApplyResult::AlreadyApplied));
    assert_eq!(client.state_id(), 6);
}

#[test]
fn test_snapshot_supersedes_all() {
    let mut client = ClientSyncState::new(80, 24);
    client.set_state_id(5);
    
    // Snapshot jumps to state 100
    let snapshot = zellij_remote_protocol::ScreenSnapshot {
        state_id: 100,
        size: Some(zellij_remote_protocol::DisplaySize { cols: 80, rows: 24 }),
        style_table_reset: true,
        styles: vec![],
        rows: vec![],
        cursor: None,
        delivered_input_watermark: 0,
    };
    
    client.apply_snapshot(&snapshot);
    assert_eq!(client.state_id(), 100);
}
```

**Step 2: Run tests to verify they fail**

```bash
cargo test -p zellij-remote-core -- sync_tests
```
Expected: FAIL

**Step 3: Implement sync module**

Create `zellij-remote-core/src/sync.rs`:
```rust
use std::sync::Arc;
use crate::frame::{FrameData, RowData, Row, Cell, Cursor};
use crate::style_table::StyleTable;
use zellij_remote_protocol::{ScreenDelta, ScreenSnapshot, CursorShape};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyResult {
    Applied,
    AlreadyApplied,
    BaseMismatch { expected: u64, got: u64 },
}

/// Client-side state for sync
pub struct ClientSyncState {
    frame: FrameData,
    style_table: StyleTable,
    state_id: u64,
}

impl ClientSyncState {
    pub fn new(cols: usize, rows: usize) -> Self {
        Self {
            frame: FrameData::new(cols, rows),
            style_table: StyleTable::new(),
            state_id: 0,
        }
    }

    pub fn state_id(&self) -> u64 {
        self.state_id
    }

    pub fn set_state_id(&mut self, id: u64) {
        self.state_id = id;
    }

    pub fn frame(&self) -> &FrameData {
        &self.frame
    }

    pub fn apply_delta(&mut self, delta: &ScreenDelta) -> ApplyResult {
        // Invariant: delta must be based on our current state
        if delta.base_state_id != self.state_id {
            // Check for duplicate (already applied)
            if delta.state_id <= self.state_id {
                return ApplyResult::AlreadyApplied;
            }
            return ApplyResult::BaseMismatch {
                expected: self.state_id,
                got: delta.base_state_id,
            };
        }

        // Apply style updates
        for style_def in &delta.styles_added {
            if let Some(style) = &style_def.style {
                self.style_table.get_or_insert(style);
            }
        }

        // Apply row patches
        for patch in &delta.row_patches {
            let row_idx = patch.row as usize;
            if row_idx >= self.frame.rows.len() {
                continue;
            }

            // Clone the row for modification (copy-on-write)
            let mut row_data = (*self.frame.rows[row_idx]).clone();

            for run in &patch.runs {
                let col_start = run.col_start as usize;
                for (i, ((&cp, &w), &sid)) in run
                    .codepoints
                    .iter()
                    .zip(run.widths.iter())
                    .zip(run.style_ids.iter())
                    .enumerate()
                {
                    let col = col_start + i;
                    if col < row_data.cells.len() {
                        row_data.cells[col] = Cell {
                            codepoint: cp,
                            width: w as u8,
                            style_id: sid as u16,
                        };
                    }
                }
            }

            self.frame.rows[row_idx] = Arc::new(row_data);
        }

        // Update cursor
        if let Some(cursor) = &delta.cursor {
            self.frame.cursor.row = cursor.row;
            self.frame.cursor.col = cursor.col;
            self.frame.cursor.visible = cursor.visible;
            self.frame.cursor.blink = cursor.blink;
            self.frame.cursor.shape = CursorShape::try_from(cursor.shape)
                .unwrap_or(CursorShape::Block);
        }

        self.state_id = delta.state_id;
        ApplyResult::Applied
    }

    pub fn apply_snapshot(&mut self, snapshot: &ScreenSnapshot) {
        // Resize if needed
        if let Some(size) = &snapshot.size {
            let new_cols = size.cols as usize;
            let new_rows = size.rows as usize;
            
            self.frame.rows.resize_with(new_rows, || Arc::new(RowData::new(new_cols)));
            self.frame.cols = new_cols;
        }

        // Reset style table if requested
        if snapshot.style_table_reset {
            self.style_table.reset();
        }

        // Load styles
        for style_def in &snapshot.styles {
            if let Some(style) = &style_def.style {
                self.style_table.get_or_insert(style);
            }
        }

        // Load rows
        for row_data in &snapshot.rows {
            let row_idx = row_data.row as usize;
            if row_idx >= self.frame.rows.len() {
                continue;
            }

            let mut new_row = RowData::new(self.frame.cols);
            for (i, ((&cp, &w), &sid)) in row_data
                .codepoints
                .iter()
                .zip(row_data.widths.iter())
                .zip(row_data.style_ids.iter())
                .enumerate()
            {
                if i < new_row.cells.len() {
                    new_row.cells[i] = Cell {
                        codepoint: cp,
                        width: w as u8,
                        style_id: sid as u16,
                    };
                }
            }
            self.frame.rows[row_idx] = Arc::new(new_row);
        }

        // Update cursor
        if let Some(cursor) = &snapshot.cursor {
            self.frame.cursor.row = cursor.row;
            self.frame.cursor.col = cursor.col;
            self.frame.cursor.visible = cursor.visible;
            self.frame.cursor.blink = cursor.blink;
            self.frame.cursor.shape = CursorShape::try_from(cursor.shape)
                .unwrap_or(CursorShape::Block);
        }

        self.state_id = snapshot.state_id;
    }

    pub fn needs_snapshot(&self) -> bool {
        self.state_id == 0
    }
}
```

**Step 4: Update lib.rs**

Modify `zellij-remote-core/src/lib.rs`:
```rust
pub mod frame;
pub mod style_table;
pub mod delta;
pub mod sync;

#[cfg(test)]
mod tests;

pub use frame::{Cell, RowData, Row, Cursor, FrameData, Frame, FrameStore};
pub use style_table::StyleTable;
pub use delta::DeltaEngine;
pub use sync::{ClientSyncState, ApplyResult};
```

**Step 5: Run tests**

```bash
cargo test -p zellij-remote-core
```
Expected: All tests pass

**Step 6: Commit**

```bash
git add zellij-remote-core/src/
git commit -m "feat(remote): implement state sync invariants with idempotent delta apply"
```

---

### Task 2.4: Implement Datagram-Based Render with Latest-Wins Semantics

> **Why now:** On high-latency lossy networks (200-500ms RTT, 1-10% loss), reliable stream head-of-line blocking dominates perceived latency. Datagrams with "latest-wins" semantics avoid retransmission stalls - this is the core Mosh insight. Our cumulative delta design already did the hard work; this is ~50 lines of routing logic.

**Files:**
- Create: `zellij-remote-core/src/render_seq.rs`
- Create: `zellij-remote-core/src/tests/render_seq_tests.rs`
- Modify: `zellij-remote-core/src/lib.rs`
- Modify: `zellij-remote-core/src/tests/mod.rs`

**Step 1: Write failing test for render sequence tracking**

Add to `zellij-remote-core/src/tests/mod.rs`:
```rust
mod render_seq_tests;
```

Create `zellij-remote-core/src/tests/render_seq_tests.rs`:
```rust
use crate::render_seq::{RenderSeqTracker, DatagramDecision};

#[test]
fn test_newer_seq_accepted() {
    let mut tracker = RenderSeqTracker::new();
    
    assert!(tracker.should_apply(1, 1)); // baseline 1, seq 1
    tracker.mark_applied(1);
    
    assert!(tracker.should_apply(1, 2)); // same baseline, newer seq
}

#[test]
fn test_stale_seq_rejected() {
    let mut tracker = RenderSeqTracker::new();
    
    tracker.mark_applied(5);
    
    // Older sequence should be rejected
    assert!(!tracker.should_apply(1, 3));
}

#[test]
fn test_wrong_baseline_rejected() {
    let mut tracker = RenderSeqTracker::new();
    tracker.set_baseline(10);
    
    // Delta based on wrong baseline
    assert!(!tracker.should_apply(5, 1));
}

#[test]
fn test_datagram_vs_stream_decision() {
    let tracker = RenderSeqTracker::new();
    
    // Small delta -> datagram
    let small_payload = vec![0u8; 500];
    assert!(matches!(
        tracker.decide_transport(&small_payload, 1200, true),
        DatagramDecision::Datagram
    ));
    
    // Large delta -> stream
    let large_payload = vec![0u8; 2000];
    assert!(matches!(
        tracker.decide_transport(&large_payload, 1200, true),
        DatagramDecision::Stream
    ));
    
    // Datagrams not supported -> stream
    assert!(matches!(
        tracker.decide_transport(&small_payload, 1200, false),
        DatagramDecision::Stream
    ));
}
```

**Step 2: Run test to verify it fails**

```bash
cargo test -p zellij-remote-core -- render_seq_tests
```
Expected: FAIL

**Step 3: Implement render sequence tracker**

Create `zellij-remote-core/src/render_seq.rs`:
```rust
/// Tracks render sequence for latest-wins datagram semantics
#[derive(Debug)]
pub struct RenderSeqTracker {
    last_applied_seq: u64,
    current_baseline_id: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatagramDecision {
    Datagram,
    Stream,
}

impl RenderSeqTracker {
    pub fn new() -> Self {
        Self {
            last_applied_seq: 0,
            current_baseline_id: 0,
        }
    }

    /// Check if a render update should be applied (latest-wins)
    pub fn should_apply(&self, baseline_id: u64, render_seq: u64) -> bool {
        // Reject if based on wrong baseline
        if baseline_id != self.current_baseline_id && self.current_baseline_id != 0 {
            return false;
        }
        
        // Reject if stale (already have newer)
        if render_seq <= self.last_applied_seq {
            return false;
        }
        
        true
    }

    /// Mark a render sequence as applied
    pub fn mark_applied(&mut self, render_seq: u64) {
        if render_seq > self.last_applied_seq {
            self.last_applied_seq = render_seq;
        }
    }

    /// Set baseline after snapshot
    pub fn set_baseline(&mut self, baseline_id: u64) {
        self.current_baseline_id = baseline_id;
        // Don't reset last_applied_seq - snapshots use state_id not render_seq
    }

    /// Reset after snapshot (new baseline established)
    pub fn reset_for_snapshot(&mut self, new_baseline_id: u64) {
        self.current_baseline_id = new_baseline_id;
        self.last_applied_seq = 0;
    }

    pub fn last_applied_seq(&self) -> u64 {
        self.last_applied_seq
    }

    pub fn current_baseline_id(&self) -> u64 {
        self.current_baseline_id
    }

    /// Decide whether to send via datagram or stream
    pub fn decide_transport(
        &self,
        encoded_payload: &[u8],
        max_datagram_bytes: u32,
        supports_datagrams: bool,
    ) -> DatagramDecision {
        if !supports_datagrams {
            return DatagramDecision::Stream;
        }
        
        if encoded_payload.len() <= max_datagram_bytes as usize {
            DatagramDecision::Datagram
        } else {
            DatagramDecision::Stream
        }
    }
}

impl Default for RenderSeqTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Server-side render sender that routes to datagram or stream
pub struct RenderSender {
    next_render_seq: u64,
}

impl RenderSender {
    pub fn new() -> Self {
        Self { next_render_seq: 1 }
    }

    /// Get next render sequence number
    pub fn next_seq(&mut self) -> u64 {
        let seq = self.next_render_seq;
        self.next_render_seq += 1;
        seq
    }

    /// Reset sequence (e.g., after baseline change)
    pub fn reset(&mut self) {
        self.next_render_seq = 1;
    }
}

impl Default for RenderSender {
    fn default() -> Self {
        Self::new()
    }
}
```

**Step 4: Update lib.rs**

Modify `zellij-remote-core/src/lib.rs`:
```rust
pub mod frame;
pub mod style_table;
pub mod delta;
pub mod sync;
pub mod render_seq;

#[cfg(test)]
mod tests;

pub use frame::{Cell, RowData, Row, Cursor, FrameData, Frame, FrameStore};
pub use style_table::StyleTable;
pub use delta::DeltaEngine;
pub use sync::{ClientSyncState, ApplyResult};
pub use render_seq::{RenderSeqTracker, RenderSender, DatagramDecision};
```

**Step 5: Run tests**

```bash
cargo test -p zellij-remote-core -- render_seq_tests
```
Expected: PASS

**Step 6: Add integration with ClientSyncState**

Add to `zellij-remote-core/src/sync.rs`:
```rust
use crate::render_seq::RenderSeqTracker;

// Add field to ClientSyncState:
pub struct ClientSyncState {
    frame: FrameData,
    style_table: StyleTable,
    state_id: u64,
    render_tracker: RenderSeqTracker,  // NEW
}

// Update new():
impl ClientSyncState {
    pub fn new(cols: usize, rows: usize) -> Self {
        Self {
            frame: FrameData::new(cols, rows),
            style_table: StyleTable::new(),
            state_id: 0,
            render_tracker: RenderSeqTracker::new(),
        }
    }

    // Add method for datagram render:
    pub fn should_apply_datagram(&self, baseline_id: u64, render_seq: u64) -> bool {
        self.render_tracker.should_apply(baseline_id, render_seq)
    }

    // Update apply_snapshot to reset tracker:
    pub fn apply_snapshot(&mut self, snapshot: &ScreenSnapshot) {
        // ... existing code ...
        self.render_tracker.reset_for_snapshot(snapshot.state_id);
    }
}
```

**Step 7: Add bridge-side send routing**

This will be used in Task 3.x when we implement the full bridge, but define the pattern now.

Add to `zellij-remote-bridge/src/lib.rs` (or create `render_router.rs`):
```rust
use zellij_remote_protocol::{
    ScreenDelta, DatagramEnvelope, StreamEnvelope,
    datagram_envelope, stream_envelope,
};
use zellij_remote_core::{RenderSender, DatagramDecision};

/// Routes render updates to datagram or stream based on size and client capabilities
pub fn route_render_update(
    delta: &ScreenDelta,
    render_seq: u64,
    baseline_id: u64,
    client_supports_datagrams: bool,
    max_datagram_bytes: u32,
) -> RenderRoute {
    // Encode to check size
    use prost::Message;
    let envelope = DatagramEnvelope {
        msg: Some(datagram_envelope::Msg::ScreenDelta(delta.clone())),
    };
    let encoded_len = envelope.encoded_len();

    if client_supports_datagrams && encoded_len <= max_datagram_bytes as usize {
        RenderRoute::Datagram(envelope)
    } else {
        RenderRoute::Stream(StreamEnvelope {
            msg: Some(stream_envelope::Msg::ScreenDeltaStream(delta.clone())),
        })
    }
}

pub enum RenderRoute {
    Datagram(DatagramEnvelope),
    Stream(StreamEnvelope),
}
```

**Step 8: Run all tests**

```bash
cargo test -p zellij-remote-core
cargo test -p zellij-remote-bridge
```
Expected: All tests pass

**Step 9: Commit**

```bash
git add zellij-remote-core/src/ zellij-remote-bridge/src/
git commit -m "feat(remote): add datagram render with latest-wins semantics for low-latency updates"
```

---

## Phase 3-7: Remaining Implementation (Summary)

The remaining phases follow the same TDD pattern:

### Phase 3: Render Backpressure & Flow Control
- Task 3.1: Per-client render window tracking
- Task 3.2: Forced snapshot on window exhaustion
- Task 3.3: Client StateAck handling

### Phase 4: Controller Lease
- Task 4.1: Lease state machine (grant/deny/keepalive/timeout/revoke)
- Task 4.2: Resize integration with Zellij IPC
- Task 4.3: Multi-client viewer mode

### Phase 5: Input Handling (Reliability-First)
- Task 5.1: Reliable stream input with InputAck
- Task 5.2: RTT estimation
- Task 5.3: Input-to-Zellij IPC forwarding

### Phase 6: Prediction Engine (After Correctness Proven)
- Task 6.1: Prediction overlay model
- Task 6.2: Reconciliation using delivered_input_watermark
- Task 6.3: Confidence levels and glitch handling

### Phase 7: Client Library & Bindings
- Task 7.1: zellij-remote-client crate
- Task 7.2: UniFFI wrapper for mobile
- Task 7.3: WASM transport for web

---

## Test Commands Reference

```bash
# Build all remote crates
cargo build -p zellij-remote-protocol -p zellij-remote-core -p zellij-remote-bridge

# Test specific crate
cargo test -p zellij-remote-protocol
cargo test -p zellij-remote-core
cargo test -p zellij-remote-bridge

# Run specific test
cargo test -p zellij-remote-core -- sync_tests::test_apply_delta_requires_matching_base

# Run spike
cargo run -p zellij-remote-bridge --example spike_server
cargo run -p zellij-remote-bridge --example spike_client

# Full verification
cargo xtask build --no-plugins
cargo xtask clippy
cargo xtask format --check
```

---

## Key Differences from v1 Plan

| Issue | v1 Plan | v2 Plan |
|-------|---------|---------|
| Codegen path | xtask + manual path | build.rs + OUT_DIR (correct) |
| Task order | Protocol → Screen → Bridge | Protocol → Spike → Correctness |
| Input transport | Datagrams with reliability | Reliable stream (MVP) |
| Render transport | Not specified | **Datagrams (latest-wins) + stream fallback** |
| Screen model | Flat ScreenModel | Arc<Row> FrameStore |
| Style lookup | O(n) linear scan | O(1) reverse HashMap |
| Row comparison | Per-cell hash | Arc pointer equality |
| Prediction | Phase 4 | Phase 6 (after correctness) |
| Backpressure | Not explicit | Render window + coalescing |

---

## Summary

This plan implements ZRP in **7 phases**, starting with a plumbing spike to de-risk integration:

1. **Phase 0:** Crate setup with correct codegen
2. **Phase 1:** WebTransport spike proving end-to-end connectivity
3. **Phase 2:** Frame store + delta engine with Arc sharing
4. **Phase 3:** Backpressure and flow control
5. **Phase 4:** Controller lease for resize
6. **Phase 5:** Reliable input handling
7. **Phase 6:** Prediction (deferred until correctness proven)
