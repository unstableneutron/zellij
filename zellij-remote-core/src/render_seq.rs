/// Tracks render sequence for latest-wins datagram semantics (client-side)
#[derive(Debug)]
pub struct RenderSeqTracker {
    last_applied_seq: u64,
    current_baseline_id: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatagramDecision {
    Datagram,
    Stream,
}

impl RenderSeqTracker {
    pub fn new() -> Self {
        Self {
            last_applied_seq: 0,
            current_baseline_id: 0,
        }
    }

    /// Check if a render update should be applied (latest-wins)
    pub fn should_apply(&self, baseline_id: u64, render_seq: u64) -> bool {
        // Reject if based on wrong baseline (need snapshot)
        if baseline_id != self.current_baseline_id {
            return false;
        }

        // Reject if stale or duplicate (already have this or newer)
        if render_seq <= self.last_applied_seq {
            return false;
        }

        true
    }

    /// Mark a render sequence as applied
    pub fn mark_applied(&mut self, render_seq: u64) {
        if render_seq > self.last_applied_seq {
            self.last_applied_seq = render_seq;
        }
    }

    /// Set baseline (after receiving snapshot)
    pub fn set_baseline(&mut self, baseline_id: u64) {
        self.current_baseline_id = baseline_id;
    }

    /// Reset after snapshot (new baseline established)
    pub fn reset_for_snapshot(&mut self, new_baseline_id: u64) {
        self.current_baseline_id = new_baseline_id;
        self.last_applied_seq = 0;
    }

    pub fn last_applied_seq(&self) -> u64 {
        self.last_applied_seq
    }

    pub fn current_baseline_id(&self) -> u64 {
        self.current_baseline_id
    }

    /// Decide whether to send via datagram or stream
    pub fn decide_transport(
        &self,
        encoded_payload: &[u8],
        max_datagram_bytes: u32,
        supports_datagrams: bool,
    ) -> DatagramDecision {
        if !supports_datagrams {
            return DatagramDecision::Stream;
        }

        if encoded_payload.len() <= max_datagram_bytes as usize {
            DatagramDecision::Datagram
        } else {
            DatagramDecision::Stream
        }
    }
}

impl Default for RenderSeqTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Server-side render sender that assigns monotonic sequence numbers
#[derive(Debug)]
pub struct RenderSender {
    next_render_seq: u64,
}

impl RenderSender {
    pub fn new() -> Self {
        Self { next_render_seq: 1 }
    }

    /// Get next render sequence number (and increment)
    pub fn next_seq(&mut self) -> u64 {
        let seq = self.next_render_seq;
        self.next_render_seq += 1;
        seq
    }

    /// Current sequence (without incrementing)
    pub fn current_seq(&self) -> u64 {
        self.next_render_seq
    }

    /// Reset sequence (e.g., after baseline change)
    pub fn reset(&mut self) {
        self.next_render_seq = 1;
    }
}

impl Default for RenderSender {
    fn default() -> Self {
        Self::new()
    }
}
