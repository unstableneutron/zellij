use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use rand::RngCore;

use crate::client_state::ClientRenderState;
use crate::frame::FrameStore;
use crate::input::{InputProcessResult, InputReceiver};
use crate::lease::LeaseManager;
use crate::resume_token::{ResumeResult, ResumeToken};
use crate::rtt::RttEstimator;
use crate::state_history::StateHistory;
use crate::style_table::StyleTable;
use zellij_remote_protocol::{
    ControllerPolicy, InputAck, InputEvent, ScreenDelta, ScreenSnapshot, StateAck,
};

#[cfg(not(test))]
use std::time::Duration;

#[cfg(test)]
use crate::lease::Duration;

const DEFAULT_LEASE_DURATION_SECS: u64 = 30;
const DEFAULT_HISTORY_SIZE: usize = 64;
const DEFAULT_TOKEN_EXPIRY_MS: u64 = 300_000; // 5 minutes
const DEFAULT_MAX_CLOCK_SKEW_MS: u64 = 30_000; // 30 seconds

static SESSION_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug)]
pub enum RenderUpdate {
    Snapshot(ScreenSnapshot),
    Delta(ScreenDelta),
}

#[derive(Debug, Clone, PartialEq)]
pub enum InputError {
    ClientNotFound,
    NotController,
    OutOfOrder { expected: u64, received: u64 },
    Duplicate,
}

pub struct RemoteSession {
    pub frame_store: FrameStore,
    pub style_table: StyleTable,
    pub lease_manager: LeaseManager,
    pub input_receivers: HashMap<u64, InputReceiver>,
    pub rtt_estimator: RttEstimator,
    pub clients: HashMap<u64, ClientRenderState>,
    pub state_history: StateHistory,
    pub session_id: u64,
    token_expiry_ms: u64,
    max_clock_skew_ms: u64,
    token_secret: [u8; 32],
}

impl RemoteSession {
    pub fn new(cols: usize, rows: usize) -> Self {
        let mut token_secret = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut token_secret);

