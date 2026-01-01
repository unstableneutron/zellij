use crate::rtt::{LinkState, RttEstimator};

#[test]
fn test_initial_sample_sets_srtt() {
    let mut estimator = RttEstimator::new();

    assert_eq!(estimator.srtt_ms(), None);

    estimator.record_sample(100);

    assert_eq!(estimator.srtt_ms(), Some(100));
    assert!((estimator.rttvar_ms() - 50.0).abs() < 0.001);
}

#[test]
fn test_subsequent_samples_smooth() {
    let mut estimator = RttEstimator::new();

    estimator.record_sample(100);
    assert_eq!(estimator.srtt_ms(), Some(100));

    estimator.record_sample(120);
    let srtt_after_second = estimator.srtt_ms().unwrap();
    assert!(srtt_after_second > 100 && srtt_after_second < 120);

    estimator.record_sample(80);
    let srtt_after_third = estimator.srtt_ms().unwrap();
    assert!(srtt_after_third > 80 && srtt_after_third < srtt_after_second);
}

#[test]
fn test_rto_calculation() {
    let mut estimator = RttEstimator::new();

    assert_eq!(estimator.rto_ms(), 1000);

    estimator.record_sample(100);

    let rto = estimator.rto_ms();
    assert_eq!(rto, 300);
}

#[test]
fn test_high_variance_increases_rto() {
    let mut estimator = RttEstimator::new();

    estimator.record_sample(100);
    let initial_rto = estimator.rto_ms();

    estimator.record_sample(200);
    let rto_after_high = estimator.rto_ms();

    assert!(rto_after_high > initial_rto);

    estimator.record_sample(50);
    let rto_after_low = estimator.rto_ms();
    assert!(rto_after_low > initial_rto);
}

#[test]
fn test_minimum_rto() {
    let mut estimator = RttEstimator::new();

    // With low RTT, we'll eventually reach Stable state (50ms floor)
    // Need enough time to reach stable: 1000ms hysteresis
    // With 10ms RTT clamped to 10ms elapsed, need >100 samples (>1000ms)
    for _ in 0..110 {
        estimator.record_sample(10);
    }

    // Should be stable now with 50ms floor
    assert_eq!(estimator.link_state(), LinkState::Stable);
    assert!(estimator.rto_ms() >= 50);
}

#[test]
fn test_smoothing_converges() {
    let mut estimator = RttEstimator::new();

    for _ in 0..50 {
        estimator.record_sample(100);
    }

    assert_eq!(estimator.srtt_ms(), Some(100));
    assert!(estimator.rttvar_ms() < 1.0);
}

#[test]
fn test_ewma_formula_correctness() {
    let mut estimator = RttEstimator::new();

    estimator.record_sample(100);
    assert_eq!(estimator.srtt_ms(), Some(100));
    assert!((estimator.rttvar_ms() - 50.0).abs() < 0.001);

    estimator.record_sample(140);

    let expected_srtt: f64 = 0.875 * 100.0 + 0.125 * 140.0;
    assert_eq!(estimator.srtt_ms(), Some(expected_srtt.round() as u32));

    let expected_rttvar: f64 = 0.75 * 50.0 + 0.25 * 40.0;
    assert!((estimator.rttvar_ms() - expected_rttvar).abs() < 0.001);
}

#[test]
fn test_initial_state_is_normal() {
    let estimator = RttEstimator::new();
    assert_eq!(estimator.link_state(), LinkState::Normal);
    assert_eq!(estimator.adaptive_floor(), 100);
}

#[test]
fn test_stable_link_lowers_floor() {
    let mut estimator = RttEstimator::new();

    // Simulate stable link: consistent RTT, no loss
    // Need enough samples and time for hysteresis (1000ms for stable)
    for _ in 0..50 {
        estimator.record_sample(100); // Consistent 100ms RTT
    }

    // After sufficient stable samples, should transition to Stable
    assert_eq!(estimator.link_state(), LinkState::Stable);
    assert_eq!(estimator.adaptive_floor(), 50);
}

