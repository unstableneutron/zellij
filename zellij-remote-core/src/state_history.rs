use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::frame::FrameData;

const DEFAULT_HISTORY_SIZE: usize = 64;

#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub state_id: u64,
    pub frame: FrameData,
    pub timestamp: Instant,
}

pub struct StateHistory {
    entries: VecDeque<HistoryEntry>,
    max_size: usize,
}

impl StateHistory {
    pub fn new(max_size: usize) -> Self {
        Self {
            entries: VecDeque::with_capacity(max_size),
            max_size,
        }
    }

    pub fn push(&mut self, state_id: u64, frame: FrameData) {
        if self.entries.len() >= self.max_size {
            self.entries.pop_front();
        }
        self.entries.push_back(HistoryEntry {
            state_id,
            frame,
            timestamp: Instant::now(),
        });
    }

    pub fn get(&self, state_id: u64) -> Option<&FrameData> {
        self.entries
            .iter()
            .find(|e| e.state_id == state_id)
            .map(|e| &e.frame)
    }

    pub fn oldest_state_id(&self) -> Option<u64> {
        self.entries.front().map(|e| e.state_id)
    }

    pub fn newest_state_id(&self) -> Option<u64> {
        self.entries.back().map(|e| e.state_id)
    }

    pub fn can_resume_from(&self, state_id: u64) -> bool {
        self.get(state_id).is_some()
    }

    pub fn prune_older_than(&mut self, max_age: Duration) {
        let cutoff = Instant::now() - max_age;
        while let Some(front) = self.entries.front() {
            if front.timestamp < cutoff {
                self.entries.pop_front();
            } else {
                break;
            }
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

impl Default for StateHistory {
    fn default() -> Self {
        Self::new(DEFAULT_HISTORY_SIZE)
    }
}
