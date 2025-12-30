use anyhow::{Context, Result};
use bytes::{Buf, BytesMut};
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    execute,
    style::Print,
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};
use prost::Message;
use std::io::{stdout, Write};
use wtransport::{ClientConfig, Endpoint};

use zellij_remote_protocol::{
    stream_envelope, Capabilities, ClientHello, ProtocolVersion, RowData, ScreenDelta,
    ScreenSnapshot, StreamEnvelope,
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
                        self.rows[row_idx][col] =
                            char::from_u32(codepoint).unwrap_or(' ');
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

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();

    let server_url = std::env::var("SERVER_URL")
        .unwrap_or_else(|_| "https://127.0.0.1:4433".to_string());
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

    // Send ClientHello
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
        // Headless mode for testing - just log messages
        run_client_loop_headless(&mut recv).await
    } else {
        // Interactive mode with terminal rendering
        let mut stdout = stdout();
        execute!(stdout, EnterAlternateScreen, Hide, Clear(ClearType::All))?;
        terminal::enable_raw_mode()?;

        let result = run_client_loop(&mut recv).await;

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
                }
                Some(stream_envelope::Msg::ScreenSnapshot(snapshot)) => {
                    println!(
                        "ScreenSnapshot: state_id={}, size={}x{}, rows={}",
                        snapshot.state_id,
                        snapshot.size.as_ref().map(|s| s.cols).unwrap_or(0),
                        snapshot.size.as_ref().map(|s| s.rows).unwrap_or(0),
                        snapshot.rows.len()
                    );
                }
                Some(stream_envelope::Msg::ScreenDeltaStream(delta)) => {
                    delta_count += 1;
                    println!(
                        "ScreenDelta #{}: base={}, state_id={}, patches={}",
                        delta_count,
                        delta.base_state_id,
                        delta.state_id,
                        delta.row_patches.len()
                    );
                    
                    // Stop after a few deltas in headless mode
                    if delta_count >= 5 {
                        println!("Received 5 deltas, stopping headless test");
                        return Ok(());
                    }
                }
                _ => {}
            }
        }
    }

    Ok(())
}

async fn run_client_loop(recv: &mut wtransport::RecvStream) -> Result<()> {
    let mut buffer = BytesMut::new();
    let mut screen = ScreenBuffer::new(80, 24);
    let mut snapshot_received = false;
    let mut delta_count = 0u32;

    loop {
        let mut chunk = [0u8; 4096];
        let n = recv.read(&mut chunk).await?.unwrap_or(0);
        if n == 0 {
            eprintln!("\r\nConnection closed by server");
            break;
        }
        buffer.extend_from_slice(&chunk[..n]);

        while let Some(envelope) = decode_envelope(&mut buffer)? {
            match envelope.msg {
                Some(stream_envelope::Msg::ServerHello(hello)) => {
                    // Show briefly then continue
                    execute!(
                        stdout(),
                        MoveTo(0, 0),
                        Print(format!(
                            "ServerHello: session={}, client_id={}",
                            hello.session_name, hello.client_id
                        ))
                    )?;
                }
                Some(stream_envelope::Msg::ScreenSnapshot(snapshot)) => {
                    screen.apply_snapshot(&snapshot);
                    screen.render()?;
                    snapshot_received = true;

                    // Show status
                    execute!(
                        stdout(),
                        MoveTo(0, 23),
                        Print(format!(
                            "Snapshot received: state_id={}, rows={}        ",
                            snapshot.state_id,
                            snapshot.rows.len()
                        ))
                    )?;
                    stdout().flush()?;
                }
                Some(stream_envelope::Msg::ScreenDeltaStream(delta)) => {
                    if !snapshot_received {
                        continue;
                    }

                    screen.apply_delta(&delta);
                    screen.render()?;
                    delta_count += 1;

                    // Show status
                    execute!(
                        stdout(),
                        MoveTo(0, 23),
                        Print(format!(
                            "Delta #{}: state_id={}, patches={}        ",
                            delta_count,
                            delta.state_id,
                            delta.row_patches.len()
                        ))
                    )?;
                    stdout().flush()?;
                }
                _ => {
                    // Ignore other messages for now
                }
            }
        }
    }

    Ok(())
}