#[test]
fn test_degraded_link_raises_floor() {
    let mut estimator = RttEstimator::new();

    // Start with some baseline
    estimator.record_sample(100);

    // Simulate high variance (degraded link)
    for i in 0..20 {
        let rtt = if i % 2 == 0 { 50 } else { 300 }; // High jitter
        estimator.record_sample(rtt);
    }

    // Should detect degraded state
    assert_eq!(estimator.link_state(), LinkState::Degraded);
    assert_eq!(estimator.adaptive_floor(), 200);
}

#[test]
fn test_loss_tracking() {
    let mut estimator = RttEstimator::new();

    // Record some samples
    for _ in 0..10 {
        estimator.record_sample(100);
    }

    // Record losses
    estimator.record_loss();
    estimator.record_loss();

    // Loss rate should be 2/12 (10 samples + 2 losses = 12 total packets)
    let loss_rate = estimator.loss_rate();
    assert!((loss_rate - 2.0 / 12.0).abs() < 0.01);
}

#[test]
fn test_high_loss_triggers_degraded() {
    let mut estimator = RttEstimator::new();

    // Record samples with high loss rate (>6%)
    // Window resets at 128, so we need to work within window
    // Need sustained high loss and enough time for degraded hysteresis (500ms)
    for _ in 0..50 {
        estimator.record_sample(100); // 50 * 100ms = 5000ms elapsed
    }
    for _ in 0..10 {
        estimator.record_loss(); // 10 losses, sample_count now 60, loss_count 10
    }

    // 60 packets total, loss_rate = 10/60 = 16.7% > 6%
    // We have 5000ms elapsed, more than 500ms needed for degraded
    // Add a few more samples to ensure state evaluation
    for _ in 0..5 {
        estimator.record_sample(100);
    }

    // loss_rate = 10/65 ≈ 15.4% > 6%, should be degraded
    assert_eq!(estimator.link_state(), LinkState::Degraded);
}

#[test]
fn test_variance_ratio_calculation() {
    let mut estimator = RttEstimator::new();

    // No samples yet
    assert_eq!(estimator.variance_ratio(), 0.0);

    // First sample: srtt=100, rttvar=50
    estimator.record_sample(100);
    // variance_ratio = 50 / 100 = 0.5
    assert!((estimator.variance_ratio() - 0.5).abs() < 0.01);
}

#[test]
fn test_variance_ratio_floor() {
    let mut estimator = RttEstimator::new();

    // Very low RTT (10ms) - should use MIN_SRTT_FOR_RATIO_MS (25ms) as floor
    estimator.record_sample(10);
    // rttvar = 10/2 = 5, srtt = 10, but floor is 25
    // variance_ratio = 5 / 25 = 0.2
    assert!((estimator.variance_ratio() - 0.2).abs() < 0.01);
}

#[test]
fn test_fast_convergence_on_improvement() {
    let mut estimator = RttEstimator::new();

    // Establish baseline at 200ms
    for _ in 0..10 {
        estimator.record_sample(200);
    }
    let srtt_before = estimator.srtt_ms().unwrap();

    // Sudden improvement to 100ms (< 0.85 * 200 = 170)
    estimator.record_sample(100);
    let srtt_after = estimator.srtt_ms().unwrap();

    // With fast alpha (0.25), should converge faster than normal alpha (0.125)
    // Normal: 0.875 * 200 + 0.125 * 100 = 187.5
    // Fast:   0.75 * 200 + 0.25 * 100 = 175
    assert!(srtt_after < 180); // Fast convergence should get us lower
    let _ = srtt_before; // silence unused warning
}

#[test]
fn test_sample_window_reset() {
    let mut estimator = RttEstimator::new();

    // Record samples and losses
    for _ in 0..64 {
        estimator.record_sample(100);
    }
    estimator.record_loss();
    estimator.record_loss();

    // Before window reset, loss_rate should be ~3%
    assert!(estimator.loss_rate() > 0.02);

    // Fill up to window size (128) to trigger reset
    for _ in 0..70 {
        estimator.record_sample(100);
    }

    // After reset, loss count should be 0
    assert_eq!(estimator.loss_rate(), 0.0);
}

