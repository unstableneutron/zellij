use anyhow::{Context, Result};
use bytes::{Buf, BytesMut};
use clap::Parser;
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{Event, KeyCode, KeyEvent as CtKeyEvent, KeyModifiers as CtKeyModifiers},
    execute,
    style::Print,
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};
use prost::Message;
use serde::Serialize;
use std::fs;
use std::io::{stdout, BufRead, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use wtransport::{ClientConfig, Endpoint};

const RESUME_TOKEN_FILE: &str = "/tmp/zellij-spike-resume-token";

use zellij_remote_bridge::{decode_datagram_envelope, encode_datagram_envelope};
use zellij_remote_core::{
    AckResult, Confidence, Cursor as CoreCursor, CursorShape, InputSender, PredictionEngine,
};
use zellij_remote_protocol::{
    datagram_envelope, input_event, key_event, protocol_error, request_snapshot, stream_envelope,
    Capabilities, ClientHello, DatagramEnvelope, InputEvent, KeyEvent, KeyModifiers,
    ProtocolVersion, RequestControl, RequestSnapshot, RowData, ScreenDelta, ScreenSnapshot,
    SpecialKey, StateAck, StreamEnvelope,
};

#[derive(Parser, Debug)]
#[clap(name = "spike_client", about = "Zellij remote spike client")]
struct Args {
    #[clap(
        short = 's',
        long,
        default_value = "https://127.0.0.1:4433",
        env = "SERVER_URL"
    )]
    server_url: String,

    #[clap(short = 't', long, env = "ZELLIJ_REMOTE_TOKEN")]
    token: Option<String>,

    #[clap(
        long,
        help = "Read token from file (must have 0600 permissions on Unix)"
    )]
    token_file: Option<String>,

    #[clap(long, env = "HEADLESS")]
    headless: bool,

    #[clap(long)]
    script: Option<String>,

    #[clap(long)]
    metrics_out: Option<String>,

    #[clap(long, default_value = "none")]
    reconnect: String,

    #[clap(long, env = "CLEAR_TOKEN")]
    clear_token: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReconnectMode {
    None,
    Once,
    Always,
    After(Duration),
}

impl ReconnectMode {
    fn parse(s: &str) -> Result<Self> {
        match s {
            "none" => Ok(ReconnectMode::None),
            "once" => Ok(ReconnectMode::Once),
            "always" => Ok(ReconnectMode::Always),
            s if s.starts_with("after=") => {
                let delay_str = s.strip_prefix("after=").unwrap();
                let delay_str = delay_str.trim_end_matches('s');
                let secs: u64 = delay_str.parse().context("invalid delay in after=Ns")?;
                Ok(ReconnectMode::After(Duration::from_secs(secs)))
            },
            _ => anyhow::bail!("invalid reconnect mode: {}", s),
        }
    }
}

#[derive(Debug, Clone)]
enum ScriptCommand {
    Sleep(u64),
    Type(String),
    Key(String),
    Reconnect,
    Quit,
}

fn parse_script(path: &str) -> Result<Vec<ScriptCommand>> {
    let file = fs::File::open(path).context("failed to open script file")?;
    let reader = std::io::BufReader::new(file);
    let mut commands = Vec::new();

    for line in reader.lines() {
        let line = line?;
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }

        let parts: Vec<&str> = line.splitn(2, ' ').collect();
        let cmd = parts[0];
        let arg = parts.get(1).map(|s| s.trim()).unwrap_or("");

        match cmd {
            "sleep" => {
                let ms: u64 = arg.parse().context("invalid sleep duration")?;
                commands.push(ScriptCommand::Sleep(ms));
            },
            "type" => {
                commands.push(ScriptCommand::Type(arg.to_string()));
            },
            "key" => {
                commands.push(ScriptCommand::Key(arg.to_string()));
            },
            "reconnect" => {
                commands.push(ScriptCommand::Reconnect);
            },
            "quit" => {
                commands.push(ScriptCommand::Quit);
            },
            _ => anyhow::bail!("unknown script command: {}", cmd),
        }
    }

    Ok(commands)
}

fn resolve_token(args: &Args) -> Result<Option<String>> {
    if let Some(ref token) = args.token {
        return Ok(Some(token.clone()));
    }

    if let Some(ref token_file) = args.token_file {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let metadata = fs::metadata(token_file)
                .with_context(|| format!("failed to read token file: {}", token_file))?;
            let mode = metadata.mode() & 0o777;
            if mode != 0o600 {
                anyhow::bail!(
                    "Token file {} has insecure permissions {:o}. Expected 0600.",
                    token_file,
                    mode
                );
            }
        }
        let token = fs::read_to_string(token_file)
            .with_context(|| format!("failed to read token file: {}", token_file))?
            .trim()
            .to_string();
        if token.is_empty() {
            anyhow::bail!("Token file {} is empty", token_file);
        }
        return Ok(Some(token));
    }

    Ok(None)
}

