use anyhow::{Context, Result};
use bytes::{Buf, BytesMut};
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{Event, KeyCode, KeyEvent as CtKeyEvent, KeyModifiers as CtKeyModifiers},
    execute,
    style::Print,
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};
use prost::Message;
use std::io::{stdout, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use wtransport::{ClientConfig, Endpoint};

use zellij_remote_core::{AckResult, InputSender};
use zellij_remote_protocol::{
    input_event, key_event, stream_envelope, Capabilities, ClientHello, InputEvent, KeyEvent,
    KeyModifiers, ProtocolVersion, RequestControl, RowData, ScreenDelta, ScreenSnapshot,
    SpecialKey, StreamEnvelope,
};

struct ScreenBuffer {
    rows: Vec<Vec<char>>,
    cols: usize,
}

impl ScreenBuffer {
    fn new(cols: usize, rows: usize) -> Self {
        Self {
            rows: vec![vec![' '; cols]; rows],
            cols,
        }
    }

    fn apply_snapshot(&mut self, snapshot: &ScreenSnapshot) {
        if let Some(size) = &snapshot.size {
            self.cols = size.cols as usize;
            self.rows = vec![vec![' '; self.cols]; size.rows as usize];
        }

        for row_data in &snapshot.rows {
            self.apply_row_data(row_data);
        }
    }

    fn apply_delta(&mut self, delta: &ScreenDelta) {
        for patch in &delta.row_patches {
            let row_idx = patch.row as usize;
            if row_idx >= self.rows.len() {
                continue;
            }

            for run in &patch.runs {
                let col_start = run.col_start as usize;
                for (i, &codepoint) in run.codepoints.iter().enumerate() {
                    let col = col_start + i;
                    if col < self.cols {
                        self.rows[row_idx][col] = char::from_u32(codepoint).unwrap_or(' ');
                    }
                }
            }
        }
    }

    fn apply_row_data(&mut self, row_data: &RowData) {
        let row_idx = row_data.row as usize;
        if row_idx >= self.rows.len() {
            return;
        }

        for (col, &codepoint) in row_data.codepoints.iter().enumerate() {
            if col < self.cols {
                self.rows[row_idx][col] = char::from_u32(codepoint).unwrap_or(' ');
            }
        }
    }

    fn render(&self) -> Result<()> {
        let mut stdout = stdout();

        for (row_idx, row) in self.rows.iter().enumerate() {
            execute!(stdout, MoveTo(0, row_idx as u16))?;
            let line: String = row.iter().collect();
            execute!(stdout, Print(&line))?;
        }

        stdout.flush()?;
        Ok(())
    }
}

fn encode_envelope(envelope: &StreamEnvelope) -> Result<Vec<u8>> {
    let len = envelope.encoded_len();
    let mut buf = BytesMut::with_capacity(len + 5);
    prost::encoding::encode_varint(len as u64, &mut buf);
    envelope.encode(&mut buf)?;
    Ok(buf.to_vec())
}

fn decode_envelope(buf: &mut BytesMut) -> Result<Option<StreamEnvelope>> {
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

fn current_time_ms() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u32)
        .unwrap_or(0)
}

