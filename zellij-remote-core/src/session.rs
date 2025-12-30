use std::collections::HashMap;

use crate::client_state::ClientRenderState;
use crate::frame::FrameStore;
use crate::input::{InputProcessResult, InputReceiver};
use crate::lease::LeaseManager;
use crate::rtt::RttEstimator;
use crate::style_table::StyleTable;
use zellij_remote_protocol::{
    ControllerPolicy, InputAck, InputEvent, ScreenDelta, ScreenSnapshot, StateAck,
};

#[cfg(not(test))]
use std::time::Duration;

#[cfg(test)]
use crate::lease::Duration;

const DEFAULT_LEASE_DURATION_SECS: u64 = 30;

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
}

impl RemoteSession {
    pub fn new(cols: usize, rows: usize) -> Self {
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
        }
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
}

impl Default for RemoteSession {
    fn default() -> Self {
        Self::new(80, 24)
    }
}
