# ZRP Zellij Integration Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Wire the existing ZRP infrastructure into Zellij's actual render and input pipelines, enabling remote clients to connect to real Zellij sessions.

**Key Principle:** Minimal intrusion. All changes behind `#[cfg(feature = "remote")]` to keep the integration surface small for long-term fork maintenance.

---

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              Zellij Server                                  │
│                                                                             │
│  ┌──────────┐     ┌──────────┐     ┌──────────┐     ┌──────────────────┐   │
│  │   PTY    │────▶│  Screen  │────▶│  Output  │────▶│ ServerInstruction│   │
│  │ (Grid)   │     │ (Tabs)   │     │(Serialize)│    │ ::Render(ANSI)   │   │
│  └──────────┘     └────┬─────┘     └──────────┘     └────────┬─────────┘   │
│                        │                                      │             │
│                        │ [NEW: Remote Tap]                    │             │
│                        ▼                                      ▼             │
│               ┌────────────────┐                     ┌─────────────────┐    │
│               │ RemoteManager  │                     │  IPC Clients    │    │
│               │ (RemoteSession)│                     │  (Unix Socket)  │    │
│               └────────┬───────┘                     └─────────────────┘    │
│                        │                                                    │
│                        ▼                                                    │
│               ┌────────────────┐                                            │
│               │ WebTransport   │◀───────────────── Remote Clients           │
│               │   Server       │                   (Mobile/Web)             │
│               └────────────────┘                                            │
└─────────────────────────────────────────────────────────────────────────────┘
```

The remote infrastructure runs as an **additional transport**, not a replacement. It observes renders and injects inputs through the existing thread bus.

---

## Phase 7.1: Thread Bus Integration (Feature-Gated)

### Task 7.1.1: Add RemoteInstruction Enum

**Files:**
- Create: `zellij-server/src/remote/mod.rs`
- Create: `zellij-server/src/remote/instruction.rs`

**Step 1: Create remote module directory**

```bash
mkdir -p zellij-server/src/remote
```

**Step 2: Create instruction types**

Create `zellij-server/src/remote/instruction.rs`:
```rust
use crate::ClientId;
use zellij_remote_core::FrameStore;
use zellij_utils::pane_size::Size;

/// Instructions sent TO the remote thread
#[derive(Debug)]
pub enum RemoteInstruction {
    /// A client's frame is ready to be sent
    FrameReady {
        client_id: ClientId,
        frame_store: FrameStore,
    },
    /// Client resized their viewport
    ClientResize {
        client_id: ClientId,
        size: Size,
    },
    /// Session is shutting down
    Shutdown,
}

/// Instructions sent FROM the remote thread to inject input
#[derive(Debug)]
pub enum RemoteInputInstruction {
    /// Remote client sent keyboard input
    Key {
        client_id: ClientId,
        key: Vec<u8>,
    },
    /// Remote client sent mouse event
    Mouse {
        client_id: ClientId,
        event: zellij_utils::input::mouse::MouseEvent,
    },
    /// Remote client connected
    ClientConnected {
        client_id: ClientId,
        size: Size,
    },
    /// Remote client disconnected
    ClientDisconnected {
        client_id: ClientId,
    },
}
```

**Step 3: Create module facade**

Create `zellij-server/src/remote/mod.rs`:
```rust
mod instruction;

pub use instruction::{RemoteInputInstruction, RemoteInstruction};
```

**Step 4: Add to lib.rs**

Modify `zellij-server/src/lib.rs`, add after line 7:
```rust
#[cfg(feature = "remote")]
pub mod remote;
```

**Verification:**
```bash
cargo build -p zellij-server --features remote
```

**Step 5: Commit**
```bash
git add zellij-server/src/remote/
git commit -m "feat(remote): add RemoteInstruction types for thread bus integration"
```

---

### Task 7.1.2: Extend ThreadSenders with Remote Channel

**Files:**
- Modify: `zellij-server/src/thread_bus.rs`

**Step 1: Add feature-gated sender field**

Add import at top of `thread_bus.rs`:
```rust
#[cfg(feature = "remote")]
use crate::remote::RemoteInstruction;
```

Add field to `ThreadSenders` struct (after line 19):
```rust
    #[cfg(feature = "remote")]
    pub to_remote: Option<SenderWithContext<RemoteInstruction>>,