fn crossterm_key_to_proto(key: &CtKeyEvent) -> Option<InputEvent> {
    static INPUT_SEQ: AtomicU64 = AtomicU64::new(1);

    let modifiers = KeyModifiers {
        bits: {
            let mut bits = 0u32;
            if key.modifiers.contains(CtKeyModifiers::SHIFT) {
                bits |= 1;
            }
            if key.modifiers.contains(CtKeyModifiers::ALT) {
                bits |= 2;
            }
            if key.modifiers.contains(CtKeyModifiers::CONTROL) {
                bits |= 4;
            }
            if key.modifiers.contains(CtKeyModifiers::SUPER) {
                bits |= 8;
            }
            bits
        },
    };

    let key_proto = match key.code {
        KeyCode::Char(c) => Some(KeyEvent {
            modifiers: Some(modifiers),
            key: Some(key_event::Key::UnicodeScalar(c as u32)),
        }),
        KeyCode::Enter => Some(KeyEvent {
            modifiers: Some(modifiers),
            key: Some(key_event::Key::Special(SpecialKey::Enter as i32)),
        }),
        KeyCode::Esc => Some(KeyEvent {
            modifiers: Some(modifiers),
            key: Some(key_event::Key::Special(SpecialKey::Escape as i32)),
        }),
        KeyCode::Backspace => Some(KeyEvent {
            modifiers: Some(modifiers),
            key: Some(key_event::Key::Special(SpecialKey::Backspace as i32)),
        }),
        KeyCode::Tab => Some(KeyEvent {
            modifiers: Some(modifiers),
            key: Some(key_event::Key::Special(SpecialKey::Tab as i32)),
        }),
        KeyCode::Left => Some(KeyEvent {
            modifiers: Some(modifiers),
            key: Some(key_event::Key::Special(SpecialKey::Left as i32)),
        }),
        KeyCode::Right => Some(KeyEvent {
            modifiers: Some(modifiers),
            key: Some(key_event::Key::Special(SpecialKey::Right as i32)),
        }),
        KeyCode::Up => Some(KeyEvent {
            modifiers: Some(modifiers),
            key: Some(key_event::Key::Special(SpecialKey::Up as i32)),
        }),
        KeyCode::Down => Some(KeyEvent {
            modifiers: Some(modifiers),
            key: Some(key_event::Key::Special(SpecialKey::Down as i32)),
        }),
        KeyCode::Home => Some(KeyEvent {
            modifiers: Some(modifiers),
            key: Some(key_event::Key::Special(SpecialKey::Home as i32)),
        }),
        KeyCode::End => Some(KeyEvent {
            modifiers: Some(modifiers),
            key: Some(key_event::Key::Special(SpecialKey::End as i32)),
        }),
        KeyCode::PageUp => Some(KeyEvent {
            modifiers: Some(modifiers),
            key: Some(key_event::Key::Special(SpecialKey::PageUp as i32)),
        }),
        KeyCode::PageDown => Some(KeyEvent {
            modifiers: Some(modifiers),
            key: Some(key_event::Key::Special(SpecialKey::PageDown as i32)),
        }),
        KeyCode::Delete => Some(KeyEvent {
            modifiers: Some(modifiers),
            key: Some(key_event::Key::Special(SpecialKey::Delete as i32)),
        }),
        KeyCode::Insert => Some(KeyEvent {
            modifiers: Some(modifiers),
            key: Some(key_event::Key::Special(SpecialKey::Insert as i32)),
        }),
        KeyCode::F(n) => {
            let special = match n {
                1 => SpecialKey::F1,
                2 => SpecialKey::F2,
                3 => SpecialKey::F3,
                4 => SpecialKey::F4,
                5 => SpecialKey::F5,
                6 => SpecialKey::F6,
                7 => SpecialKey::F7,
                8 => SpecialKey::F8,
                9 => SpecialKey::F9,
                10 => SpecialKey::F10,
                11 => SpecialKey::F11,
                12 => SpecialKey::F12,
                _ => return None,
            };
            Some(KeyEvent {
                modifiers: Some(modifiers),
                key: Some(key_event::Key::Special(special as i32)),
            })
        },
        _ => None,
    };

    key_proto.map(|k| InputEvent {
        input_seq: INPUT_SEQ.fetch_add(1, Ordering::Relaxed),
        client_time_ms: current_time_ms(),
        payload: Some(input_event::Payload::Key(k)),
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();

    let server_url =
        std::env::var("SERVER_URL").unwrap_or_else(|_| "https://127.0.0.1:4433".to_string());
    let headless = std::env::var("HEADLESS").is_ok();

    let config = ClientConfig::builder()
        .with_bind_default()
        .with_no_cert_validation()
        .build();

    eprintln!("Connecting to {}...", server_url);
    let connection = Endpoint::client(config)?
        .connect(&server_url)
        .await
        .context("failed to connect to server")?;

    eprintln!("Connected! Opening bidirectional stream...");
    let (mut send, mut recv) = connection.open_bi().await?.await?;

    let client_hello = StreamEnvelope {
        msg: Some(stream_envelope::Msg::ClientHello(ClientHello {
            client_name: "spike-client".to_string(),
            version: Some(ProtocolVersion {
                major: zellij_remote_protocol::ZRP_VERSION_MAJOR,
                minor: zellij_remote_protocol::ZRP_VERSION_MINOR,
            }),
            capabilities: Some(Capabilities {
                supports_datagrams: false,
                max_datagram_bytes: zellij_remote_protocol::DEFAULT_MAX_DATAGRAM_BYTES,
                supports_style_dictionary: true,
                supports_styled_underlines: false,
                supports_prediction: true,
                supports_images: false,
                supports_clipboard: false,
                supports_hyperlinks: false,
            }),
            bearer_token: vec![],
            resume_token: vec![],
        })),
    };

    let encoded = encode_envelope(&client_hello)?;
    send.write_all(&encoded).await?;
    eprintln!("Sent ClientHello, waiting for ServerHello...");

    if headless {
        run_client_loop_headless(&mut recv).await
    } else {
        let mut stdout = stdout();
        execute!(stdout, EnterAlternateScreen, Hide, Clear(ClearType::All))?;
        terminal::enable_raw_mode()?;

        let result = run_client_loop(&mut send, &mut recv).await;

        terminal::disable_raw_mode()?;
        execute!(stdout, Show, LeaveAlternateScreen)?;

        result
    }
}

async fn run_client_loop_headless(recv: &mut wtransport::RecvStream) -> Result<()> {
    let mut buffer = BytesMut::new();
    let mut delta_count = 0u32;

    loop {
        let mut chunk = [0u8; 4096];
        let n = recv.read(&mut chunk).await?.unwrap_or(0);
        if n == 0 {
            println!("Connection closed by server");
            break;
        }
        buffer.extend_from_slice(&chunk[..n]);

        while let Some(envelope) = decode_envelope(&mut buffer)? {
            match envelope.msg {
                Some(stream_envelope::Msg::ServerHello(hello)) => {
                    println!(
                        "ServerHello: session={}, client_id={}",
                        hello.session_name, hello.client_id
                    );
                },
                Some(stream_envelope::Msg::ScreenSnapshot(snapshot)) => {
                    println!(
                        "ScreenSnapshot: state_id={}, size={}x{}, rows={}",
                        snapshot.state_id,
                        snapshot.size.as_ref().map(|s| s.cols).unwrap_or(0),
                        snapshot.size.as_ref().map(|s| s.rows).unwrap_or(0),
                        snapshot.rows.len()
                    );
                },
                Some(stream_envelope::Msg::ScreenDeltaStream(delta)) => {
                    delta_count += 1;
                    println!(
                        "ScreenDelta #{}: base={}, state_id={}, patches={}",
                        delta_count,
                        delta.base_state_id,
                        delta.state_id,
                        delta.row_patches.len()
                    );

                    if delta_count >= 5 {
                        println!("Received 5 deltas, stopping headless test");
                        return Ok(());
                    }
                },
                _ => {},
            }
        }
    }

    Ok(())
}

async fn run_client_loop(
    send: &mut wtransport::SendStream,
    recv: &mut wtransport::RecvStream,
) -> Result<()> {
    let mut buffer = BytesMut::new();
    let mut screen = ScreenBuffer::new(80, 24);
    let mut snapshot_received = false;
    let mut _delta_count = 0u32;
    let mut is_controller = false;
    let mut input_sender = InputSender::new(256);

    let (input_tx, mut input_rx) = mpsc::channel::<InputEvent>(64);
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_clone = shutdown.clone();

    std::thread::spawn(move || {
        while !shutdown_clone.load(Ordering::Relaxed) {
            if crossterm::event::poll(std::time::Duration::from_millis(100)).unwrap_or(false) {
                if let Ok(Event::Key(key)) = crossterm::event::read() {
                    if key.code == KeyCode::Char('c')
                        && key.modifiers.contains(CtKeyModifiers::CONTROL)
                    {
                        shutdown_clone.store(true, Ordering::Relaxed);
                        break;
                    }

                    if let Some(input_event) = crossterm_key_to_proto(&key) {
                        let _ = input_tx.blocking_send(input_event);
                    }
                }
            }
        }
    });

    loop {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        tokio::select! {
            read_result = async {
                let mut chunk = [0u8; 4096];
                recv.read(&mut chunk).await.map(|n| (n, chunk))
            } => {
                let (n, chunk) = read_result?;
                let n = n.unwrap_or(0);
                if n == 0 {
                    eprintln!("\r\nConnection closed by server");
                    break;
                }
                buffer.extend_from_slice(&chunk[..n]);

                while let Some(envelope) = decode_envelope(&mut buffer)? {
                    match envelope.msg {
                        Some(stream_envelope::Msg::ServerHello(hello)) => {
                            if let Some(lease) = &hello.lease {
                                if lease.owner_client_id == hello.client_id {
                                    is_controller = true;
                                }
                            }

                            if !is_controller {
                                let request = StreamEnvelope {
                                    msg: Some(stream_envelope::Msg::RequestControl(RequestControl {
                                        reason: "want to type".to_string(),
                                        desired_size: None,
                                        force: false,
                                    })),
                                };
                                let encoded = encode_envelope(&request)?;
                                send.write_all(&encoded).await?;
                            }

                            execute!(
                                stdout(),
                                MoveTo(0, 0),
                                Print(format!(
                                    "Session: {}, Client: {}, Controller: {}     ",
                                    hello.session_name, hello.client_id, is_controller
                                ))
                            )?;
                        }
                        Some(stream_envelope::Msg::GrantControl(_)) => {
                            is_controller = true;
                            execute!(
                                stdout(),
                                MoveTo(60, 0),
                                Print("Controller: true ")
                            )?;
                        }
                        Some(stream_envelope::Msg::DenyControl(deny)) => {
                            execute!(
                                stdout(),
                                MoveTo(0, 23),
                                Print(format!("Control denied: {}                    ", deny.reason))
                            )?;
                        }
                        Some(stream_envelope::Msg::ScreenSnapshot(snapshot)) => {
                            screen.apply_snapshot(&snapshot);
                            screen.render()?;
                            snapshot_received = true;
                        }
                        Some(stream_envelope::Msg::ScreenDeltaStream(delta)) => {
                            if !snapshot_received {
                                continue;
                            }

                            screen.apply_delta(&delta);
                            screen.render()?;
                            _delta_count += 1;
                        }
                        Some(stream_envelope::Msg::InputAck(ack)) => {
                            match input_sender.process_ack(&ack) {
                                AckResult::Ok { rtt_sample } => {
                                    if let Some(sample) = rtt_sample {
                                        execute!(
                                            stdout(),
                                            MoveTo(0, 23),
                                            Print(format!(
                                                "RTT: {}ms, Acked: {}, Inflight: {}        ",
                                                sample.rtt_ms, ack.acked_seq, input_sender.inflight_count()
                                            ))
                                        )?;
                                    }
                                }
                                AckResult::Stale => {}
                            }
                        }
                        _ => {}
                    }
                }
            }
            Some(input_event) = input_rx.recv() => {
                if is_controller && input_sender.can_send() {
                    let seq = input_event.input_seq;
                    let time_ms = input_event.client_time_ms;

                    let envelope = StreamEnvelope {
                        msg: Some(stream_envelope::Msg::InputEvent(input_event)),
                    };
                    let encoded = encode_envelope(&envelope)?;
                    send.write_all(&encoded).await?;
                    input_sender.mark_sent(seq, time_ms);
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {
            }
        }
    }

    shutdown.store(true, Ordering::Relaxed);
    Ok(())
}
