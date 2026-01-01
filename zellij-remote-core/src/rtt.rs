const DEFAULT_ALPHA: f64 = 0.125;
const DEFAULT_BETA: f64 = 0.25;
const DEFAULT_INITIAL_RTO_MS: u32 = 1000;

const MIN_RTO_STABLE_MS: u32 = 50;
const MIN_RTO_NORMAL_MS: u32 = 100;
const MIN_RTO_DEGRADED_MS: u32 = 200;
const MAX_RTO_MS: u32 = 60000;

const VARIANCE_STABLE_THRESHOLD: f64 = 0.20;
const VARIANCE_DEGRADED_THRESHOLD: f64 = 0.50;
const LOSS_STABLE_THRESHOLD: f64 = 0.015; // 1.5%
const LOSS_DEGRADED_THRESHOLD: f64 = 0.06; // 6%

const MIN_SRTT_FOR_RATIO_MS: f64 = 25.0;
const ALPHA_FAST: f64 = 0.25;
const FAST_CONVERGENCE_RATIO: f64 = 0.85;

// Hysteresis durations
const HYSTERESIS_STABLE_ENTER_MS: u64 = 1000;
const HYSTERESIS_DEGRADED_ENTER_MS: u64 = 500;
const HYSTERESIS_RECOVER_MS: u64 = 2000;

const SAMPLE_WINDOW_SIZE: u64 = 128;

const MIN_ELAPSED_MS: u64 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LinkState {
    Stable,
    #[default]
    Normal,
    Degraded,
}

#[derive(Debug, Clone)]
pub struct RttEstimator {
    srtt_ms: Option<f64>,
    rttvar_ms: f64,
    alpha: f64,
    beta: f64,
    sample_count: u64,
    loss_count: u64,
    current_state: LinkState,
    state_candidate: LinkState,
    candidate_since_ms: u64,
    monotonic_time_ms: u64,
}

impl RttEstimator {
    pub fn new() -> Self {
        Self {
            srtt_ms: None,
            rttvar_ms: 0.0,
            alpha: DEFAULT_ALPHA,
            beta: DEFAULT_BETA,
            sample_count: 0,
            loss_count: 0,
            current_state: LinkState::Normal,
            state_candidate: LinkState::Normal,
            candidate_since_ms: 0,
            monotonic_time_ms: 0,
        }
    }

    pub fn record_packet(&mut self, rtt_ms: Option<u32>) {
        if self.sample_count >= SAMPLE_WINDOW_SIZE {
            self.sample_count = 0;
            self.loss_count = 0;
        }
        self.sample_count += 1;

        match rtt_ms {
            Some(rtt) => {
                let rtt_f = rtt as f64;

                match self.srtt_ms {
                    None => {
                        self.srtt_ms = Some(rtt_f);
                        self.rttvar_ms = rtt_f / 2.0;
                    },
                    Some(srtt) => {
                        let alpha = if rtt_f < FAST_CONVERGENCE_RATIO * srtt
                            || rtt_f < srtt - 2.0 * self.rttvar_ms
                        {
                            ALPHA_FAST
                        } else {
                            self.alpha
                        };

                        let new_rttvar =
                            (1.0 - self.beta) * self.rttvar_ms + self.beta * (srtt - rtt_f).abs();
                        let new_srtt = (1.0 - alpha) * srtt + alpha * rtt_f;

                        self.rttvar_ms = new_rttvar;
                        self.srtt_ms = Some(new_srtt);
                    },
                }

                let elapsed_ms = (rtt as u64).max(MIN_ELAPSED_MS);
                self.update_state(elapsed_ms);
            },
            None => {
                self.loss_count += 1;
                self.update_state(MIN_ELAPSED_MS);
            },
        }
    }

    pub fn record_sample(&mut self, rtt_ms: u32) {
        self.record_packet(Some(rtt_ms));
    }

    pub fn record_loss(&mut self) {
        self.record_packet(None);
    }

    pub fn loss_rate(&self) -> f64 {
        if self.sample_count == 0 {
            return 0.0;
        }
        self.loss_count as f64 / self.sample_count as f64
    }

    pub fn variance_ratio(&self) -> f64 {
        match self.srtt_ms {
            None => 0.0,
            Some(srtt) => self.rttvar_ms / srtt.max(MIN_SRTT_FOR_RATIO_MS),
        }
    }

    fn target_state(&self) -> LinkState {
        let vr = self.variance_ratio();
        let lr = self.loss_rate();

        if vr < VARIANCE_STABLE_THRESHOLD && lr < LOSS_STABLE_THRESHOLD {
            LinkState::Stable
        } else if vr > VARIANCE_DEGRADED_THRESHOLD || lr > LOSS_DEGRADED_THRESHOLD {
            LinkState::Degraded
        } else {
            LinkState::Normal
        }
    }

    fn update_state(&mut self, elapsed_ms: u64) {
        self.monotonic_time_ms += elapsed_ms;
        let target = self.target_state();

        if target == self.current_state {
            self.state_candidate = self.current_state;
            self.candidate_since_ms = self.monotonic_time_ms;
            return;
        }

        if target != self.state_candidate {
            self.state_candidate = target;
            self.candidate_since_ms = self.monotonic_time_ms;
            return;
        }

        let duration = self
            .monotonic_time_ms
            .saturating_sub(self.candidate_since_ms);
        let required = match (self.current_state, target) {
            (LinkState::Degraded, _) => HYSTERESIS_RECOVER_MS,
            (_, LinkState::Stable) => HYSTERESIS_STABLE_ENTER_MS,
            (_, LinkState::Degraded) => HYSTERESIS_DEGRADED_ENTER_MS,
            _ => HYSTERESIS_STABLE_ENTER_MS,
        };

        if duration >= required {
            self.current_state = target;
            self.candidate_since_ms = self.monotonic_time_ms;
        }
    }

    pub fn adaptive_floor(&self) -> u32 {
        match self.current_state {
            LinkState::Stable => MIN_RTO_STABLE_MS,
            LinkState::Normal => MIN_RTO_NORMAL_MS,
            LinkState::Degraded => MIN_RTO_DEGRADED_MS,
        }
    }

    pub fn link_state(&self) -> LinkState {
        self.current_state
    }

    pub fn srtt_ms(&self) -> Option<u32> {
        self.srtt_ms.map(|s| s.round() as u32)
    }

    pub fn rto_ms(&self) -> u32 {
        match self.srtt_ms {
            None => DEFAULT_INITIAL_RTO_MS,
            Some(srtt) => {
                let rto = srtt + 4.0 * self.rttvar_ms;
                (rto.round() as u32).clamp(self.adaptive_floor(), MAX_RTO_MS)
            },
        }
    }

    pub fn rttvar_ms(&self) -> f64 {
        self.rttvar_ms
    }
}

impl Default for RttEstimator {
    fn default() -> Self {
        Self::new()
    }
}