        Self {
            frame_store: FrameStore::new(cols, rows),
            style_table: StyleTable::new(),
            lease_manager: LeaseManager::new(
                ControllerPolicy::LastWriterWins,
                Duration::from_secs(DEFAULT_LEASE_DURATION_SECS),
            ),
            input_receivers: HashMap::new(),
            rtt_estimator: RttEstimator::new(),
            clients: HashMap::new(),
            state_history: StateHistory::new(DEFAULT_HISTORY_SIZE),
            session_id: SESSION_ID_COUNTER.fetch_add(1, Ordering::Relaxed),
            token_expiry_ms: DEFAULT_TOKEN_EXPIRY_MS,
            max_clock_skew_ms: DEFAULT_MAX_CLOCK_SKEW_MS,
            token_secret,
        }
    }

    pub fn with_session_id(cols: usize, rows: usize, session_id: u64) -> Self {
        let mut session = Self::new(cols, rows);
        session.session_id = session_id;
        session
    }

    #[cfg(test)]
    pub fn with_token_secret(cols: usize, rows: usize, secret: [u8; 32]) -> Self {
        let mut session = Self::new(cols, rows);
        session.token_secret = secret;
        session
    }

    pub fn add_client(&mut self, client_id: u64, window_size: u32) {
        self.clients
            .insert(client_id, ClientRenderState::new(window_size));
        self.input_receivers.insert(client_id, InputReceiver::new());
    }

    pub fn remove_client(&mut self, client_id: u64) {
        self.clients.remove(&client_id);
        self.input_receivers.remove(&client_id);
        self.lease_manager.remove_client(client_id);
    }

    pub fn process_input(
        &mut self,
        client_id: u64,
        input: &InputEvent,
    ) -> Result<InputAck, InputError> {
        if !self.lease_manager.is_controller(client_id) {
            return Err(InputError::NotController);
        }

        let receiver = self
            .input_receivers
            .get_mut(&client_id)
            .ok_or(InputError::ClientNotFound)?;

        match receiver.process_input(input) {
            InputProcessResult::Processed => Ok(receiver.generate_ack()),
            InputProcessResult::Duplicate => Err(InputError::Duplicate),
            InputProcessResult::OutOfOrder { expected, received } => {
                Err(InputError::OutOfOrder { expected, received })
            },
        }
    }

    pub fn process_state_ack(&mut self, client_id: u64, ack: &StateAck) {
        if let Some(client_state) = self.clients.get_mut(&client_id) {
            client_state.process_state_ack(ack);

            if ack.srtt_ms > 0 {
                self.rtt_estimator.record_sample(ack.srtt_ms);
            }

            let pending_state_id = client_state.pending_state_id();
            if ack.last_applied_state_id >= pending_state_id {
                if let Some(pending_frame) = client_state.pending_frame().cloned() {
                    client_state.advance_baseline(ack.last_applied_state_id, pending_frame);
                }
            }
        }
    }

    pub fn get_render_update(&mut self, client_id: u64) -> Option<RenderUpdate> {
        let client_state = self.clients.get_mut(&client_id)?;
        let current_frame = self.frame_store.current_frame();
        let current_state_id = self.frame_store.current_state_id();

        if client_state.should_send_snapshot() {
            let snapshot = client_state.prepare_snapshot(
                current_frame,
                current_state_id,
                &mut self.style_table,
            );
            Some(RenderUpdate::Snapshot(snapshot))
        } else if client_state.can_send() {
            let delta =
                client_state.prepare_delta(current_frame, current_state_id, &mut self.style_table);
            delta.map(RenderUpdate::Delta)
        } else {
            None
        }
    }

    pub fn client_count(&self) -> usize {
        self.clients.len()
    }

    pub fn has_client(&self, client_id: u64) -> bool {
        self.clients.contains_key(&client_id)
    }

    pub fn force_client_snapshot(&mut self, client_id: u64) {
        if let Some(client_state) = self.clients.get_mut(&client_id) {
            client_state.reset_baseline();
        }
    }

    pub fn record_state_snapshot(&mut self) {
        let state_id = self.frame_store.current_state_id();
        let frame = self.frame_store.current_frame().clone();
        self.state_history.push(state_id, frame);
    }

    pub fn generate_resume_token(&self, client_id: u64) -> Vec<u8> {
        let last_applied_state_id = self
            .clients
            .get(&client_id)
            .map(|c| c.baseline_state_id())
            .unwrap_or(0);

        let last_acked_input_seq = self
            .input_receivers
            .get(&client_id)
            .map(|r| r.last_acked_seq())
            .unwrap_or(0);

        let token = ResumeToken::new(
            self.session_id,
            client_id,
            last_applied_state_id,
            last_acked_input_seq,
        );
        token.encode_signed(&self.token_secret)
    }

    pub fn try_resume(&mut self, token_bytes: &[u8], window_size: u32) -> ResumeResult {
        let token = match ResumeToken::decode_signed(token_bytes, &self.token_secret) {
            Some(t) => t,
            None => return ResumeResult::InvalidToken,
        };

        let current_time_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        if !token.is_valid_timestamp(self.token_expiry_ms, current_time_ms, self.max_clock_skew_ms)
        {
            if token.issued_at_ms > current_time_ms + self.max_clock_skew_ms {
                return ResumeResult::FutureDatedToken;
            }
            return ResumeResult::ExpiredToken;
        }

        if token.session_id != self.session_id {
            return ResumeResult::SessionMismatch;
        }

        if self.clients.contains_key(&token.client_id) {
            return ResumeResult::ClientIdInUse;
        }

        if !self
            .state_history
            .can_resume_from(token.last_applied_state_id)
        {
            return ResumeResult::StateNotFound;
        }

        self.clients
            .insert(token.client_id, ClientRenderState::new(window_size));
        self.input_receivers
            .insert(token.client_id, InputReceiver::new_from_seq(token.last_acked_input_seq));

        if let Some(baseline_frame) = self.state_history.get(token.last_applied_state_id) {
            if let Some(client_state) = self.clients.get_mut(&token.client_id) {
                client_state.advance_baseline(token.last_applied_state_id, baseline_frame.clone());
            }
        }

        ResumeResult::Resumed {
            client_id: token.client_id,
            baseline_state_id: token.last_applied_state_id,
        }
    }

    pub fn set_token_expiry(&mut self, expiry_ms: u64) {
        self.token_expiry_ms = expiry_ms;
    }

    pub fn set_max_clock_skew(&mut self, skew_ms: u64) {
        self.max_clock_skew_ms = skew_ms;
    }

    pub fn can_resume_from_state(&self, state_id: u64) -> bool {
        self.state_history.can_resume_from(state_id)
    }

    #[cfg(test)]
    pub fn token_secret(&self) -> &[u8; 32] {
        &self.token_secret
    }
}

impl Default for RemoteSession {
    fn default() -> Self {
        Self::new(80, 24)
    }
}