```

**Step 2: Add send method**

Add method to `ThreadSenders` impl (after `send_to_background_jobs`):
```rust
    #[cfg(feature = "remote")]
    pub fn send_to_remote(&self, instruction: RemoteInstruction) -> Result<()> {
        if self.should_silently_fail {
            let _ = self
                .to_remote
                .as_ref()
                .map(|sender| sender.send(instruction))
                .unwrap_or_else(|| Ok(()));
            Ok(())
        } else {
            self.to_remote
                .as_ref()
                .context("failed to get remote sender")?
                .send(instruction)
                .to_anyhow()
                .context("failed to send message to remote thread")
        }
    }
```

**Step 3: Update Default impl**

The `#[derive(Default)]` handles this automatically since `Option<T>` defaults to `None`.

**Verification:**
```bash
cargo build -p zellij-server --features remote
cargo build -p zellij-server  # Without feature - should still compile
```

**Step 4: Commit**
```bash
git add zellij-server/src/thread_bus.rs
git commit -m "feat(remote): add feature-gated remote sender to ThreadSenders"
```

---

## Phase 7.2: Remote Thread Implementation

### Task 7.2.1: Create RemoteManager

**Files:**
- Create: `zellij-server/src/remote/manager.rs`

**Step 1: Create manager**

Create `zellij-server/src/remote/manager.rs`:
```rust
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::ClientId;
use zellij_remote_core::{
    FrameStore, InputError, LeaseResult, RemoteSession, RenderUpdate, StyleTable,
};
use zellij_remote_protocol::InputEvent;
use zellij_utils::pane_size::Size;

/// Manages remote client connections and state
pub struct RemoteManager {
    session: Arc<RwLock<RemoteSession>>,
    style_table: Arc<RwLock<StyleTable>>,
    /// Maps Zellij ClientId to remote internal client ID
    client_mapping: HashMap<ClientId, u64>,
    next_remote_id: u64,
}

impl RemoteManager {
    pub fn new(initial_size: Size) -> Self {
        Self {
            session: Arc::new(RwLock::new(RemoteSession::new(
                initial_size.cols,
                initial_size.rows,
            ))),
            style_table: Arc::new(RwLock::new(StyleTable::new())),
            client_mapping: HashMap::new(),
            next_remote_id: 1,
        }
    }

    pub fn session(&self) -> Arc<RwLock<RemoteSession>> {
        self.session.clone()
    }

    pub fn style_table(&self) -> Arc<RwLock<StyleTable>> {
        self.style_table.clone()
    }

    /// Register a new remote client, returns the remote client ID
    pub async fn add_client(&mut self, zellij_id: ClientId, window_size: u32) -> u64 {
        let remote_id = self.next_remote_id;
        self.next_remote_id += 1;
        self.client_mapping.insert(zellij_id, remote_id);

        let mut session = self.session.write().await;
        session.add_client(remote_id, window_size);

        remote_id
    }

    /// Remove a remote client
    pub async fn remove_client(&mut self, zellij_id: ClientId) {
        if let Some(remote_id) = self.client_mapping.remove(&zellij_id) {
            let mut session = self.session.write().await;
            session.remove_client(remote_id);
        }
    }

    /// Get remote ID for a Zellij client
    pub fn get_remote_id(&self, zellij_id: ClientId) -> Option<u64> {
        self.client_mapping.get(&zellij_id).copied()
    }

    /// Check if a Zellij client is remote
    pub fn is_remote_client(&self, zellij_id: ClientId) -> bool {
        self.client_mapping.contains_key(&zellij_id)
    }

    /// Update frame store from Grid data
    pub async fn update_frame(&self, frame_store: FrameStore) {
        let mut session = self.session.write().await;
        // Copy frame data into session's frame store
        // The frame_store already has the converted Grid data
        session.frame_store = frame_store;
        session.frame_store.advance_state();
        session.record_state_snapshot();
    }

    /// Get render update for a specific client
    pub async fn get_render_update(&self, zellij_id: ClientId) -> Option<RenderUpdate> {
        let remote_id = self.get_remote_id(zellij_id)?;
        let mut session = self.session.write().await;
        session.get_render_update(remote_id)
    }
}
```

