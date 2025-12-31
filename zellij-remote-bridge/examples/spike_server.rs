use anyhow::Result;
use bytes::BytesMut;
use prost::Message;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use zellij_remote_bridge::{decode_datagram_envelope, encode_envelope};
use zellij_remote_core::{
    Cell, FrameStore, InputError, LeaseResult, RemoteSession, RenderUpdate, ResumeResult,
};
use zellij_remote_protocol::{
    datagram_envelope, input_event, key_event, stream_envelope, Capabilities, ClientHello,
    DenyControl, DisplaySize, GrantControl, InputEvent, ProtocolVersion, ServerHello, SessionState,
    StreamEnvelope,
};

const SCREEN_COLS: usize = 80;
const SCREEN_ROWS: usize = 24;
const DEFAULT_RENDER_WINDOW: u32 = 4;

static CLIENT_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

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

    let session = Arc::new(RwLock::new(RemoteSession::new(SCREEN_COLS, SCREEN_ROWS)));

    {
        let mut s = session.write().await;
        draw_welcome_screen(&mut s.frame_store);
        s.frame_store.advance_state();
        s.record_state_snapshot();
    }

    let session_updater = session.clone();
    tokio::spawn(async move {
        let mut counter = 0u32;
        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let mut session = session_updater.write().await;
            update_animation(&mut session.frame_store, counter);
            session.frame_store.advance_state();
            session.record_state_snapshot();
            counter += 1;
        }
    });

    log::info!("WebTransport server listening on {}", listen_addr);

    loop {
        let incoming = server.accept().await;
        let session_request = incoming.await?;

        log::info!("Incoming connection from {}", session_request.authority());

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
    session: Arc<RwLock<RemoteSession>>,
) -> Result<()> {
    let (mut send, mut recv) = connection.accept_bi().await?;

    let client_hello = read_client_hello(&mut recv).await?;

    let (client_id, resumed) = {
        let mut s = session.write().await;

        if !client_hello.resume_token.is_empty() {
            match s.try_resume(&client_hello.resume_token, DEFAULT_RENDER_WINDOW) {
                ResumeResult::Resumed {
                    client_id,
                    baseline_state_id,
                } => {
                    log::info!(
                        "Client {} resumed from state_id={} (total clients: {})",
                        client_id,
                        baseline_state_id,
                        s.client_count()
                    );
                    (client_id, true)
                },
                reason => {
                    log::info!("Resume token rejected ({:?}), creating new client", reason);
                    let client_id = CLIENT_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
                    s.add_client(client_id, DEFAULT_RENDER_WINDOW);
                    (client_id, false)
                },
            }
        } else {
            let client_id = CLIENT_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
            s.add_client(client_id, DEFAULT_RENDER_WINDOW);
            log::info!(
                "Client {} connected (total clients: {})",
                client_id,
                s.client_count()
            );
            (client_id, false)
        }
    };

    log::info!(
        "Received ClientHello from {} (client_id={}, resumed={})",
        client_hello.client_name,
        client_id,
        resumed
    );

    let (server_hello, resume_token) = {
        let mut s = session.write().await;
        let lease = s.lease_manager.request_control(
            client_id,
            Some(DisplaySize { cols: 80, rows: 24 }),
            false,
        );

        let lease_info = match lease {
            LeaseResult::Granted(l) => Some(l),
            LeaseResult::Denied { .. } => s.lease_manager.get_current_lease(),
        };

        let resume_token = s.generate_resume_token(client_id);
        (
            build_server_hello(&client_hello, client_id, lease_info, resume_token.clone()),
            resume_token,
        )
    };

    let encoded = encode_envelope(&StreamEnvelope {
        msg: Some(stream_envelope::Msg::ServerHello(server_hello)),
    })?;
    send.write_all(&encoded).await?;
    log::info!(
        "Sent ServerHello to client {} (resume_token len={})",
        client_id,
        resume_token.len()
    );

    {
        let mut s = session.write().await;
        if resumed {
            if let Some(RenderUpdate::Delta(delta)) = s.get_render_update(client_id) {
                let encoded = encode_envelope(&StreamEnvelope {
                    msg: Some(stream_envelope::Msg::ScreenDeltaStream(delta)),
                })?;
                send.write_all(&encoded).await?;
                log::info!("Sent resume delta to client {}", client_id);
            }
        } else if let Some(RenderUpdate::Snapshot(snapshot)) = s.get_render_update(client_id) {
            let encoded = encode_envelope(&StreamEnvelope {
                msg: Some(stream_envelope::Msg::ScreenSnapshot(snapshot)),
            })?;
            send.write_all(&encoded).await?;
            log::info!("Sent initial ScreenSnapshot to client {}", client_id);
        }
    }

    let session_for_datagrams = session.clone();
    tokio::spawn(async move {
        loop {
            match connection.receive_datagram().await {
                Ok(datagram) => {
                    if let Ok(envelope) = decode_datagram_envelope(&datagram) {
                        if let Some(datagram_envelope::Msg::StateAck(state_ack)) = envelope.msg {
                            let mut s = session_for_datagrams.write().await;
                            s.process_state_ack(client_id, &state_ack);
                            log::debug!(
                                "Processed StateAck from client {}: last_applied={}",
                                client_id,
                                state_ack.last_applied_state_id
                            );
                        }
                    }
                },
                Err(e) => {
                    log::debug!("Datagram receive ended for client {}: {}", client_id, e);
                    break;
                },
            }
        }
    });

    let mut buffer = BytesMut::new();

    loop {
        tokio::select! {
            read_result = async {
                let mut chunk = [0u8; 4096];
                recv.read(&mut chunk).await.map(|n| (n, chunk))
            } => {
                let (n, chunk) = read_result?;
                let n = n.unwrap_or(0);
                if n == 0 {
                    log::info!("Client {} stream closed", client_id);
                    break;
                }
                buffer.extend_from_slice(&chunk[..n]);

                while let Some(envelope) = decode_envelope(&mut buffer)? {
                    match envelope.msg {
                        Some(stream_envelope::Msg::InputEvent(input)) => {
                            let ack = {
                                let mut s = session.write().await;
                                match s.process_input(client_id, &input) {
                                    Ok(ack) => {
                                        handle_input_effect(&mut s.frame_store, &input);
                                        s.frame_store.advance_state();
                                        Some(ack)
                                    }
                                    Err(InputError::NotController) => {
                                        log::warn!("Client {} sent input but is not controller", client_id);
                                        None
                                    }
                                    Err(InputError::Duplicate) => {
                                        log::debug!("Duplicate input from client {}", client_id);
                                        None
                                    }
                                    Err(e) => {
                                        log::warn!("Input error from client {}: {:?}", client_id, e);
                                        None
                                    }
                                }
                            };

                            if let Some(ack) = ack {
                                let encoded = encode_envelope(&StreamEnvelope {
                                    msg: Some(stream_envelope::Msg::InputAck(ack)),
                                })?;
                                send.write_all(&encoded).await?;
                            }
                        }
                        Some(stream_envelope::Msg::RequestControl(req)) => {
                            let response = {
                                let mut s = session.write().await;
                                let result = s.lease_manager.request_control(
                                    client_id,
                                    req.desired_size,
                                    req.force,
                                );

                                match result {
                                    LeaseResult::Granted(lease) => {
                                        log::info!("Granted control to client {}", client_id);
                                        stream_envelope::Msg::GrantControl(GrantControl {
                                            lease: Some(lease),
                                        })
                                    }
                                    LeaseResult::Denied { reason, current_lease } => {
                                        log::info!("Denied control to client {}: {}", client_id, reason);
                                        stream_envelope::Msg::DenyControl(DenyControl {
                                            reason,
                                            lease: current_lease,
                                        })
                                    }
                                }
                            };

                            let encoded = encode_envelope(&StreamEnvelope {
                                msg: Some(response),
                            })?;
                            send.write_all(&encoded).await?;
                        }
                        _ => {
                            log::debug!("Ignoring unhandled message from client {}", client_id);
                        }
                    }
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(100)) => {
                let update = {
                    let mut s = session.write().await;
                    s.get_render_update(client_id)
                };

                match update {
                    Some(RenderUpdate::Snapshot(snapshot)) => {
                        let encoded = encode_envelope(&StreamEnvelope {
                            msg: Some(stream_envelope::Msg::ScreenSnapshot(snapshot)),
                        })?;
                        if let Err(e) = send.write_all(&encoded).await {
                            log::warn!("Failed to send snapshot to client {}: {}", client_id, e);
                            break;
                        }
                    }
                    Some(RenderUpdate::Delta(delta)) => {
                        if !delta.row_patches.is_empty() || delta.cursor.is_some() {
                            let encoded = encode_envelope(&StreamEnvelope {
                                msg: Some(stream_envelope::Msg::ScreenDeltaStream(delta)),
                            })?;
                            if let Err(e) = send.write_all(&encoded).await {
                                log::warn!("Failed to send delta to client {}: {}", client_id, e);
                                break;
                            }
                        }
                    }
                    None => {}
                }
            }
        }
    }

    {
        let mut s = session.write().await;
        s.remove_client(client_id);
        log::info!(
            "Client {} disconnected (remaining: {})",
            client_id,
            s.client_count()
        );
    }

    Ok(())
}

fn handle_input_effect(store: &mut FrameStore, input: &InputEvent) {
    match &input.payload {
        Some(input_event::Payload::Key(key)) => {
            if let Some(key_event::Key::UnicodeScalar(codepoint)) = &key.key {
                if let Some(ch) = char::from_u32(*codepoint) {
                    echo_char(store, ch);
                }
            }
        },
        Some(input_event::Payload::TextUtf8(text)) => {
            if let Ok(s) = std::str::from_utf8(text) {
                for ch in s.chars() {
                    echo_char(store, ch);
                }
            }
        },
        _ => {},
    }
}

static ECHO_COL: AtomicU64 = AtomicU64::new(2);
const ECHO_ROW: usize = 20;

fn echo_char(store: &mut FrameStore, ch: char) {
    let col = ECHO_COL.fetch_add(1, Ordering::Relaxed) as usize;

    if col >= SCREEN_COLS - 2 {
        ECHO_COL.store(2, Ordering::Relaxed);
        store.update_row(ECHO_ROW, |row_data| {
            for c in 2..SCREEN_COLS - 2 {
                row_data.set_cell(
                    c,
                    Cell {
                        codepoint: ' ' as u32,
                        width: 1,
                        style_id: 0,
                    },
                );
            }
        });
        return;
    }

    store.update_row(ECHO_ROW, |row_data| {
        row_data.set_cell(
            col,
            Cell {
                codepoint: ch as u32,
                width: 1,
                style_id: 0,
            },
        );
    });
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
                },
                _ => {
                    anyhow::bail!("expected ClientHello, got other message");
                },
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
        },
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