#[test]
fn test_rto_uses_adaptive_floor() {
    let mut estimator = RttEstimator::new();

    // Very low RTT that would normally give RTO below floor
    // Need >1000ms for stable: with 10ms elapsed, need >100 samples
    for _ in 0..110 {
        estimator.record_sample(10);
    }

    // Should be stable now with 50ms floor
    assert_eq!(estimator.link_state(), LinkState::Stable);

    // RTO should be at least 50ms (stable floor)
    assert!(estimator.rto_ms() >= 50);
}

#[test]
fn test_hysteresis_prevents_oscillation() {
    let mut estimator = RttEstimator::new();

    // Establish stable state
    for _ in 0..50 {
        estimator.record_sample(100);
    }
    assert_eq!(estimator.link_state(), LinkState::Stable);

    // Single bad sample shouldn't immediately change state
    estimator.record_sample(500);
    assert_eq!(estimator.link_state(), LinkState::Stable); // Still stable due to hysteresis
}

#[test]
fn test_rto_clamped_to_max() {
    let mut estimator = RttEstimator::new();

    // Very high RTT
    estimator.record_sample(50000);
    estimator.record_sample(55000);

    // RTO should be clamped to MAX_RTO_MS (60000)
    assert!(estimator.rto_ms() <= 60000);
}

// === New tests for Oracle-identified coverage gaps ===

#[test]
fn test_loss_rate_cannot_exceed_one() {
    let mut estimator = RttEstimator::new();

    // Record many losses then one sample
    for _ in 0..10 {
        estimator.record_loss();
    }
    estimator.record_sample(100);

    // Loss rate should be 10/11, not > 1.0
    let loss_rate = estimator.loss_rate();
    assert!(loss_rate <= 1.0);
    assert!((loss_rate - 10.0 / 11.0).abs() < 0.02);
}

#[test]
fn test_window_boundary_correctness() {
    let mut estimator = RttEstimator::new();

    // Fill exactly to window size (128)
    for _ in 0..128 {
        estimator.record_sample(100);
    }

    // Window should have reset, next samples start fresh
    estimator.record_loss();
    estimator.record_loss();

    // Now sample_count=2, loss_count=2, loss_rate=1.0
    let loss_rate = estimator.loss_rate();
    assert!((loss_rate - 1.0).abs() < 0.01);
}

#[test]
fn test_degraded_to_stable_uses_recover_hysteresis() {
    let mut estimator = RttEstimator::new();

    // Get into degraded state via high jitter
    for i in 0..30 {
        let rtt = if i % 2 == 0 { 50 } else { 400 };
        estimator.record_sample(rtt);
    }
    assert_eq!(estimator.link_state(), LinkState::Degraded);

    // Stable samples for 1500ms (less than 2000ms recover)
    // Need enough samples to also reduce variance below threshold
    for _ in 0..50 {
        estimator.record_sample(100);
    }

    // After variance stabilizes but not enough time, may still be degraded
    // Add more samples to exceed 2000ms with stable variance
    for _ in 0..30 {
        estimator.record_sample(100);
    }
    // 80 * 100ms = 8000ms total stable samples, variance settled
    assert_eq!(estimator.link_state(), LinkState::Stable);
}

#[test]
fn test_zero_rtt_does_not_get_stuck() {
    let mut estimator = RttEstimator::new();

    // RTT=0 uses MIN_ELAPSED_MS (10ms)
    for _ in 0..150 {
        estimator.record_sample(0);
    }

    // 150 * 10ms = 1500ms > 1000ms for stable entry
    assert_eq!(estimator.link_state(), LinkState::Stable);
}

#[test]
fn test_low_rtt_hysteresis_timing() {
    let mut estimator = RttEstimator::new();

    // With 5ms RTT (clamped to 10ms elapsed), need >100 samples for >1000ms
    for _ in 0..110 {
        estimator.record_sample(5);
    }

    assert_eq!(estimator.link_state(), LinkState::Stable);
}

#[test]
fn test_record_packet_api() {
    let mut estimator = RttEstimator::new();

    // Test record_packet with Some (delivered packet)
    estimator.record_packet(Some(100));
    assert_eq!(estimator.srtt_ms(), Some(100));

    // Test record_packet with None (lost packet)
    estimator.record_packet(None);
    assert!(estimator.loss_rate() > 0.0);
}
