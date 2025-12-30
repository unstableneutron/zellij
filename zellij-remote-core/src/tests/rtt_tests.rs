use crate::rtt::RttEstimator;

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

    for _ in 0..20 {
        estimator.record_sample(10);
    }

    assert!(estimator.rto_ms() >= 100);
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
