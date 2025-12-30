use anyhow::Result;
use bytes::BytesMut;
use prost::Message;
use std::time::Duration;
use zellij_remote_bridge::encode_envelope;
use zellij_remote_core::{Cell, DeltaEngine, FrameStore, StyleTable};
use zellij_remote_protocol::{
    stream_envelope, Capabilities, ClientHello, ControllerLease, ControllerPolicy,
    ProtocolVersion, ServerHello, SessionState, StreamEnvelope,
};

const SCREEN_COLS: usize = 80;
const SCREEN_ROWS: usize = 24;

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

    log::info!("WebTransport server listening on {}", listen_addr);

    loop {
        let incoming = server.accept().await;
        let session_request = incoming.await?;

        log::info!(
            "Incoming connection from {}",
            session_request.authority()
        );

        let connection = session_request.accept().await?;

        tokio::spawn(async move {
            if let Err(e) = handle_connection(connection).await {
                log::error!("Connection error: {}", e);
            }
        });
    }
}

async fn handle_connection(connection: wtransport::Connection) -> Result<()> {
    let (mut send, mut recv) = connection.accept_bi().await?;

    // === HANDSHAKE ===
    let client_hello = read_client_hello(&mut recv).await?;
    log::info!("Received ClientHello from {}", client_hello.client_name);

    let server_hello = build_server_hello(&client_hello);
    let encoded = encode_envelope(&StreamEnvelope {
        msg: Some(stream_envelope::Msg::ServerHello(server_hello)),
    })?;
    send.write_all(&encoded).await?;
    log::info!("Sent ServerHello");

    // === SEND INITIAL SNAPSHOT ===
    let mut store = FrameStore::new(SCREEN_COLS, SCREEN_ROWS);
    let mut style_table = StyleTable::new();

    // Draw initial content
    draw_welcome_screen(&mut store);
    store.advance_state();

    let snapshot = DeltaEngine::compute_snapshot(
        store.current_frame(),
        &mut style_table,
        store.current_state_id(),
    );

    let encoded = encode_envelope(&StreamEnvelope {
        msg: Some(stream_envelope::Msg::ScreenSnapshot(snapshot)),
    })?;
    send.write_all(&encoded).await?;
    log::info!("Sent ScreenSnapshot (state_id={})", store.current_state_id());

    // === SEND PERIODIC DELTAS ===
    let mut counter = 0u32;
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;

        let baseline = store.snapshot();
        
        // Update the screen with animation
        update_animation(&mut store, counter);
        store.advance_state();

        let delta = DeltaEngine::compute_delta(
            &baseline.data,
            store.current_frame(),
            &mut style_table,
            baseline.state_id,
            store.current_state_id(),
        );

        if !delta.row_patches.is_empty() || delta.cursor.is_some() {
            let encoded = encode_envelope(&StreamEnvelope {
                msg: Some(stream_envelope::Msg::ScreenDeltaStream(delta)),
            })?;
            send.write_all(&encoded).await?;
            log::debug!(
                "Sent ScreenDelta (state_id={}, patches={})",
                store.current_state_id(),
                counter
            );
        }

        counter += 1;
        if counter > 100 {
            break;
        }
    }

    log::info!("Demo complete, closing connection");
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

fn build_server_hello(client_hello: &ClientHello) -> ServerHello {
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
        client_id: 1,
        session_name: "spike-demo".to_string(),
        session_state: SessionState::Running.into(),
        lease: Some(ControllerLease {
            lease_id: 1,
            owner_client_id: 1,
            policy: ControllerPolicy::LastWriterWins.into(),
            current_size: None,
            remaining_ms: 30000,
            duration_ms: 30000,
        }),
        resume_token: vec![],
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
        "║  This is a live demo of the ZRP protocol over WebTransport/QUIC.           ║",
        empty,
        "║  The server is sending:                                                     ║",
        "║    • Initial ScreenSnapshot (full screen state)                            ║",
        "║    • Periodic ScreenDelta updates (incremental changes)                    ║",
        empty,
        "║  Watch the counter below update in real-time:                              ║",
        empty,
        "║                                                                              ║",
        empty,
        "║  Protocol features demonstrated:                                            ║",
        "║    ✓ WebTransport/QUIC transport                                           ║",
        "║    ✓ Protobuf message encoding                                             ║",
        "║    ✓ Length-prefixed framing                                               ║",
        "║    ✓ Cumulative delta updates                                              ║",
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