**Step 2: Add to module**

Update `zellij-server/src/remote/mod.rs`:
```rust
mod instruction;
mod manager;

pub use instruction::{RemoteInputInstruction, RemoteInstruction};
pub use manager::RemoteManager;
```

**Verification:**
```bash
cargo build -p zellij-server --features remote
```

**Step 3: Commit**
```bash
git add zellij-server/src/remote/
git commit -m "feat(remote): add RemoteManager for client state management"
```

---

### Task 7.2.2: Create Remote Thread Main Loop

**Files:**
- Create: `zellij-server/src/remote/thread.rs`

**Step 1: Create thread implementation**

Create `zellij-server/src/remote/thread.rs`:
```rust
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::RwLock;
use zellij_utils::channels::ChannelWithContext;
use zellij_utils::errors::prelude::*;
use zellij_utils::pane_size::Size;

use super::instruction::RemoteInstruction;
use super::manager::RemoteManager;

/// Configuration for the remote server
pub struct RemoteConfig {
    pub listen_addr: SocketAddr,
    pub session_name: String,
    pub initial_size: Size,
}

impl Default for RemoteConfig {
    fn default() -> Self {
        Self {
            listen_addr: "127.0.0.1:4433".parse().unwrap(),
            session_name: "zellij".to_string(),
            initial_size: Size { cols: 80, rows: 24 },
        }
    }
}

/// Main entry point for the remote thread
pub fn remote_thread_main(
    receiver: ChannelWithContext<RemoteInstruction>,
    config: RemoteConfig,
) -> Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to create tokio runtime for remote thread")?;

    rt.block_on(async {
        let manager = Arc::new(RwLock::new(RemoteManager::new(config.initial_size)));

        // Spawn the instruction handler
        let manager_handle = manager.clone();
        let instruction_handler = tokio::spawn(async move {
            handle_instructions(receiver, manager_handle).await
        });

        // TODO: Spawn WebTransport server here
        // For now, just wait for shutdown
        log::info!(
            "Remote thread started (WebTransport server would listen on {})",
            config.listen_addr
        );

        let _ = instruction_handler.await;
        log::info!("Remote thread shutting down");
    });

    Ok(())
}

async fn handle_instructions(
    receiver: ChannelWithContext<RemoteInstruction>,
    manager: Arc<RwLock<RemoteManager>>,
) {
    loop {
        match receiver.recv() {
            Ok((instruction, _err_ctx)) => match instruction {
                RemoteInstruction::FrameReady {
                    client_id,
                    frame_store,
                } => {
                    let mgr = manager.read().await;
                    if mgr.is_remote_client(client_id) {
                        mgr.update_frame(frame_store).await;
                        // TODO: Send delta/snapshot to client
                    }
                },
                RemoteInstruction::ClientResize { client_id, size } => {
                    log::debug!("Remote client {} resized to {:?}", client_id, size);
                    // TODO: Handle resize
                },
                RemoteInstruction::Shutdown => {
                    log::info!("Remote thread received shutdown signal");
                    break;
                },
            },
            Err(e) => {
                log::error!("Remote instruction channel error: {}", e);
                break;
            },
        }
    }
}
```

**Step 2: Update module exports**

Update `zellij-server/src/remote/mod.rs`:
```rust
mod instruction;
mod manager;
mod thread;

pub use instruction::{RemoteInputInstruction, RemoteInstruction};
pub use manager::RemoteManager;
pub use thread::{remote_thread_main, RemoteConfig};
```

**Verification:**
```bash
cargo build -p zellij-server --features remote
```

**Step 3: Commit**
```bash
git add zellij-server/src/remote/
git commit -m "feat(remote): add remote thread main loop with instruction handling"
```

---

## Phase 7.3: Screen Integration (Render Tap)

### Task 7.3.1: Add Remote Render Hook in Screen

**Files:**
- Modify: `zellij-server/src/screen.rs`

This is the key integration point. We add a feature-gated call after Grid rendering but before ANSI serialization.

**Step 1: Add feature-gated import**

Add near the top of `screen.rs` (after other imports):
```rust
#[cfg(feature = "remote")]
use crate::remote::RemoteInstruction;
#[cfg(feature = "remote")]
use crate::remote_bridge::grid_to_frame_store;
```