#[derive(Debug, Serialize, Default)]
struct Metrics {
    session_name: String,
    client_id: u64,
    connect_time_ms: u64,
    connect_times: Vec<u64>,
    total_duration_ms: u64,
    rtt_samples: Vec<u32>,
    rtt_min_ms: u32,
    rtt_avg_ms: f64,
    rtt_max_ms: u32,
    deltas_received: u64,
    deltas_via_datagram: u64,
    deltas_via_stream: u64,
    base_mismatches: u64,
    snapshots_received: u64,
    snapshots_requested: u64,
    inputs_sent: u64,
    inputs_acked: u64,
    prediction_count: u64,
    reconnect_count: u64,
    datagram_decode_errors: u64,
    errors: Vec<String>,
}

impl Metrics {
    fn finalize(&mut self) {
        if !self.rtt_samples.is_empty() {
            self.rtt_min_ms = *self.rtt_samples.iter().min().unwrap_or(&0);
            self.rtt_max_ms = *self.rtt_samples.iter().max().unwrap_or(&0);
            self.rtt_avg_ms = self.rtt_samples.iter().map(|&x| x as f64).sum::<f64>()
                / self.rtt_samples.len() as f64;
        }
    }

    fn write_to_file(&mut self, path: &str) -> Result<()> {
        self.finalize();
        let json = serde_json::to_string_pretty(self)?;
        fs::write(path, json)?;
        Ok(())
    }
}

struct ScreenBuffer {
    rows: Vec<Vec<char>>,
    cols: usize,
    cursor: CoreCursor,
}

