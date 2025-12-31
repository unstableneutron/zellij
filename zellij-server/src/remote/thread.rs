use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use anyhow::{Context, Result};
use bytes::BytesMut;
use prost::Message;
use tokio::sync::{mpsc, RwLock};
use wtransport::{Endpoint, Identity, ServerConfig};
use zellij_remote_bridge::encode_envelope;
use zellij_remote_core::{FrameStore, LeaseResult, RenderUpdate};
use zellij_remote_protocol::{
    protocol_error, stream_envelope, Capabilities, ClientHello, ControllerLease, DenyControl,
    DisplaySize, GrantControl, ProtocolError, ProtocolVersion, ServerHello, SessionState,
    StreamEnvelope,
};
use zellij_utils::channels::{Receiver, SenderWithContext};
use zellij_utils::errors::ErrorContext;
use zellij_utils::pane_size::Size;

use super::input_translate::translate_input;
use super::instruction::RemoteInstruction;
use super::manager::RemoteManager;
use crate::screen::ScreenInstruction;
use crate::ClientId;

static REMOTE_CLIENT_ID_COUNTER: AtomicU64 = AtomicU64::new(1);
static TEST_KNOBS: OnceLock<TestKnobs> = OnceLock::new();

struct TestKnobs {
    drop_delta_nth: Option<u32>,
    delay_send_ms: Option<u64>,
    force_snapshot_every: Option<u32>,
    log_frame_stats: bool,
}

impl TestKnobs {
    fn from_env() -> Self {
        Self {
            drop_delta_nth: std::env::var("ZELLIJ_REMOTE_DROP_DELTA_NTH")
                .ok()
                .and_then(|s| s.parse().ok()),
            delay_send_ms: std::env::var("ZELLIJ_REMOTE_DELAY_SEND_MS")
                .ok()
                .and_then(|s| s.parse().ok()),
            force_snapshot_every: std::env::var("ZELLIJ_REMOTE_FORCE_SNAPSHOT_EVERY")
                .ok()
                .and_then(|s| s.parse().ok()),
            log_frame_stats: std::env::var("ZELLIJ_REMOTE_LOG_FRAME_STATS")
                .ok()
                .map(|s| s == "1")
                .unwrap_or(false),
        }
    }

    fn get() -> &'static TestKnobs {
        TEST_KNOBS.get_or_init(Self::from_env)
    }

    fn is_any_active(&self) -> bool {
        self.drop_delta_nth.is_some()
            || self.delay_send_ms.is_some()
            || self.force_snapshot_every.is_some()
            || self.log_frame_stats
    }

    fn log_active_knobs(&self) {
        if !self.is_any_active() {
            return;
        }

        let mut active = Vec::new();
        if let Some(n) = self.drop_delta_nth {
            active.push(format!("DROP_DELTA_NTH={}", n));
        }
        if let Some(ms) = self.delay_send_ms {
            active.push(format!("DELAY_SEND_MS={}", ms));
        }
        if let Some(n) = self.force_snapshot_every {
            active.push(format!("FORCE_SNAPSHOT_EVERY={}", n));
        }
        if self.log_frame_stats {
            active.push("LOG_FRAME_STATS=1".to_string());
        }
        log::warn!(
            "Remote server test knobs active: {}",
            active.join(", ")
        );
    }
}

const MAX_FRAME_SIZE: usize = 1_048_576; // 1 MB
const CLIENT_CHANNEL_SIZE: usize = 4;

/// Configuration for the remote server
pub struct RemoteConfig {
    pub listen_addr: SocketAddr,
    pub session_name: String,
    pub initial_size: Size,
    pub to_screen: SenderWithContext<ScreenInstruction>,
    pub bearer_token: Option<Vec<u8>>,
}