**Step 2: Add helper method to Screen**

Add this method to the Screen impl (in a feature-gated block):
```rust
    #[cfg(feature = "remote")]
    fn send_to_remote_clients(&self, tab: &Tab) -> Result<()> {
        // Only proceed if remote sender is configured
        let remote_sender = match &self.bus.senders.to_remote {
            Some(sender) => sender,
            None => return Ok(()),
        };

        // For each connected client that could be remote, convert Grid to FrameStore
        // Initially we send the same frame to all remote clients
        // TODO: Optimize to only convert once and send to all
        let connected_clients: Vec<crate::ClientId> = self
            .connected_clients
            .borrow()
            .keys()
            .copied()
            .collect();

        for client_id in connected_clients {
            // Get the active pane's Grid for this client
            if let Some(active_pane) = tab.get_active_pane(client_id) {
                if let Some(grid) = active_pane.grid() {
                    let mut style_table = zellij_remote_core::StyleTable::new();
                    let frame_store = grid_to_frame_store(grid, &mut style_table);

                    let _ = remote_sender.send(RemoteInstruction::FrameReady {
                        client_id,
                        frame_store,
                    });
                }
            }
        }

        Ok(())
    }
```

**Step 3: Call the hook in render_to_clients**

In `render_to_clients()`, add after the tab render loop (around line 1517, after `tab.render(&mut output, None)`):
```rust
                    #[cfg(feature = "remote")]
                    {
                        let _ = self.send_to_remote_clients(tab);
                    }
```

**Note:** This is a minimal integration. The full implementation would:
1. Only send to clients marked as remote
2. Use a shared style table across clients
3. Handle multiple tabs/panes properly

**Verification:**
```bash
cargo build -p zellij-server --features remote
cargo build -p zellij-server  # Without feature
```

**Step 4: Commit**
```bash
git add zellij-server/src/screen.rs
git commit -m "feat(remote): add feature-gated render hook in Screen"
```

---

### Task 7.3.2: Add Grid Access to Panes

**Files:**
- Modify: `zellij-server/src/panes/terminal_pane.rs`

The `grid()` method may not exist on the Pane trait. We need to add it.

**Step 1: Check if grid() exists**

```bash
grep -n "fn grid" zellij-server/src/panes/terminal_pane.rs
```

If not, add to the pane trait or provide a concrete accessor:

Add to `TerminalPane` impl:
```rust
    #[cfg(feature = "remote")]
    pub fn grid(&self) -> Option<&Grid> {
        Some(&self.grid)
    }
```

**Step 2: Add trait method if needed**

If there's a Pane trait that needs extending, add:
```rust
    #[cfg(feature = "remote")]
    fn grid(&self) -> Option<&Grid> {
        None // Default implementation
    }
```

**Verification:**
```bash
cargo build -p zellij-server --features remote
```

**Step 3: Commit**
```bash
git add zellij-server/src/panes/
git commit -m "feat(remote): add grid accessor for remote rendering"
```

---

## Phase 7.4: Server Startup Integration

### Task 7.4.1: Spawn Remote Thread in Session Init

**Files:**
- Modify: `zellij-server/src/lib.rs`

**Step 1: Add feature-gated thread spawn**

In `start_server()`, after the session initialization (around where other threads are spawned), add:

```rust
    #[cfg(feature = "remote")]
    let remote_thread = {
        use crate::remote::{remote_thread_main, RemoteConfig};

        let (to_remote, remote_receiver) = channels::bounded(50);
        let to_remote = SenderWithContext::new(to_remote);

        // Add remote sender to thread senders
        // This requires modifying the senders initialization

        let config = RemoteConfig {
            listen_addr: std::env::var("ZELLIJ_REMOTE_ADDR")
                .unwrap_or_else(|_| "127.0.0.1:4433".to_string())
                .parse()
                .unwrap_or_else(|_| "127.0.0.1:4433".parse().unwrap()),
            session_name: session_name.clone(),
            initial_size: Size { cols: 80, rows: 24 },
        };

        std::thread::Builder::new()
            .name("remote".to_string())
            .spawn(move || remote_thread_main(remote_receiver, config).fatal())
            .expect("failed to spawn remote thread")
    };
```

