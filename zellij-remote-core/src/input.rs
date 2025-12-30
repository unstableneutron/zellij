use std::collections::VecDeque;
use zellij_remote_protocol::{InputAck, InputEvent};

#[cfg(not(test))]
use std::time::Instant;

#[cfg(test)]
use crate::lease::Instant;

#[derive(Debug, Clone, PartialEq)]
pub enum InputProcessResult {
    Processed,
    Duplicate,
    OutOfOrder { expected: u64, received: u64 },
}

#[derive(Debug)]
pub struct InputReceiver {
    last_processed_seq: u64,
    pending_rtt_sample: Option<(u64, u32)>,
}

impl InputReceiver {
    pub fn new() -> Self {
        Self {
            last_processed_seq: 0,
            pending_rtt_sample: None,
        }
    }

    pub fn process_input(&mut self, input: &InputEvent) -> InputProcessResult {
        let seq = input.input_seq;

        if seq == 0 {
            return InputProcessResult::OutOfOrder {
                expected: self.last_processed_seq + 1,
                received: seq,
            };
        }

        if seq <= self.last_processed_seq {
            return InputProcessResult::Duplicate;
        }

        let expected = self.last_processed_seq + 1;
        if seq != expected {
            return InputProcessResult::OutOfOrder {
                expected,
                received: seq,
            };
        }

        self.last_processed_seq = seq;
        self.pending_rtt_sample = Some((seq, input.client_time_ms));

        InputProcessResult::Processed
    }

    pub fn generate_ack(&mut self) -> InputAck {
        let (rtt_sample_seq, echoed_client_time_ms) =
            self.pending_rtt_sample.take().unwrap_or((0, 0));

        InputAck {
            acked_seq: self.last_processed_seq,
            rtt_sample_seq,
            echoed_client_time_ms,
        }
    }

    pub fn last_acked_seq(&self) -> u64 {
        self.last_processed_seq
    }
}

impl Default for InputReceiver {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct InflightInput {
    pub seq: u64,
    pub client_time_ms: u32,
    pub sent_at: Instant,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RttSample {
    pub rtt_ms: u32,
    pub seq: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AckResult {
    Ok { rtt_sample: Option<RttSample> },
    Stale,
}

#[derive(Debug)]
pub struct InputSender {
    next_seq: u64,
    inflight: VecDeque<InflightInput>,
    max_inflight: usize,
}

impl InputSender {
    pub fn new(max_inflight: usize) -> Self {
        Self {
            next_seq: 1,
            inflight: VecDeque::new(),
            max_inflight,
        }
    }

    pub fn can_send(&self) -> bool {
        self.inflight.len() < self.max_inflight
    }

    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    pub fn mark_sent(&mut self, seq: u64, client_time_ms: u32) {
        if seq == self.next_seq {
            self.inflight.push_back(InflightInput {
                seq,
                client_time_ms,
                sent_at: Instant::now(),
            });
            self.next_seq += 1;
        }
    }

    pub fn process_ack(&mut self, ack: &InputAck) -> AckResult {
        if ack.acked_seq == 0 {
            return AckResult::Stale;
        }

        let mut rtt_sample = None;

        while let Some(front) = self.inflight.front() {
            if front.seq <= ack.acked_seq {
                let input = self.inflight.pop_front().unwrap();

                if input.seq == ack.rtt_sample_seq
                    && input.client_time_ms == ack.echoed_client_time_ms
                {
                    let elapsed = input.sent_at.elapsed();
                    rtt_sample = Some(RttSample {
                        rtt_ms: elapsed.as_millis() as u32,
                        seq: input.seq,
                    });
                }
            } else {
                break;
            }
        }

        AckResult::Ok { rtt_sample }
    }

    pub fn inflight_count(&self) -> usize {
        self.inflight.len()
    }
}
