use crate::backpressure::RenderWindow;
use crate::delta::DeltaEngine;
use crate::frame::FrameData;
use crate::style_table::StyleTable;
use zellij_remote_protocol::{ScreenDelta, ScreenSnapshot, StateAck};

#[derive(Debug)]
pub struct ClientRenderState {
    render_window: RenderWindow,
    acked_baseline: Option<FrameData>,
    acked_baseline_state_id: u64,
    pending_frame: Option<FrameData>,
    pending_state_id: u64,
}

impl ClientRenderState {
    pub fn new(window_size: u32) -> Self {
        Self {
            render_window: RenderWindow::new(window_size),
            acked_baseline: None,
            acked_baseline_state_id: 0,
            pending_frame: None,
            pending_state_id: 0,
        }
    }

    pub fn process_state_ack(&mut self, ack: &StateAck) {
        self.render_window.ack_received(ack.last_applied_state_id);
    }

    pub fn advance_baseline(&mut self, acked_state_id: u64, acked_frame: FrameData) {
        if acked_state_id >= self.acked_baseline_state_id || self.acked_baseline.is_none() {
            self.acked_baseline = Some(acked_frame);
            self.acked_baseline_state_id = acked_state_id;
        }
    }

    pub fn should_send_snapshot(&self) -> bool {
        self.acked_baseline.is_none() || self.render_window.should_force_snapshot()
    }

    pub fn can_send(&self) -> bool {
        self.render_window.can_send()
    }

    pub fn prepare_delta(
        &mut self,
        current_frame: &FrameData,
        current_state_id: u64,
        style_table: &mut StyleTable,
    ) -> Option<ScreenDelta> {
        let baseline = self.acked_baseline.as_ref()?;

        if !self.render_window.can_send() {
            return None;
        }

        let delta = DeltaEngine::compute_delta(
            baseline,
            current_frame,
            style_table,
            self.acked_baseline_state_id,
            current_state_id,
        );

        self.render_window.mark_sent(current_state_id);
        self.pending_frame = Some(current_frame.clone());
        self.pending_state_id = current_state_id;

        Some(delta)
    }

    pub fn prepare_snapshot(
        &mut self,
        current_frame: &FrameData,
        current_state_id: u64,
        style_table: &mut StyleTable,
    ) -> ScreenSnapshot {
        let snapshot = DeltaEngine::compute_snapshot(current_frame, style_table, current_state_id);

        self.render_window.reset_for_snapshot(current_state_id);
        self.acked_baseline = Some(current_frame.clone());
        self.acked_baseline_state_id = current_state_id;
        self.pending_frame = Some(current_frame.clone());
        self.pending_state_id = current_state_id;

        snapshot
    }

    pub fn pending_frame(&self) -> Option<&FrameData> {
        self.pending_frame.as_ref()
    }

    pub fn pending_state_id(&self) -> u64 {
        self.pending_state_id
    }

    pub fn render_window(&self) -> &RenderWindow {
        &self.render_window
    }

    pub fn render_window_mut(&mut self) -> &mut RenderWindow {
        &mut self.render_window
    }

    pub fn baseline_state_id(&self) -> u64 {
        self.acked_baseline_state_id
    }

    pub fn has_baseline(&self) -> bool {
        self.acked_baseline.is_some()
    }
}

impl Default for ClientRenderState {
    fn default() -> Self {
        Self::new(4)
    }
}