**Note:** The actual integration requires threading the `to_remote` sender through to the `ThreadSenders` struct, which happens during session initialization. This task provides the skeleton.

**Verification:**
```bash
cargo build -p zellij-server --features remote
```

**Step 2: Commit**
```bash
git add zellij-server/src/lib.rs
git commit -m "feat(remote): spawn remote thread during session initialization"
```

---

## Phase 7.5: Input Routing

### Task 7.5.1: Create Input Translator

**Files:**
- Create: `zellij-server/src/remote/input.rs`

This module translates ZRP InputEvent to Zellij's Action type.

**Step 1: Create translator**

Create `zellij-server/src/remote/input.rs`:
```rust
use zellij_remote_protocol::{input_event, key_event, InputEvent, KeyModifiers, SpecialKey};
use zellij_utils::data::{BareKey, KeyWithModifier};
use zellij_utils::input::actions::Action;

/// Translate ZRP InputEvent to Zellij Action
pub fn translate_input(event: &InputEvent) -> Option<Action> {
    match &event.payload {
        Some(input_event::Payload::TextUtf8(bytes)) => {
            if let Ok(text) = std::str::from_utf8(bytes) {
                // Convert text to Write action
                let bytes = bytes.clone();
                Some(Action::Write {
                    key: None,
                    bytes: Some(bytes),
                    is_kitty_keyboard_protocol: false,
                })
            } else {
                None
            }
        },
        Some(input_event::Payload::Key(key_event)) => {
            translate_key_event(key_event)
        },
        Some(input_event::Payload::RawBytes(bytes)) => {
            Some(Action::Write {
                key: None,
                bytes: Some(bytes.clone()),
                is_kitty_keyboard_protocol: false,
            })
        },
        Some(input_event::Payload::Mouse(mouse_event)) => {
            // TODO: Translate mouse events
            None
        },
        None => None,
    }
}

fn translate_key_event(key: &zellij_remote_protocol::KeyEvent) -> Option<Action> {
    let key_with_modifier = match &key.key {
        Some(key_event::Key::UnicodeScalar(codepoint)) => {
            let ch = char::from_u32(*codepoint)?;
            let bare_key = BareKey::Char(ch);
            let modifiers = translate_modifiers(key.modifiers.as_ref());
            KeyWithModifier {
                bare_key,
                key_modifiers: modifiers,
            }
        },
        Some(key_event::Key::Special(special)) => {
            let bare_key = translate_special_key(*special)?;
            let modifiers = translate_modifiers(key.modifiers.as_ref());
            KeyWithModifier {
                bare_key,
                key_modifiers: modifiers,
            }
        },
        None => return None,
    };

    // Convert to bytes for Write action
    let bytes = key_to_bytes(&key_with_modifier);
    
    Some(Action::Write {
        key: Some(key_with_modifier),
        bytes: Some(bytes),
        is_kitty_keyboard_protocol: false,
    })
}

fn translate_modifiers(mods: Option<&KeyModifiers>) -> Vec<zellij_utils::data::KeyModifier> {
    let mut result = Vec::new();
    if let Some(mods) = mods {
        let bits = mods.bits;
        if bits & 1 != 0 {
            result.push(zellij_utils::data::KeyModifier::Shift);
        }
        if bits & 2 != 0 {
            result.push(zellij_utils::data::KeyModifier::Alt);
        }
        if bits & 4 != 0 {
            result.push(zellij_utils::data::KeyModifier::Ctrl);
        }
        if bits & 8 != 0 {
            result.push(zellij_utils::data::KeyModifier::Super);
        }
    }
    result
}

fn translate_special_key(special: i32) -> Option<BareKey> {
    match SpecialKey::try_from(special).ok()? {
        SpecialKey::Enter => Some(BareKey::Enter),
        SpecialKey::Escape => Some(BareKey::Esc),
        SpecialKey::Backspace => Some(BareKey::Backspace),
        SpecialKey::Tab => Some(BareKey::Tab),
        SpecialKey::Left => Some(BareKey::Left),
        SpecialKey::Right => Some(BareKey::Right),
        SpecialKey::Up => Some(BareKey::Up),
        SpecialKey::Down => Some(BareKey::Down),
        SpecialKey::Home => Some(BareKey::Home),
        SpecialKey::End => Some(BareKey::End),
        SpecialKey::PageUp => Some(BareKey::PageUp),
        SpecialKey::PageDown => Some(BareKey::PageDown),
        SpecialKey::Insert => Some(BareKey::Insert),
        SpecialKey::Delete => Some(BareKey::Delete),
        SpecialKey::F1 => Some(BareKey::F(1)),
        SpecialKey::F2 => Some(BareKey::F(2)),
        SpecialKey::F3 => Some(BareKey::F(3)),
        SpecialKey::F4 => Some(BareKey::F(4)),
        SpecialKey::F5 => Some(BareKey::F(5)),
        SpecialKey::F6 => Some(BareKey::F(6)),
        SpecialKey::F7 => Some(BareKey::F(7)),
        SpecialKey::F8 => Some(BareKey::F(8)),
        SpecialKey::F9 => Some(BareKey::F(9)),
        SpecialKey::F10 => Some(BareKey::F(10)),
        SpecialKey::F11 => Some(BareKey::F(11)),
        SpecialKey::F12 => Some(BareKey::F(12)),
        _ => None,
    }
}

fn key_to_bytes(key: &KeyWithModifier) -> Vec<u8> {
    // Simple conversion - full implementation would handle all key types
    match &key.bare_key {
        BareKey::Char(c) => {
            let mut s = String::new();
            s.push(*c);
            s.into_bytes()
        },
        BareKey::Enter => vec![b'\r'],
        BareKey::Tab => vec![b'\t'],
        BareKey::Backspace => vec![0x7f],
        BareKey::Esc => vec![0x1b],
        BareKey::Left => b"\x1b[D".to_vec(),
        BareKey::Right => b"\x1b[C".to_vec(),
        BareKey::Up => b"\x1b[A".to_vec(),
        BareKey::Down => b"\x1b[B".to_vec(),
        _ => vec![],
    }
}
```

