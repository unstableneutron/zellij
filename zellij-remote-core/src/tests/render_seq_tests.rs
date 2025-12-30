use crate::render_seq::{DatagramDecision, RenderSender, RenderSeqTracker};

#[test]
fn test_newer_seq_accepted() {
    let mut tracker = RenderSeqTracker::new();

    assert!(tracker.should_apply(0, 1)); // baseline 0, seq 1
    tracker.mark_applied(1);

    assert!(tracker.should_apply(0, 2)); // same baseline, newer seq
}

#[test]
fn test_stale_seq_rejected() {
    let mut tracker = RenderSeqTracker::new();

    tracker.mark_applied(5);

    // Older sequence should be rejected
    assert!(!tracker.should_apply(0, 3));
}

#[test]
fn test_wrong_baseline_rejected() {
    let mut tracker = RenderSeqTracker::new();
    tracker.set_baseline(10);

    // Delta based on wrong baseline
    assert!(!tracker.should_apply(5, 1));
}

#[test]
fn test_reset_after_snapshot() {
    let mut tracker = RenderSeqTracker::new();
    tracker.mark_applied(100);

    // After snapshot, sequence resets
    tracker.reset_for_snapshot(50);

    assert_eq!(tracker.current_baseline_id(), 50);
    assert!(tracker.should_apply(50, 1)); // New sequence 1 should work
}

#[test]
fn test_datagram_vs_stream_decision() {
    let tracker = RenderSeqTracker::new();

    // Small payload -> datagram
    let small_payload = vec![0u8; 500];
    assert!(matches!(
        tracker.decide_transport(&small_payload, 1200, true),
        DatagramDecision::Datagram
    ));

    // Large payload -> stream
    let large_payload = vec![0u8; 2000];
    assert!(matches!(
        tracker.decide_transport(&large_payload, 1200, true),
        DatagramDecision::Stream
    ));

    // Datagrams not supported -> stream
    assert!(matches!(
        tracker.decide_transport(&small_payload, 1200, false),
        DatagramDecision::Stream
    ));
}

#[test]
fn test_render_sender_sequence() {
    let mut sender = RenderSender::new();

    assert_eq!(sender.next_seq(), 1);
    assert_eq!(sender.next_seq(), 2);
    assert_eq!(sender.next_seq(), 3);

    sender.reset();
    assert_eq!(sender.next_seq(), 1);
}

#[test]
fn test_equal_seq_rejected() {
    let mut tracker = RenderSeqTracker::new();

    tracker.mark_applied(5);

    // Equal sequence (duplicate) should be rejected
    assert!(!tracker.should_apply(0, 5));
}