impl ScreenBuffer {
    fn new(cols: usize, rows: usize) -> Self {
        Self {
            rows: vec![vec![' '; cols]; rows],
            cols,
            cursor: CoreCursor {
                col: 0,
                row: 0,
                visible: true,
                blink: true,
                shape: CursorShape::Block,
            },
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

        if let Some(cursor) = &snapshot.cursor {
            self.cursor.col = cursor.col;
            self.cursor.row = cursor.row;
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

        if let Some(cursor) = &delta.cursor {
            self.cursor.col = cursor.col;
            self.cursor.row = cursor.row;
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

    fn clone_with_overlay(&self, prediction_engine: &PredictionEngine) -> Self {
        let mut overlay = self.clone();
        for pred in prediction_engine.pending_predictions() {
            for &(col, row, ref cell) in &pred.cells {
                if row < overlay.rows.len() && col < overlay.cols && cell.codepoint != 0 {
                    overlay.rows[row][col] = char::from_u32(cell.codepoint).unwrap_or(' ');
                }
            }
            overlay.cursor = pred.cursor;
        }
        overlay
    }
}

impl Clone for ScreenBuffer {
    fn clone(&self) -> Self {
        Self {
            rows: self.rows.clone(),
            cols: self.cols,
            cursor: self.cursor,
        }
    }
}

fn render_screen(screen: &ScreenBuffer, pending_count: usize) -> Result<()> {
    let mut stdout = stdout();

    for (row_idx, row) in screen.rows.iter().enumerate() {
        execute!(stdout, MoveTo(0, row_idx as u16))?;
        let line: String = row.iter().collect();
        execute!(stdout, Print(&line))?;
    }

    if screen.cursor.visible {
        execute!(
            stdout,
            MoveTo(screen.cursor.col as u16, screen.cursor.row as u16)
        )?;
    }

    if pending_count > 0 {
        execute!(
            stdout,
            MoveTo(70, 0),
            Print(format!("[P:{}]", pending_count))
        )?;
    }

    stdout.flush()?;
    Ok(())
}

fn encode_envelope(envelope: &StreamEnvelope) -> Result<Vec<u8>> {
    let len = envelope.encoded_len();
    let mut buf = BytesMut::with_capacity(len + 5);
    prost::encoding::encode_varint(len as u64, &mut buf);
    envelope.encode(&mut buf)?;
    Ok(buf.to_vec())
}

fn send_state_ack(connection: &wtransport::Connection, state_id: u64, datagrams_negotiated: bool) {
    if !datagrams_negotiated {
        return;
    }

    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u32;

    let ack = StateAck {
        last_applied_state_id: state_id,
        last_received_state_id: state_id,
        client_time_ms: now_ms,
        estimated_loss_ppm: 0,
        srtt_ms: 0,
    };

    let envelope = DatagramEnvelope {
        msg: Some(datagram_envelope::Msg::StateAck(ack)),
    };
    let encoded = encode_datagram_envelope(&envelope);

    if let Err(e) = connection.send_datagram(&encoded) {
        log::trace!("Failed to send StateAck datagram: {}", e);
    } else {
        log::trace!("Sent StateAck for state_id={}", state_id);
    }
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

fn parse_key_string(key_str: &str) -> Option<InputEvent> {
    static INPUT_SEQ: AtomicU64 = AtomicU64::new(1);

    let parts: Vec<&str> = key_str.split('+').collect();
    let mut ctrl = false;
    let mut alt = false;
    let mut shift = false;
    let key_name = parts.last()?;

    for &part in parts.iter().take(parts.len().saturating_sub(1)) {
        match part.to_lowercase().as_str() {
            "ctrl" => ctrl = true,
            "alt" => alt = true,
            "shift" => shift = true,
            _ => {},
        }
    }

    let mut bits = 0u32;
    if shift {
        bits |= 1;
    }
    if alt {
        bits |= 2;
    }
    if ctrl {
        bits |= 4;
    }

    let modifiers = KeyModifiers { bits };

    let key_proto = match key_name.to_lowercase().as_str() {
        "enter" | "return" => KeyEvent {
            modifiers: Some(modifiers),
            key: Some(key_event::Key::Special(SpecialKey::Enter as i32)),
        },
        "esc" | "escape" => KeyEvent {
            modifiers: Some(modifiers),
            key: Some(key_event::Key::Special(SpecialKey::Escape as i32)),
        },
        "backspace" => KeyEvent {
            modifiers: Some(modifiers),
            key: Some(key_event::Key::Special(SpecialKey::Backspace as i32)),
        },
        "tab" => KeyEvent {
            modifiers: Some(modifiers),
            key: Some(key_event::Key::Special(SpecialKey::Tab as i32)),
        },
        "left" => KeyEvent {
            modifiers: Some(modifiers),
            key: Some(key_event::Key::Special(SpecialKey::Left as i32)),
        },
        "right" => KeyEvent {
            modifiers: Some(modifiers),
            key: Some(key_event::Key::Special(SpecialKey::Right as i32)),
        },
        "up" => KeyEvent {
            modifiers: Some(modifiers),
            key: Some(key_event::Key::Special(SpecialKey::Up as i32)),
        },
        "down" => KeyEvent {
            modifiers: Some(modifiers),
            key: Some(key_event::Key::Special(SpecialKey::Down as i32)),
        },
        "home" => KeyEvent {
            modifiers: Some(modifiers),
            key: Some(key_event::Key::Special(SpecialKey::Home as i32)),
        },
        "end" => KeyEvent {
            modifiers: Some(modifiers),
            key: Some(key_event::Key::Special(SpecialKey::End as i32)),
        },
        "pageup" => KeyEvent {
            modifiers: Some(modifiers),
            key: Some(key_event::Key::Special(SpecialKey::PageUp as i32)),
        },
        "pagedown" => KeyEvent {
            modifiers: Some(modifiers),
            key: Some(key_event::Key::Special(SpecialKey::PageDown as i32)),
        },
        "delete" => KeyEvent {
            modifiers: Some(modifiers),
            key: Some(key_event::Key::Special(SpecialKey::Delete as i32)),
        },
        "insert" => KeyEvent {
            modifiers: Some(modifiers),
            key: Some(key_event::Key::Special(SpecialKey::Insert as i32)),
        },
        "space" => KeyEvent {
            modifiers: Some(modifiers),
            key: Some(key_event::Key::UnicodeScalar(' ' as u32)),
        },
        s if s.len() == 1 => {
            let c = s.chars().next()?;
            KeyEvent {
                modifiers: Some(modifiers),
                key: Some(key_event::Key::UnicodeScalar(c as u32)),
            }
        },
        _ => return None,
    };

    Some(InputEvent {
        input_seq: INPUT_SEQ.fetch_add(1, Ordering::Relaxed),
        client_time_ms: current_time_ms(),
        payload: Some(input_event::Payload::Key(key_proto)),
    })
}

fn char_to_input_event(c: char) -> InputEvent {
    static INPUT_SEQ: AtomicU64 = AtomicU64::new(1);

    let key_proto = KeyEvent {
        modifiers: Some(KeyModifiers { bits: 0 }),
        key: Some(key_event::Key::UnicodeScalar(c as u32)),
    };

    InputEvent {
        input_seq: INPUT_SEQ.fetch_add(1, Ordering::Relaxed),
        client_time_ms: current_time_ms(),
        payload: Some(input_event::Payload::Key(key_proto)),
    }
}

fn load_resume_token() -> Option<Vec<u8>> {
    match std::fs::read(RESUME_TOKEN_FILE) {
        Ok(data) if !data.is_empty() => Some(data),
        Ok(_) => None,
        Err(_) => None,
    }
}

#[cfg(unix)]
fn save_resume_token(token: &[u8]) {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    if token.is_empty() {
        let _ = fs::remove_file(RESUME_TOKEN_FILE);
        return;
    }

    let path = format!("{}-{}", RESUME_TOKEN_FILE, std::process::id());

    match std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)
    {
        Ok(mut file) => {
            if let Err(e) = file.write_all(token) {
                log::warn!("Failed to write resume token: {}", e);
                return;
            }
            if let Err(e) = std::fs::rename(&path, RESUME_TOKEN_FILE) {
                log::warn!("Failed to rename resume token file: {}", e);
                let _ = std::fs::remove_file(&path);
            }
        },
        Err(e) => {
            log::warn!("Failed to create resume token file: {}", e);
        },
    }
}

#[cfg(not(unix))]
fn save_resume_token(token: &[u8]) {
    if token.is_empty() {
        let _ = fs::remove_file(RESUME_TOKEN_FILE);
    } else if let Err(e) = std::fs::write(RESUME_TOKEN_FILE, token) {
        log::warn!("Failed to save resume token: {}", e);
    }
}

fn clear_resume_token() {
    let _ = fs::remove_file(RESUME_TOKEN_FILE);
}

#[derive(Debug)]
enum ClientResult {
    Disconnected,
    ScriptReconnect,
    ScriptQuit,
    Shutdown,
}

struct ClientState {
    args: Args,
    metrics: Metrics,
    start_time: Instant,
    reconnect_mode: ReconnectMode,
    script_commands: Option<Vec<ScriptCommand>>,
    script_index: usize,
}

impl ClientState {
    fn new(args: Args) -> Result<Self> {
        let reconnect_mode = ReconnectMode::parse(&args.reconnect)?;
        let script_commands = args.script.as_ref().map(|p| parse_script(p)).transpose()?;

        Ok(Self {
            args,
            metrics: Metrics::default(),
            start_time: Instant::now(),
            reconnect_mode,
            script_commands,
            script_index: 0,
        })
    }

    fn should_reconnect(&self, attempts: u64) -> bool {
        match self.reconnect_mode {
            ReconnectMode::None => false,
            ReconnectMode::Once => attempts == 0,
            ReconnectMode::Always => true,
            ReconnectMode::After(_) => true,
        }
    }

    fn reconnect_delay(&self) -> Option<Duration> {
        match self.reconnect_mode {
            ReconnectMode::After(d) => Some(d),
            _ => None,
        }
    }
}

static CONNECT_COUNT: AtomicU64 = AtomicU64::new(0);

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();

    let args = Args::parse();
    let mut state = ClientState::new(args)?;

    if state.args.clear_token {
        clear_resume_token();
        eprintln!("Cleared stored resume token");
    }

    let config = ClientConfig::builder()
        .with_bind_default()
        .with_no_cert_validation()
        .build();

    let endpoint = Endpoint::client(config)?;

    let mut reconnect_attempts = 0u64;

    loop {
        let result = run_connection(&endpoint, &mut state).await;

        match result {
            Ok(ClientResult::ScriptQuit) | Ok(ClientResult::Shutdown) => {
                break;
            },
            Ok(ClientResult::ScriptReconnect) => {
                state.metrics.reconnect_count += 1;
                if let Some(delay) = state.reconnect_delay() {
                    eprintln!("Script reconnect, waiting {:?}...", delay);
                    tokio::time::sleep(delay).await;
                }
                continue;
            },
            Ok(ClientResult::Disconnected) => {
                if state.should_reconnect(reconnect_attempts) {
                    state.metrics.reconnect_count += 1;
                    reconnect_attempts += 1;
                    if let Some(delay) = state.reconnect_delay() {
                        eprintln!("Disconnected, reconnecting after {:?}...", delay);
                        tokio::time::sleep(delay).await;
                    } else {
                        eprintln!("Disconnected, reconnecting...");
                    }
                    continue;
                } else {
                    break;
                }
            },
            Err(e) => {
                state.metrics.errors.push(e.to_string());
                if state.should_reconnect(reconnect_attempts) {
                    state.metrics.reconnect_count += 1;
                    reconnect_attempts += 1;
                    if let Some(delay) = state.reconnect_delay() {
                        eprintln!("Error: {}, reconnecting after {:?}...", e, delay);
                        tokio::time::sleep(delay).await;
                    } else {
                        eprintln!("Error: {}, reconnecting...", e);
                    }
                    continue;
                } else {
                    eprintln!("Error: {}", e);
                    break;
                }
            },
        }
    }

    state.metrics.total_duration_ms = state.start_time.elapsed().as_millis() as u64;

    if let Some(ref path) = state.args.metrics_out {
        state.metrics.write_to_file(path)?;
        eprintln!("Metrics written to {}", path);
    }

    Ok(())
}

async fn run_connection(
    endpoint: &Endpoint<wtransport::endpoint::endpoint_side::Client>,
    state: &mut ClientState,
) -> Result<ClientResult> {
    let token = resolve_token(&state.args)?;
    let bearer_token = token
        .as_ref()
        .map(|s| s.as_bytes().to_vec())
        .unwrap_or_default();

    let resume_token = load_resume_token().unwrap_or_default();
    if !resume_token.is_empty() {
        eprintln!(
            "Found stored resume token ({} bytes), will attempt resume",
            resume_token.len()
        );
    }

    if !bearer_token.is_empty() {
        eprintln!("Using bearer token ({} bytes)", bearer_token.len());
    }

    let connect_start = Instant::now();
    eprintln!("Connecting to {}...", state.args.server_url);
    let connection = endpoint
        .connect(&state.args.server_url)
        .await
        .context("failed to connect to server")?;

    let connect_time_ms = connect_start.elapsed().as_millis() as u64;
    let count = CONNECT_COUNT.fetch_add(1, Ordering::Relaxed);
    let likely_0rtt = count > 0 && connect_time_ms < 100;

    log::info!(
        "Connect #{}: {:.2}ms {}",
        count,
        connect_time_ms as f64,
        if likely_0rtt { "(likely 0-RTT)" } else { "" }
    );
    eprintln!(
        "Connect #{}: {}ms{}",
        count,
        connect_time_ms,
        if likely_0rtt { " (likely 0-RTT)" } else { "" }
    );

    state.metrics.connect_time_ms = connect_time_ms;
    state.metrics.connect_times.push(connect_time_ms);
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
                supports_datagrams: true,
                max_datagram_bytes: zellij_remote_protocol::DEFAULT_MAX_DATAGRAM_BYTES,
                supports_style_dictionary: true,
                supports_styled_underlines: false,
                supports_prediction: true,
                supports_images: false,
                supports_clipboard: false,
                supports_hyperlinks: false,
            }),
            bearer_token,
            resume_token,
        })),
    };

    let encoded = encode_envelope(&client_hello)?;
    send.write_all(&encoded).await?;
    eprintln!("Sent ClientHello, waiting for ServerHello...");

    if state.args.headless {
        run_client_loop_headless(&mut recv, state).await
    } else {
        let mut stdout = stdout();
        execute!(stdout, EnterAlternateScreen, Hide, Clear(ClearType::All))?;
        terminal::enable_raw_mode()?;

        let result = run_client_loop(&connection, &mut send, &mut recv, state).await;

        terminal::disable_raw_mode()?;
        execute!(stdout, Show, LeaveAlternateScreen)?;

        result
    }
}

