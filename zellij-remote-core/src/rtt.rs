const DEFAULT_ALPHA: f64 = 0.125;
const DEFAULT_BETA: f64 = 0.25;
const MIN_RTO_MS: u32 = 100;
const DEFAULT_INITIAL_RTO_MS: u32 = 1000;

#[derive(Debug, Clone)]
pub struct RttEstimator {
    srtt_ms: Option<f64>,
    rttvar_ms: f64,
    alpha: f64,
    beta: f64,
}

impl RttEstimator {
    pub fn new() -> Self {
        Self {
            srtt_ms: None,
            rttvar_ms: 0.0,
            alpha: DEFAULT_ALPHA,
            beta: DEFAULT_BETA,
        }
    }

    pub fn record_sample(&mut self, rtt_ms: u32) {
        let rtt = rtt_ms as f64;

        match self.srtt_ms {
            None => {
                self.srtt_ms = Some(rtt);
                self.rttvar_ms = rtt / 2.0;
            },
            Some(srtt) => {
                let new_rttvar =
                    (1.0 - self.beta) * self.rttvar_ms + self.beta * (srtt - rtt).abs();
                let new_srtt = (1.0 - self.alpha) * srtt + self.alpha * rtt;

                self.rttvar_ms = new_rttvar;
                self.srtt_ms = Some(new_srtt);
            },
        }
    }

    pub fn srtt_ms(&self) -> Option<u32> {
        self.srtt_ms.map(|s| s.round() as u32)
    }

    pub fn rto_ms(&self) -> u32 {
        match self.srtt_ms {
            None => DEFAULT_INITIAL_RTO_MS,
            Some(srtt) => {
                let rto = srtt + 4.0 * self.rttvar_ms;
                (rto.round() as u32).max(MIN_RTO_MS)
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