**Step 2: Update module**

Update `zellij-server/src/remote/mod.rs`:
```rust
mod input;
mod instruction;
mod manager;
mod thread;

pub use input::translate_input;
pub use instruction::{RemoteInputInstruction, RemoteInstruction};
pub use manager::RemoteManager;
pub use thread::{remote_thread_main, RemoteConfig};
```

**Verification:**
```bash
cargo build -p zellij-server --features remote
```

**Step 3: Commit**
```bash
git add zellij-server/src/remote/
git commit -m "feat(remote): add input translator from ZRP to Zellij Actions"
```

---

## Integration Point Summary

After all tasks, the integration touches these files:

| File | Change Type | Lines Changed (est.) |
|------|-------------|---------------------|
| `zellij-server/src/lib.rs` | Add module + spawn thread | ~15 |
| `zellij-server/src/thread_bus.rs` | Add sender field + method | ~20 |
| `zellij-server/src/screen.rs` | Add render hook | ~30 |
| `zellij-server/src/panes/terminal_pane.rs` | Add grid accessor | ~5 |
| `zellij-server/src/remote/` | NEW module | ~400 |

All core changes are behind `#[cfg(feature = "remote")]`, keeping the diff minimal for rebasing.

---

## Testing Strategy

### Unit Tests
- Input translation: `cargo test -p zellij-server --features remote -- remote::input`
- Manager state: `cargo test -p zellij-server --features remote -- remote::manager`

### Integration Tests
1. Start Zellij with remote feature: `cargo run --features remote -- --session test`
2. Set `ZELLIJ_REMOTE_ADDR=0.0.0.0:4433`
3. Connect with spike_client: `cargo run --example spike_client -p zellij-remote-bridge`
4. Verify handshake completes and frames are received

### Cross-Machine Testing
Same as Phase 6 testing over Tailscale.

---

## Future Work

After Phase 7 is complete:

1. **Full Grid capture**: Currently captures active pane only; expand to full tab composition
2. **Client type differentiation**: Skip ANSI serialization for remote-only clients
3. **Bidirectional input**: Route translated Actions through existing keybind system
4. **CLI integration**: Add `zellij remote-serve` subcommand
5. **Authentication**: Token-based auth during handshake