async fn run_client_loop_headless(
    recv: &mut wtransport::RecvStream,
    state: &mut ClientState,
) -> Result<ClientResult> {
    let mut buffer = BytesMut::new();
    let mut delta_count = 0u32;

    loop {
        let mut chunk = [0u8; 4096];
        let n = recv.read(&mut chunk).await?.unwrap_or(0);
        if n == 0 {
            println!("Connection closed by server");
            return Ok(ClientResult::Disconnected);
        }
        buffer.extend_from_slice(&chunk[..n]);

        while let Some(envelope) = decode_envelope(&mut buffer)? {
            match envelope.msg {
                Some(stream_envelope::Msg::ServerHello(hello)) => {
                    println!(
                        "ServerHello: session={}, client_id={}, resume_token_len={}",
                        hello.session_name,
                        hello.client_id,
                        hello.resume_token.len()
                    );
                    state.metrics.session_name = hello.session_name;
                    state.metrics.client_id = hello.client_id;
                    save_resume_token(&hello.resume_token);
                },
                Some(stream_envelope::Msg::ScreenSnapshot(snapshot)) => {
                    println!(
                        "ScreenSnapshot: state_id={}, size={}x{}, rows={}",
                        snapshot.state_id,
                        snapshot.size.as_ref().map(|s| s.cols).unwrap_or(0),
                        snapshot.size.as_ref().map(|s| s.rows).unwrap_or(0),
                        snapshot.rows.len()
                    );
                    state.metrics.snapshots_received += 1;
                    println!("Received snapshot, stopping headless test");
                    return Ok(ClientResult::ScriptQuit);
                },

                Some(stream_envelope::Msg::ScreenDeltaStream(delta)) => {
                    delta_count += 1;
                    state.metrics.deltas_received += 1;
                    println!(
                        "ScreenDelta #{}: base={}, state_id={}, patches={}",
                        delta_count,
                        delta.base_state_id,
                        delta.state_id,
                        delta.row_patches.len()
                    );
                },
                Some(stream_envelope::Msg::ProtocolError(error)) => {
                    if error.code == protocol_error::Code::Unauthorized as i32 {
                        eprintln!("Authentication failed. Check your --token, --token-file, or ZELLIJ_REMOTE_TOKEN.");
                    } else {
                        eprintln!("Server error: {} (code={})", error.message, error.code);
                    }
                    if error.fatal {
                        return Ok(ClientResult::Disconnected);
                    }
                },
                _ => {},
            }
        }
    }
}

