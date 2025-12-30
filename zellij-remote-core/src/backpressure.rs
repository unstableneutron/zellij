const DEFAULT_WINDOW_SIZE: u32 = 4;

#[derive(Debug)]
pub struct RenderWindow {
    window_size: u32,
    oldest_unacked_state_id: u64,
    newest_sent_state_id: u64,
}

impl RenderWindow {
    pub fn new(window_size: u32) -> Self {
        Self {
            window_size,
            oldest_unacked_state_id: 0,
            newest_sent_state_id: 0,
        }
    }

    pub fn can_send(&self) -> bool {
        !self.is_window_exhausted()
    }

    pub fn mark_sent(&mut self, state_id: u64) {
        debug_assert!(
            state_id > self.newest_sent_state_id || self.newest_sent_state_id == 0,
            "state_id must be monotonically increasing: {} <= {}",
            state_id,
            self.newest_sent_state_id
        );
        if self.oldest_unacked_state_id == 0 {
            self.oldest_unacked_state_id = state_id;
        }
        if state_id > self.newest_sent_state_id {
            self.newest_sent_state_id = state_id;
        }
    }

    pub fn ack_received(&mut self, state_id: u64) {
        if state_id > self.newest_sent_state_id {
            return;
        }
        if state_id >= self.oldest_unacked_state_id {
            self.oldest_unacked_state_id = state_id + 1;
        }
        if self.oldest_unacked_state_id > self.newest_sent_state_id {
            self.oldest_unacked_state_id = 0;
            self.newest_sent_state_id = 0;
        }
    }

    pub fn oldest_unacked(&self) -> Option<u64> {
        if self.oldest_unacked_state_id == 0 {
            None
        } else {
            Some(self.oldest_unacked_state_id)
        }
    }

    pub fn is_window_exhausted(&self) -> bool {
        if self.oldest_unacked_state_id == 0 {
            return false;
        }
        self.unacked_count() >= self.window_size
    }

    pub fn unacked_count(&self) -> u32 {
        if self.oldest_unacked_state_id == 0 || self.newest_sent_state_id == 0 {
            return 0;
        }
        (self.newest_sent_state_id - self.oldest_unacked_state_id + 1) as u32
    }

    pub fn should_force_snapshot(&self) -> bool {
        self.is_window_exhausted()
    }

    pub fn reset_for_snapshot(&mut self, new_state_id: u64) {
        self.oldest_unacked_state_id = new_state_id;
        self.newest_sent_state_id = new_state_id;
    }

    pub fn window_size(&self) -> u32 {
        self.window_size
    }
}

impl Default for RenderWindow {
    fn default() -> Self {
        Self::new(DEFAULT_WINDOW_SIZE)
    }
}
