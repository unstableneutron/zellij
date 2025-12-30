use anyhow::Result;
use bytes::BytesMut;
use prost::Message;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use zellij_remote_bridge::encode_envelope;
use zellij_remote_core::{Cell, DeltaEngine, Frame, FrameStore, StyleTable};
use zellij_remote_protocol::{
    stream_envelope, Capabilities, ClientHello, ControllerLease, ControllerPolicy,
    ProtocolVersion, ServerHello, SessionState, StreamEnvelope,
};

const SCREEN_COLS: usize = 80;
const SCREEN_ROWS: usize = 24;

static CLIENT_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Shared session state that persists across client connections
struct SharedSession {
    store: FrameStore,
    style_table: StyleTable,
    connection_count: u64,
}

impl SharedSession {
    fn new() -> Self {
        let mut store = FrameStore::new(SCREEN_COLS, SCREEN_ROWS);
        draw_welcome_screen(&mut store);
        store.advance_state();

        Self {
            store,
            style_table: StyleTable::new(),
            connection_count: 0,
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();

    let listen_addr: std::net::SocketAddr = std::env::var("LISTEN_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:4433".to_string())
        .parse()
        .expect("Invalid LISTEN_ADDR");

    println!("Starting spike server on {}", listen_addr);

    let identity = wtransport::Identity::self_signed(["localhost", "spike-server"])
        .expect("Failed to create identity");

    let config = wtransport::ServerConfig::builder()
        .with_bind_default(listen_addr.port())
        .with_identity(identity)
        .build();

    let server = wtransport::Endpoint::server(config)?;

    // Shared session state - persists across connections
    let session = Arc::new(RwLock::new(SharedSession::new()));

    // Background task to update screen state
    let session_updater = session.clone();
    tokio::spawn(async move {
        let mut counter = 0u32;
        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let mut session = session_updater.write().await;
            update_animation(&mut session.store, counter);
            session.store.advance_state();
            counter += 1;
        }
    });

    log::info!("WebTransport server listening on {}", listen_addr);

    loop {
        let incoming = server.accept().await;
        let session_request = incoming.await?;

        log::info!(
            "Incoming connection from {}",
            session_request.authority()
        );

        let connection = session_request.accept().await?;
        let session = session.clone();

        tokio::spawn(async move {
            if let Err(e) = handle_connection(connection, session).await {
                log::error!("Connection error: {}", e);
            }
        });
    }
}

async fn handle_connection(
    connection: wtransport::Connection,
    session: Arc<RwLock<SharedSession>>,
) -> Result<()> {
    let (mut send, mut recv) = connection.accept_bi().await?;

    let client_id = CLIENT_ID_COUNTER.fetch_add(1, Ordering::Relaxed);

    // Track connection
    {
        let mut s = session.write().await;
        s.connection_count += 1;
        log::info!(
            "Client {} connected (total connections: {})",
            client_id,
            s.connection_count
        );
    }

    // === HANDSHAKE ===
    let client_hello = read_client_hello(&mut recv).await?;
    log::info!(
        "Received ClientHello from {} (client_id={})",
        client_hello.client_name,
        client_id
    );

    // Check if client wants to resume
    let resume_state_id = if !client_hello.resume_token.is_empty() {
        // Parse resume token as state_id (simple implementation)
        let bytes: [u8; 8] = client_hello
            .resume_token
            .get(..8)
            .and_then(|s| s.try_into().ok())
            .unwrap_or([0; 8]);
        let state_id = u64::from_le_bytes(bytes);
        log::info!("Client requesting resume from state_id={}", state_id);
        Some(state_id)
    } else {
        None
    };

    let server_hello = build_server_hello(&client_hello, client_id);
    let encoded = encode_envelope(&StreamEnvelope {
        msg: Some(stream_envelope::Msg::ServerHello(server_hello)),
    })?;
    send.write_all(&encoded).await?;
    log::info!("Sent ServerHello to client {}", client_id);

    // === SEND INITIAL STATE ===
    let (current_state_id, snapshot_or_delta) = {
        let session = session.read().await;
        let current_state_id = session.store.current_state_id();

        if let Some(resume_id) = resume_state_id {
            if resume_id < current_state_id {
                // Client has old state, send delta
                // Note: In real impl, we'd need to keep history. For demo, just send snapshot.
                log::info!(
                    "Client resume_id={} < current={}, sending snapshot",
                    resume_id,
                    current_state_id
                );
            }
        }

        // Always send snapshot for now (delta resume requires state history)
        let snapshot = DeltaEngine::compute_snapshot(
            session.store.current_frame(),
            &mut session.style_table.clone(),
            current_state_id,
        );
        (current_state_id, snapshot)
    };

    let encoded = encode_envelope(&StreamEnvelope {
        msg: Some(stream_envelope::Msg::ScreenSnapshot(snapshot_or_delta)),
    })?;
    send.write_all(&encoded).await?;
    log::info!(
        "Sent ScreenSnapshot (state_id={}) to client {}",
        current_state_id,
        client_id
    );

    // === STREAM DELTAS ===
    let mut last_sent_state_id = current_state_id;
    let mut last_snapshot: Option<Frame> = None;

    loop {
        tokio::time::sleep(Duration::from_millis(100)).await;

        let (current_state_id, maybe_delta) = {
            let session = session.read().await;
            let current_id = session.store.current_state_id();

            if current_id > last_sent_state_id {
                let current_frame = session.store.snapshot();

                let delta = if let Some(ref baseline) = last_snapshot {
                    DeltaEngine::compute_delta(
                        &baseline.data,
                        &current_frame.data,
                        &mut session.style_table.clone(),
                        last_sent_state_id,
                        current_id,
                    )
                } else {
                    // No baseline, compute from empty
                    DeltaEngine::compute_delta(
                        &FrameStore::new(SCREEN_COLS, SCREEN_ROWS).snapshot().data,
                        &current_frame.data,
                        &mut session.style_table.clone(),
                        last_sent_state_id,
                        current_id,
                    )
                };

                (current_id, Some((delta, current_frame)))
            } else {
                (current_id, None)
            }
        };

        if let Some((delta, frame)) = maybe_delta {
            if !delta.row_patches.is_empty() || delta.cursor.is_some() {
                let encoded = encode_envelope(&StreamEnvelope {
                    msg: Some(stream_envelope::Msg::ScreenDeltaStream(delta)),
                })?;

                if let Err(e) = send.write_all(&encoded).await {
                    log::warn!("Failed to send delta to client {}: {}", client_id, e);
                    break;
                }

                log::debug!(
                    "Sent ScreenDelta (state_id={}) to client {}",
                    current_state_id,
                    client_id
                );
            }

            last_sent_state_id = current_state_id;
            last_snapshot = Some(frame);
        }
    }

    log::info!("Client {} disconnected", client_id);
    Ok(())
}

async fn read_client_hello(recv: &mut wtransport::RecvStream) -> Result<ClientHello> {
    let mut buffer = BytesMut::new();

    loop {
        let mut chunk = [0u8; 1024];
        let n = recv.read(&mut chunk).await?.unwrap_or(0);
        if n == 0 {
            anyhow::bail!("connection closed during handshake");
        }
        buffer.extend_from_slice(&chunk[..n]);

        if let Some(envelope) = decode_envelope(&mut buffer)? {
            match envelope.msg {
                Some(stream_envelope::Msg::ClientHello(hello)) => {
                    return Ok(hello);
                }
                _ => {
                    anyhow::bail!("expected ClientHello, got other message");
                }
            }
        }
    }
}

fn decode_envelope(buf: &mut BytesMut) -> Result<Option<StreamEnvelope>> {
    use bytes::Buf;

    if buf.is_empty() {
        return Ok(None);
    }

    let mut peek = &buf[..];
    let len = match prost::encoding::decode_varint(&mut peek) {
        Ok(len) => len as usize,
        Err(_) => {
            if buf.len() < 10 {
                return Ok(None);
            }
            anyhow::bail!("invalid varint in frame header");
        }
    };

    let varint_len = buf.len() - peek.len();
    let total_len = varint_len + len;

    if buf.len() < total_len {
        return Ok(None);
    }

    buf.advance(varint_len);
    let frame_data = buf.split_to(len);
    let envelope = StreamEnvelope::decode(&frame_data[..])?;
    Ok(Some(envelope))
}

fn build_server_hello(client_hello: &ClientHello, client_id: u64) -> ServerHello {
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

    ServerHello {
        negotiated_version: Some(ProtocolVersion {
            major: zellij_remote_protocol::ZRP_VERSION_MAJOR,
            minor: zellij_remote_protocol::ZRP_VERSION_MINOR,
        }),
        negotiated_capabilities: Some(negotiated_caps),
        client_id,
        session_name: "spike-demo".to_string(),
        session_state: SessionState::Running.into(),
        lease: Some(ControllerLease {
            lease_id: 1,
            owner_client_id: client_id,
            policy: ControllerPolicy::LastWriterWins.into(),
            current_size: None,
            remaining_ms: 30000,
            duration_ms: 30000,
        }),
        resume_token: vec![], // Server could send token for client to use on reconnect
        snapshot_interval_ms: 5000,
        max_inflight_inputs: 256,
        render_window: zellij_remote_protocol::DEFAULT_RENDER_WINDOW,
    }
}

fn draw_welcome_screen(store: &mut FrameStore) {
    let title = "╔══════════════════════════════════════════════════════════════════════════════╗";
    let empty = "║                                                                              ║";
    let bottom = "╚══════════════════════════════════════════════════════════════════════════════╝";

    let lines = [
        title,
        empty,
        "║                     🦎 Zellij Remote Protocol Demo 🦎                       ║",
        empty,
        "║  This server maintains PERSISTENT SESSION STATE across connections.        ║",
        empty,
        "║  • Disconnect and reconnect - you'll see the same counter value           ║",
        "║  • Multiple clients can connect simultaneously                             ║",
        "║  • State continues updating even with no clients                          ║",
        empty,
        "║  Watch the counter below - it persists across reconnections:              ║",
        empty,
        "║                                                                              ║",
        empty,
        "║  Protocol features demonstrated:                                            ║",
        "║    ✓ Session persistence                                                   ║",
        "║    ✓ Multiple client support                                               ║",
        "║    ✓ Cumulative delta updates                                              ║",
        "║    ✓ Graceful disconnect handling                                          ║",
        empty,
        bottom,
    ];

    for (row_idx, line) in lines.iter().enumerate() {
        if row_idx >= SCREEN_ROWS {
            break;
        }
        draw_text(store, row_idx, line);
    }
}

fn draw_text(store: &mut FrameStore, row: usize, text: &str) {
    store.update_row(row, |row_data| {
        for (col, ch) in text.chars().enumerate() {
            if col >= SCREEN_COLS {
                break;
            }
            row_data.set_cell(
                col,
                Cell {
                    codepoint: ch as u32,
                    width: 1,
                    style_id: 0,
                },
            );
        }
    });
}

fn update_animation(store: &mut FrameStore, counter: u32) {
    // Update counter display on row 12
    let counter_text = format!(
        "║                    Counter: {:5}  |  State ID: {:5}                      ║",
        counter,
        store.current_state_id() + 1
    );
    draw_text(store, 12, &counter_text);

    // Animate a spinner on row 22
    let spinners = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    let spinner = spinners[(counter as usize) % spinners.len()];
    let status = format!(
        "  {} Streaming updates... (Ctrl+C to stop)                                    ",
        spinner
    );
    draw_text(store, 22, &status);
}