async fn run_client_loop(
    connection: &wtransport::Connection,
    send: &mut wtransport::SendStream,
    recv: &mut wtransport::RecvStream,
    state: &mut ClientState,
) -> Result<ClientResult> {
    let mut buffer = BytesMut::new();
    let mut confirmed_screen = ScreenBuffer::new(80, 24);
    let mut snapshot_received = false;
    let mut _delta_count = 0u32;
    let mut is_controller = false;
    let mut input_sender = InputSender::new(256);
    let mut prediction_engine = PredictionEngine::new();
    let mut last_applied_state_id: u64 = 0;
    let mut consecutive_mismatches: u32 = 0;
    let mut snapshot_in_flight: bool = false;
    let datagrams_negotiated = connection.max_datagram_size().is_some();

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

    let (script_tx, mut script_rx) = mpsc::channel::<ScriptCommand>(64);
    let script_index_update = Arc::new(AtomicU64::new(state.script_index as u64));
    if let Some(ref commands) = state.script_commands {
        let commands = commands.clone();
        let start_index = state.script_index;
        let shutdown_clone = shutdown.clone();
        let script_index_update_clone = script_index_update.clone();
        tokio::spawn(async move {
            for (i, cmd) in commands.iter().enumerate().skip(start_index) {
                if shutdown_clone.load(Ordering::Relaxed) {
                    break;
                }
                if let ScriptCommand::Sleep(ms) = cmd {
                    tokio::time::sleep(Duration::from_millis(*ms)).await;
                } else if script_tx.send(cmd.clone()).await.is_err() {
                    break;
                }
                if matches!(cmd, ScriptCommand::Quit | ScriptCommand::Reconnect) {
                    script_index_update_clone.store((i + 1) as u64, Ordering::Relaxed);
                    break;
                }
                script_index_update_clone.store((i + 1) as u64, Ordering::Relaxed);
            }
        });
    }

    loop {
        if shutdown.load(Ordering::Relaxed) {
            return Ok(ClientResult::Shutdown);
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
                    return Ok(ClientResult::Disconnected);
                }
                buffer.extend_from_slice(&chunk[..n]);

                while let Some(envelope) = decode_envelope(&mut buffer)? {
                    match envelope.msg {
                        Some(stream_envelope::Msg::ServerHello(hello)) => {
                            state.metrics.session_name = hello.session_name.clone();
                            state.metrics.client_id = hello.client_id;
                            save_resume_token(&hello.resume_token);

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
                        Some(stream_envelope::Msg::ProtocolError(error)) => {
                            if error.code == protocol_error::Code::Unauthorized as i32 {
                                eprintln!("\r\nAuthentication failed. Check your --token, --token-file, or ZELLIJ_REMOTE_TOKEN.");
                            } else {
                                eprintln!("\r\nServer error: {} (code={})", error.message, error.code);
                            }
                            if error.fatal {
                                return Ok(ClientResult::Disconnected);
                            }
                        }
                        Some(stream_envelope::Msg::ScreenSnapshot(snapshot)) => {
                            prediction_engine.clear();
                            confirmed_screen.apply_snapshot(&snapshot);
                            render_screen(&confirmed_screen, 0)?;
                            snapshot_received = true;
                            snapshot_in_flight = false;
                            last_applied_state_id = snapshot.state_id;
                            consecutive_mismatches = 0;
                            state.metrics.snapshots_received += 1;
                            send_state_ack(&connection, snapshot.state_id, datagrams_negotiated);
                        }

                        Some(stream_envelope::Msg::ScreenDeltaStream(delta)) => {
                            if !snapshot_received {
                                continue;
                            }

                            if delta.state_id <= last_applied_state_id {
                                log::trace!(
                                    "Dropping old/duplicate stream delta: state_id={} <= last_applied={}",
                                    delta.state_id,
                                    last_applied_state_id
                                );
                                continue;
                            }

                            if delta.base_state_id != last_applied_state_id {
                                consecutive_mismatches += 1;
                                state.metrics.base_mismatches += 1;

                                if consecutive_mismatches >= 3 && !snapshot_in_flight {
                                    let request = StreamEnvelope {
                                        msg: Some(stream_envelope::Msg::RequestSnapshot(RequestSnapshot {
                                            reason: request_snapshot::Reason::BaseMismatch as i32,
                                            known_state_id: last_applied_state_id,
                                        })),
                                    };
                                    let encoded = encode_envelope(&request)?;
                                    send.write_all(&encoded).await?;
                                    state.metrics.snapshots_requested += 1;
                                    snapshot_in_flight = true;
                                    consecutive_mismatches = 0;
                                } else if snapshot_in_flight {
                                    log::trace!("Ignoring stream delta mismatch while snapshot in flight");
                                }
                                continue;
                            }

                            let server_cursor = CoreCursor {
                                col: delta.cursor.as_ref().map(|c| c.col).unwrap_or(confirmed_screen.cursor.col),
                                row: delta.cursor.as_ref().map(|c| c.row).unwrap_or(confirmed_screen.cursor.row),
                                visible: true,
                                blink: true,
                                shape: CursorShape::Block,
                            };

                            prediction_engine.reconcile(
                                delta.delivered_input_watermark,
                                &server_cursor,
                            );

                            confirmed_screen.apply_delta(&delta);
                            last_applied_state_id = delta.state_id;
                            consecutive_mismatches = 0;

                            let display = confirmed_screen.clone_with_overlay(&prediction_engine);
                            render_screen(&display, prediction_engine.pending_count())?;
                            _delta_count += 1;
                            state.metrics.deltas_received += 1;
                            state.metrics.deltas_via_stream += 1;
                            send_state_ack(&connection, delta.state_id, datagrams_negotiated);
                        }
                        Some(stream_envelope::Msg::InputAck(ack)) => {
                            match input_sender.process_ack(&ack) {
                                AckResult::Ok { rtt_sample } => {
                                    state.metrics.inputs_acked += 1;
                                    if let Some(sample) = rtt_sample {
                                        state.metrics.rtt_samples.push(sample.rtt_ms);
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
                    send_input(send, &mut input_sender, &mut prediction_engine, &confirmed_screen, &input_event, state).await?;
                }
            }
            Some(script_cmd) = script_rx.recv() => {
                match script_cmd {
                    ScriptCommand::Sleep(_) => {
                    },
                    ScriptCommand::Type(text) => {
                        for c in text.chars() {
                            let input_event = char_to_input_event(c);
                            if is_controller && input_sender.can_send() {
                                send_input(send, &mut input_sender, &mut prediction_engine, &confirmed_screen, &input_event, state).await?;
                            }
                            tokio::time::sleep(Duration::from_millis(10)).await;
                        }
                    },
                    ScriptCommand::Key(key_str) => {
                        if let Some(input_event) = parse_key_string(&key_str) {
                            if is_controller && input_sender.can_send() {
                                send_input(send, &mut input_sender, &mut prediction_engine, &confirmed_screen, &input_event, state).await?;
                            }
                        }
                    },
                    ScriptCommand::Reconnect => {
                        shutdown.store(true, Ordering::Relaxed);
                        state.script_index = script_index_update.load(Ordering::Relaxed) as usize;
                        return Ok(ClientResult::ScriptReconnect);
                    },
                    ScriptCommand::Quit => {
                        shutdown.store(true, Ordering::Relaxed);
                        state.script_index = script_index_update.load(Ordering::Relaxed) as usize;
                        return Ok(ClientResult::ScriptQuit);
                    },
                }
            }
            datagram_result = connection.receive_datagram() => {
                match datagram_result {
                    Ok(datagram) => {
                        match decode_datagram_envelope(&datagram) {
                            Ok(envelope) => {
                            match envelope.msg {
                                Some(datagram_envelope::Msg::ScreenDelta(delta)) => {
                                    if !snapshot_received {
                                        continue;
                                    }

                                    // First: Drop old/duplicate datagrams
                                    if delta.state_id <= last_applied_state_id {
                                        log::trace!(
                                            "Dropping old/duplicate datagram: state_id={} <= last_applied={}",
                                            delta.state_id,
                                            last_applied_state_id
                                        );
                                        continue;
                                    }

                                    // Second: Check base mismatch
                                    if delta.base_state_id != last_applied_state_id {
                                        consecutive_mismatches += 1;
                                        state.metrics.base_mismatches += 1;

                                        if consecutive_mismatches >= 3 && !snapshot_in_flight {
                                            let request = StreamEnvelope {
                                                msg: Some(stream_envelope::Msg::RequestSnapshot(RequestSnapshot {
                                                    reason: request_snapshot::Reason::BaseMismatch as i32,
                                                    known_state_id: last_applied_state_id,
                                                })),
                                            };
                                            let encoded = encode_envelope(&request)?;
                                            send.write_all(&encoded).await?;
                                            state.metrics.snapshots_requested += 1;
                                            snapshot_in_flight = true;
                                            consecutive_mismatches = 0;
                                        } else if snapshot_in_flight {
                                            log::trace!("Ignoring mismatch while snapshot in flight");
                                        }
                                        continue;
                                    }

                                    let server_cursor = CoreCursor {
                                        col: delta.cursor.as_ref().map(|c| c.col).unwrap_or(confirmed_screen.cursor.col),
                                        row: delta.cursor.as_ref().map(|c| c.row).unwrap_or(confirmed_screen.cursor.row),
                                        visible: true,
                                        blink: true,
                                        shape: CursorShape::Block,
                                    };

                                    prediction_engine.reconcile(
                                        delta.delivered_input_watermark,
                                        &server_cursor,
                                    );

                                    confirmed_screen.apply_delta(&delta);
                                    last_applied_state_id = delta.state_id;
                                    consecutive_mismatches = 0;

                                    let display = confirmed_screen.clone_with_overlay(&prediction_engine);
                                    render_screen(&display, prediction_engine.pending_count())?;
                                    _delta_count += 1;
                                    state.metrics.deltas_received += 1;
                                    state.metrics.deltas_via_datagram += 1;
                                    send_state_ack(&connection, delta.state_id, datagrams_negotiated);
                                }
                                _ => {}
                            }
                            }
                            Err(e) => {
                                log::trace!("Datagram decode error: {}", e);
                                state.metrics.datagram_decode_errors += 1;
                            }
                        }
                    }
                    Err(_) => {}
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {
            }
        }
    }
}

async fn send_input(
    send: &mut wtransport::SendStream,
    input_sender: &mut InputSender,
    prediction_engine: &mut PredictionEngine,
    confirmed_screen: &ScreenBuffer,
    input_event: &InputEvent,
    state: &mut ClientState,
) -> Result<()> {
    let seq = input_event.input_seq;
    let time_ms = input_event.client_time_ms;

    if let Some(input_event::Payload::Key(ref key)) = input_event.payload {
        if let Some(key_event::Key::UnicodeScalar(codepoint)) = key.key {
            if let Some(ch) = char::from_u32(codepoint) {
                if prediction_engine.confidence(ch) != Confidence::None {
                    let overlay_cursor = if prediction_engine.pending_count() > 0 {
                        prediction_engine
                            .pending_predictions()
                            .last()
                            .map(|p| p.cursor)
                            .unwrap_or(confirmed_screen.cursor)
                    } else {
                        confirmed_screen.cursor
                    };
                    if prediction_engine
                        .predict_char(ch, seq, &overlay_cursor, confirmed_screen.cols)
                        .is_some()
                    {
                        state.metrics.prediction_count += 1;
                        let display = confirmed_screen.clone_with_overlay(prediction_engine);
                        render_screen(&display, prediction_engine.pending_count())?;
                    }
                }
            }
        }
    }

    let envelope = StreamEnvelope {
        msg: Some(stream_envelope::Msg::InputEvent(input_event.clone())),
    };
    let encoded = encode_envelope(&envelope)?;
    send.write_all(&encoded).await?;
    input_sender.mark_sent(seq, time_ms);
    state.metrics.inputs_sent += 1;

    Ok(())
}