fn build_server_hello(
    client_hello: &ClientHello,
    client_id: u64,
    lease: Option<zellij_remote_protocol::ControllerLease>,
    resume_token: Vec<u8>,
) -> ServerHello {
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
        lease,
        resume_token,
        snapshot_interval_ms: 5000,
        max_inflight_inputs: 256,
        render_window: DEFAULT_RENDER_WINDOW,
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
        "║  This server uses RemoteSession with proper lease management.              ║",
        empty,
        "║  • First client gets controller lease automatically                        ║",
        "║  • Input events are processed and echoed to screen                         ║",
        "║  • Watch the counter and type to see characters echoed below               ║",
        empty,
        "║  Watch the counter below - it persists across reconnections:              ║",
        empty,
        "║                                                                              ║",
        empty,
        "║  Protocol features demonstrated:                                            ║",
        "║    ✓ RemoteSession with lease management                                   ║",
        "║    ✓ InputEvent processing with InputAck                                   ║",
        "║    ✓ Controller-only input handling                                        ║",
        "║    ✓ Delta streaming with RenderUpdate                                     ║",
        empty,
        "║  Typed input: >                                                            ║",
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
    let counter_text = format!(
        "║                    Counter: {:5}  |  State ID: {:5}                      ║",
        counter,
        store.current_state_id() + 1
    );
    draw_text(store, 12, &counter_text);

    let spinners = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    let spinner = spinners[(counter as usize) % spinners.len()];
    let status = format!(
        "  {} Streaming updates... (Ctrl+C to stop)                                    ",
        spinner
    );
    draw_text(store, 22, &status);
}