impl std::fmt::Debug for RemoteConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteConfig")
            .field("listen_addr", &self.listen_addr)
            .field("session_name", &self.session_name)
            .field("initial_size", &self.initial_size)
            .field("bearer_token", &self.bearer_token.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

/// Per-client WebTransport connection state (M1: uses channel instead of raw stream)
struct ClientConnection {
    sender: mpsc::Sender<StreamEnvelope>,
    #[allow(dead_code)]
    remote_id: u64,
}

/// Shared state between the main loop and connection handlers
struct SharedState {
    manager: RemoteManager,
    #[allow(dead_code)]
    current_frame: Option<FrameStore>,
    session_name: String,
    to_screen: SenderWithContext<ScreenInstruction>,
    active_zellij_client: Option<ClientId>,
    frame_count: u32,
    delta_count: u32,
    dropped_delta_count: u32,
}

/// Message from connection handlers to the main loop
enum ConnectionEvent {
    ClientConnected {
        remote_id: u64,
        send: wtransport::SendStream,
    },
    ClientDisconnected {
        remote_id: u64,
    },
    InputReceived {
        remote_id: u64,
        input: zellij_remote_protocol::InputEvent,
    },
    RequestControl {
        remote_id: u64,
        request: zellij_remote_protocol::RequestControl,
    },
}

/// Main entry point for the remote thread
pub fn remote_thread_main(
    receiver: Receiver<(RemoteInstruction, ErrorContext)>,
    config: RemoteConfig,
) -> Result<()> {
    log::info!(
        "Remote thread starting: listen_addr={}, session={}",
        config.listen_addr,
        config.session_name
    );

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .thread_name("remote-tokio")
        .build()
        .context("failed to create tokio runtime for remote thread")?;

    rt.block_on(async { run_remote_server(receiver, config).await })
}

async fn run_remote_server(
    receiver: Receiver<(RemoteInstruction, ErrorContext)>,
    config: RemoteConfig,
) -> Result<()> {
    let bearer_token = config.bearer_token.clone();

    if bearer_token.is_none() {
        log::warn!("Remote server running WITHOUT authentication - any client can connect!");
    }

    let is_loopback = config.listen_addr.ip().is_loopback();
    if !is_loopback && bearer_token.is_none() {
        log::error!(
            "CRITICAL SECURITY WARNING: Remote server binding to non-loopback address {} \
             without authentication! This exposes your session to the network without any protection. \
             Set ZELLIJ_REMOTE_TOKEN environment variable to enable authentication.",
            config.listen_addr.ip()
        );
    }

    TestKnobs::get().log_active_knobs();

    let shared_state = Arc::new(RwLock::new(SharedState {
        manager: RemoteManager::new(config.initial_size.cols, config.initial_size.rows),
        current_frame: None,
        session_name: config.session_name.clone(),
        to_screen: config.to_screen,
        active_zellij_client: None,
        frame_count: 0,
        delta_count: 0,
        dropped_delta_count: 0,
    }));

    let (conn_event_tx, mut conn_event_rx) = mpsc::channel::<ConnectionEvent>(64);
    let mut clients: HashMap<u64, ClientConnection> = HashMap::new();

    let identity = Identity::self_signed(["localhost", "zellij-remote"])
        .map_err(|e| anyhow::anyhow!("failed to create self-signed identity: {}", e))?;

    let server_config = ServerConfig::builder()
        .with_bind_address(config.listen_addr)
        .with_identity(identity)
        .build();

    let server = Endpoint::server(server_config)?;

    log::info!(
        "WebTransport server listening on {}{}",
        config.listen_addr,
        if bearer_token.is_some() { " (authenticated)" } else { " (UNAUTHENTICATED)" }
    );

    // M3: Spawn a dedicated task for blocking recv instead of spawning per-receive
    let (instruction_tx, mut instruction_rx) = mpsc::channel::<RemoteInstruction>(64);
    tokio::task::spawn_blocking({
        let receiver = receiver.clone();
        move || {
            loop {
                match receiver.recv() {
                    Ok((instruction, _err_ctx)) => {
                        if instruction_tx.blocking_send(instruction).is_err() {
                            break;
                        }
                    }
                    Err(_) => {
                        break;
                    }
                }
            }
        }
    });

    loop {
        tokio::select! {
            biased;

            Some(instruction) = instruction_rx.recv() => {
                let should_exit = handle_instruction(
                    &shared_state,
                    &mut clients,
                    instruction,
                ).await?;
                if should_exit {
                    log::info!("Remote thread received shutdown signal");
                    break;
                }
            }

            incoming = server.accept() => {
                let session_request = incoming.await?;
                log::info!("Incoming WebTransport connection from {}", session_request.authority());

                let connection = session_request.accept().await?;
                let shared_state = shared_state.clone();
                let conn_event_tx = conn_event_tx.clone();
                let bearer_token = bearer_token.clone();

                tokio::spawn(async move {
                    if let Err(e) = handle_connection(connection, shared_state, conn_event_tx, bearer_token).await {
                        log::error!("Connection error: {}", e);
                    }
                });
            }

            Some(event) = conn_event_rx.recv() => {
                handle_connection_event(&shared_state, &mut clients, event).await?;
            }
        }
    }

    log::info!("Remote thread shutting down");
    Ok(())
}

async fn handle_instruction(
    shared_state: &Arc<RwLock<SharedState>>,
    clients: &mut HashMap<u64, ClientConnection>,
    instruction: RemoteInstruction,
) -> Result<bool> {
    match instruction {
        RemoteInstruction::FrameReady {
            client_id: _,
            frame_store,
            style_table,
        } => {
            let knobs = TestKnobs::get();

            // M2: Clone data needed for sending before releasing lock
            #[allow(clippy::type_complexity)]
            let (updates_to_send, delay_ms): (Vec<(u64, StreamEnvelope, bool, usize)>, Option<u64>) = {
                let mut state = shared_state.write().await;
                state.current_frame = Some(frame_store.clone());
                state.frame_count = state.frame_count.wrapping_add(1);
                *state.manager.style_table_mut() = style_table;

                let session = state.manager.session_mut();
                for (row_idx, row) in frame_store.current_frame().rows.iter().enumerate() {
                    session.frame_store.set_row(row_idx, row.0.as_ref().clone());
                }
                session.frame_store.set_cursor(frame_store.current_frame().cursor);
                session.frame_store.advance_state();
                session.record_state_snapshot();

                let _state_id = session.frame_store.current_state_id();

                let force_snapshot = knobs
                    .force_snapshot_every
                    .map(|n| n > 0 && state.frame_count % n == 0)
                    .unwrap_or(false);

                if force_snapshot {
                    for &remote_id in clients.keys() {
                        state.manager.session_mut().force_client_snapshot(remote_id);
                    }
                }

                let updates: Vec<_> = clients
                    .keys()
                    .filter_map(|&remote_id| {
                        state.manager.session_mut().get_render_update(remote_id).map(|update| {
                            let (msg, is_delta, frame_size) = match update {
                                RenderUpdate::Snapshot(snapshot) => {
                                    let size = snapshot.encoded_len();
                                    (
                                        StreamEnvelope {
                                            msg: Some(stream_envelope::Msg::ScreenSnapshot(snapshot)),
                                        },
                                        false,
                                        size,
                                    )
                                },
                                RenderUpdate::Delta(delta) => {
                                    let size = delta.encoded_len();
                                    state.delta_count = state.delta_count.wrapping_add(1);
                                    (
                                        StreamEnvelope {
                                            msg: Some(stream_envelope::Msg::ScreenDeltaStream(delta)),
                                        },
                                        true,
                                        size,
                                    )
                                },
                            };
                            (remote_id, msg, is_delta, frame_size)
                        })
                    })
                    .collect();

                (updates, knobs.delay_send_ms)
            };
            // Lock released here

            if let Some(ms) = delay_ms {
                tokio::time::sleep(tokio::time::Duration::from_millis(ms)).await;
            }

            // M1: Send to each client's channel (non-blocking)
            let mut clients_to_remove = Vec::new();
            let client_count = clients.len();

            for (remote_id, msg, is_delta, frame_size) in updates_to_send {
                let should_drop = if is_delta {
                    knobs.drop_delta_nth.map(|n| {
                        if n > 0 {
                            let mut state_guard = shared_state.blocking_write();
                            let should_drop = state_guard.delta_count.is_multiple_of(n);
                            if should_drop {
                                state_guard.dropped_delta_count = state_guard.dropped_delta_count.wrapping_add(1);
                            }
                            should_drop
                        } else {
                            false
                        }
                    }).unwrap_or(false)
                } else {
                    false
                };

                if knobs.log_frame_stats {
                    log::info!(
                        "[FRAME_STATS] type={} size={} clients={} dropped={} drop_nth={:?} delay_ms={:?}",
                        if is_delta { "delta" } else { "snapshot" },
                        frame_size,
                        client_count,
                        should_drop,
                        knobs.drop_delta_nth,
                        knobs.delay_send_ms,
                    );
                }

                if should_drop {
                    log::debug!("Test knob: dropping delta for client {}", remote_id);
                    continue;
                }

                if let Some(client) = clients.get(&remote_id) {
                    if let Err(mpsc::error::TrySendError::Full(_)) = client.sender.try_send(msg) {
                        log::warn!(
                            "Client {} channel full, dropping frame (backpressure)",
                            remote_id
                        );
                    } else if client.sender.is_closed() {
                        clients_to_remove.push(remote_id);
                    }
                }
            }

            for remote_id in clients_to_remove {
                clients.remove(&remote_id);
                let mut state = shared_state.write().await;
                state.manager.session_mut().remove_client(remote_id);
                log::info!("Removed client {} due to closed channel", remote_id);
            }

            log::trace!("Frame ready: clients={}", clients.len());
        }
        RemoteInstruction::ClientResize { client_id, size } => {
            let mut state = shared_state.write().await;
            state.manager.resize(size.cols, size.rows);
            log::debug!("Client {} resized: {}x{}", client_id, size.cols, size.rows);
        }
        RemoteInstruction::ClientConnected { client_id, size } => {
            let mut state = shared_state.write().await;
            state.active_zellij_client = Some(client_id);
            log::info!("Zellij client {} connected: {}x{}", client_id, size.cols, size.rows);
        }
        RemoteInstruction::ClientDisconnected { client_id } => {
            let mut state = shared_state.write().await;
            if state.active_zellij_client == Some(client_id) {
                state.active_zellij_client = None;
            }
            log::info!("Zellij client {} disconnected", client_id);
        }
        RemoteInstruction::Shutdown => {
            return Ok(true);
        }
    }
    Ok(false)
}

struct ClientGuard {
    remote_id: u64,
    shared_state: Arc<RwLock<SharedState>>,
    conn_event_tx: mpsc::Sender<ConnectionEvent>,
    disarmed: bool,
}

impl ClientGuard {
    fn new(
        remote_id: u64,
        shared_state: Arc<RwLock<SharedState>>,
        conn_event_tx: mpsc::Sender<ConnectionEvent>,
    ) -> Self {
        Self {
            remote_id,
            shared_state,
            conn_event_tx,
            disarmed: false,
        }
    }

    fn disarm(&mut self) {
        self.disarmed = true;
    }
}

impl Drop for ClientGuard {
    fn drop(&mut self) {
        if self.disarmed {
            return;
        }
        let remote_id = self.remote_id;
        let shared_state = self.shared_state.clone();
        let conn_event_tx = self.conn_event_tx.clone();
        tokio::spawn(async move {
            {
                let mut state = shared_state.write().await;
                state.manager.session_mut().remove_client(remote_id);
                log::info!("ClientGuard cleanup: removed client {}", remote_id);
            }
            if let Err(e) = conn_event_tx.send(ConnectionEvent::ClientDisconnected { remote_id }).await {
                log::warn!("Failed to send ClientDisconnected during guard cleanup: {}", e);
            }
        });
    }
}

async fn handle_connection(
    connection: wtransport::Connection,
    shared_state: Arc<RwLock<SharedState>>,
    conn_event_tx: mpsc::Sender<ConnectionEvent>,
    expected_token: Option<Vec<u8>>,
) -> Result<()> {
    let (mut send, mut recv) = connection.accept_bi().await?;
    let remote_id = REMOTE_CLIENT_ID_COUNTER.fetch_add(1, Ordering::Relaxed);

    let client_hello = read_client_hello(&mut recv).await?;
    log::info!(
        "Received ClientHello from {} (remote_id={})",
        client_hello.client_name,
        remote_id
    );

    if let Some(ref expected) = expected_token {
        if client_hello.bearer_token != *expected {
            log::warn!(
                "Authentication failed for remote client {} ({}): invalid bearer token",
                remote_id,
                client_hello.client_name
            );
            anyhow::bail!("authentication failed: invalid bearer token");
        }
        log::debug!("Remote client {} authenticated successfully", remote_id);
    }

    let mut guard = ClientGuard::new(remote_id, shared_state.clone(), conn_event_tx.clone());

    {
        let mut state = shared_state.write().await;
        state.manager.session_mut().add_client(remote_id, 4);

        let session = state.manager.session_mut();
        let lease = session.lease_manager.request_control(
            remote_id,
            Some(DisplaySize { cols: 80, rows: 24 }),
            false,
        );

        let lease_info = match lease {
            LeaseResult::Granted(l) => Some(l),
            LeaseResult::Denied { .. } => session.lease_manager.get_current_lease(),
        };

        let resume_token = session.generate_resume_token(remote_id);
        let session_name = state.session_name.clone();

        let server_hello = build_server_hello(&client_hello, remote_id, lease_info, resume_token, &session_name);
        let encoded = encode_envelope(&StreamEnvelope {
            msg: Some(stream_envelope::Msg::ServerHello(server_hello)),
        })?;
        send.write_all(&encoded).await?;
        log::info!("Sent ServerHello to remote client {}", remote_id);

        if let Some(RenderUpdate::Snapshot(snapshot)) = state.manager.session_mut().get_render_update(remote_id) {
            let encoded = encode_envelope(&StreamEnvelope {
                msg: Some(stream_envelope::Msg::ScreenSnapshot(snapshot)),
            })?;
            send.write_all(&encoded).await?;
            log::info!("Sent initial ScreenSnapshot to remote client {}", remote_id);
        }
    }

    guard.disarm();

    conn_event_tx.send(ConnectionEvent::ClientConnected {
        remote_id,
        send,
    }).await?;

    let mut buffer = BytesMut::new();
    loop {
        let mut chunk = [0u8; 4096];
        match recv.read(&mut chunk).await? {
            Some(0) | None => {
                log::info!("Remote client {} stream closed", remote_id);
                break;
            }
            Some(n) => {
                buffer.extend_from_slice(&chunk[..n]);

                while let Some(envelope) = decode_envelope(&mut buffer)? {
                    match envelope.msg {
                        Some(stream_envelope::Msg::InputEvent(input)) => {
                            conn_event_tx.send(ConnectionEvent::InputReceived {
                                remote_id,
                                input,
                            }).await?;
                        }
                        Some(stream_envelope::Msg::RequestControl(req)) => {
                            conn_event_tx.send(ConnectionEvent::RequestControl {
                                remote_id,
                                request: req,
                            }).await?;
                        }

                        _ => {
                            log::debug!("Unhandled message from client {}", remote_id);
                        }
                    }
                }
            }
        }
    }

    conn_event_tx.send(ConnectionEvent::ClientDisconnected { remote_id }).await?;
    Ok(())
}

/// Spawns a per-client sender task that receives from the channel and writes to the stream (M1)
fn spawn_client_sender_task(
    remote_id: u64,
    mut send_stream: wtransport::SendStream,
    mut receiver: mpsc::Receiver<StreamEnvelope>,
) {
    tokio::spawn(async move {
        while let Some(msg) = receiver.recv().await {
            match encode_envelope(&msg) {
                Ok(encoded) => {
                    if let Err(e) = send_stream.write_all(&encoded).await {
                        log::warn!("Client {} sender task: write failed: {}", remote_id, e);
                        break;
                    }
                }
                Err(e) => {
                    log::error!("Client {} sender task: encode failed: {}", remote_id, e);
                }
            }
        }
        log::debug!("Client {} sender task exiting", remote_id);
    });
}

async fn handle_connection_event(
    shared_state: &Arc<RwLock<SharedState>>,
    clients: &mut HashMap<u64, ClientConnection>,
    event: ConnectionEvent,
) -> Result<()> {
    match event {
        ConnectionEvent::ClientConnected { remote_id, send } => {
            // M1: Create bounded channel and spawn sender task
            let (tx, rx) = mpsc::channel::<StreamEnvelope>(CLIENT_CHANNEL_SIZE);
            spawn_client_sender_task(remote_id, send, rx);
            clients.insert(remote_id, ClientConnection { sender: tx, remote_id });
            log::info!("Remote client {} added to active clients (total: {})", remote_id, clients.len());
        }
        ConnectionEvent::ClientDisconnected { remote_id } => {
            clients.remove(&remote_id);
            let mut state = shared_state.write().await;
            state.manager.session_mut().remove_client(remote_id);
            log::info!("Remote client {} removed (total: {})", remote_id, clients.len());
        }
        ConnectionEvent::InputReceived { remote_id, input } => {
            // M2: Clone data needed, release lock before network I/O
            let (is_controller, process_result, active_zellij_client, to_screen) = {
                let mut state = shared_state.write().await;
                let is_controller = state.manager.session_mut().lease_manager.is_controller(remote_id);
                if !is_controller {
                    (false, None, None, None)
                } else {
                    let result = state.manager.session_mut().process_input(remote_id, &input);
                    (true, Some(result), state.active_zellij_client, Some(state.to_screen.clone()))
                }
            };
            // Lock released here

            if !is_controller {
                log::warn!(
                    "Remote client {} sent input but is not the controller, denying",
                    remote_id
                );

                if let Some(client) = clients.get(&remote_id) {
                    let error = ProtocolError {
                        code: protocol_error::Code::LeaseDenied as i32,
                        message: "Not the controller".to_string(),
                        fatal: false,
                    };
                    let msg = StreamEnvelope {
                        msg: Some(stream_envelope::Msg::ProtocolError(error)),
                    };
                    if let Err(mpsc::error::TrySendError::Full(_)) = client.sender.try_send(msg) {
                        log::warn!("Client {} channel full, dropping error message", remote_id);
                    }
                }
                return Ok(());
            }

            match process_result.unwrap() {
                Ok(ack) => {
                    if let Some(action) = translate_input(&input) {
                        match action {
                            zellij_utils::input::actions::Action::Write {
                                key_with_modifier,
                                bytes,
                                is_kitty_keyboard_protocol,
                            } => {
                                if let Some(zellij_client_id) = active_zellij_client {
                                    if let Some(ref to_screen) = to_screen {
                                        if let Err(e) = to_screen.send(ScreenInstruction::WriteCharacter(
                                            key_with_modifier,
                                            bytes,
                                            is_kitty_keyboard_protocol,
                                            zellij_client_id,
                                            None,
                                        )) {
                                            log::error!(
                                                "Failed to send to screen thread (may have crashed): {}",
                                                e
                                            );
                                        } else {
                                            log::trace!(
                                                "Routed input from remote client {} to zellij client {}",
                                                remote_id,
                                                zellij_client_id
                                            );
                                        }
                                    }
                                } else {
                                    log::warn!(
                                        "No active Zellij client to route input from remote client {}",
                                        remote_id
                                    );
                                }
                            }
                            _ => {
                                log::debug!("Non-write action from remote client {}, ignoring", remote_id);
                            }
                        }
                    }
                    if let Some(client) = clients.get(&remote_id) {
                        let msg = StreamEnvelope {
                            msg: Some(stream_envelope::Msg::InputAck(ack)),
                        };
                        if let Err(mpsc::error::TrySendError::Full(_)) = client.sender.try_send(msg) {
                            log::warn!("Client {} channel full, dropping InputAck", remote_id);
                        }
                    }
                    log::trace!("Input from client {} processed", remote_id);
                }
                Err(e) => {
                    log::warn!("Input error from client {}: {:?}", remote_id, e);
                }
            }
        }
        ConnectionEvent::RequestControl { remote_id, request } => {
            // M2: Clone result before releasing lock
            let response = {
                let mut state = shared_state.write().await;
                let result = state.manager.session_mut().lease_manager.request_control(
                    remote_id,
                    request.desired_size,
                    request.force,
                );

                match result {
                    LeaseResult::Granted(lease) => {
                        log::info!("Granted control to remote client {}", remote_id);
                        stream_envelope::Msg::GrantControl(GrantControl {
                            lease: Some(lease),
                        })
                    }
                    LeaseResult::Denied { reason, current_lease } => {
                        log::info!("Denied control to remote client {}: {}", remote_id, reason);
                        stream_envelope::Msg::DenyControl(DenyControl {
                            reason,
                            lease: current_lease,
                        })
                    }
                }
            };
            // Lock released here

            if let Some(client) = clients.get(&remote_id) {
                let msg = StreamEnvelope { msg: Some(response) };
                if let Err(mpsc::error::TrySendError::Full(_)) = client.sender.try_send(msg) {
                    log::warn!("Client {} channel full, dropping control response", remote_id);
                }
            }
        }
    }
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

    if len > MAX_FRAME_SIZE {
        anyhow::bail!(
            "frame size {} exceeds maximum allowed size {} bytes",
            len,
            MAX_FRAME_SIZE
        );
    }

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
    lease: Option<ControllerLease>,
    resume_token: Vec<u8>,
    session_name: &str,
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
        session_name: session_name.to_string(),
        session_state: SessionState::Running.into(),
        lease,
        resume_token,
        snapshot_interval_ms: 5000,
        max_inflight_inputs: 256,
        render_window: zellij_remote_protocol::DEFAULT_RENDER_WINDOW,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_remote_config_default() {
        let (to_screen, _) = zellij_utils::channels::bounded(1);
        let config = RemoteConfig {
            listen_addr: "127.0.0.1:4433".parse().unwrap(),
            session_name: "zellij".to_string(),
            initial_size: Size { cols: 80, rows: 24 },
            to_screen: zellij_utils::channels::SenderWithContext::new(to_screen),
            bearer_token: None,
        };
        assert_eq!(config.listen_addr.port(), 4433);
        assert_eq!(config.session_name, "zellij");
        assert_eq!(config.initial_size.cols, 80);
        assert_eq!(config.initial_size.rows, 24);
        assert!(config.bearer_token.is_none());
    }

    #[test]
    fn test_decode_envelope_rejects_oversized_frame() {
        let mut buf = bytes::BytesMut::new();
        buf.extend_from_slice(&[0x80, 0x80, 0x80, 0x08]); // varint encoding of 16MB (exceeds MAX_FRAME_SIZE)
        let result = decode_envelope(&mut buf);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("exceeds maximum allowed size"));
    }
}
