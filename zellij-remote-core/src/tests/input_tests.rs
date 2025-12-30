use crate::input::{AckResult, InputProcessResult, InputReceiver, InputSender};
use crate::lease::{Duration, TestClock};
use zellij_remote_protocol::InputEvent;

fn make_input(seq: u64, client_time_ms: u32) -> InputEvent {
    InputEvent {
        input_seq: seq,
        client_time_ms,
        payload: None,
    }
}

#[test]
fn test_sequential_input_processed() {
    let mut receiver = InputReceiver::new();

    let result1 = receiver.process_input(&make_input(1, 100));
    assert_eq!(result1, InputProcessResult::Processed);
    assert_eq!(receiver.last_acked_seq(), 1);

    let result2 = receiver.process_input(&make_input(2, 200));
    assert_eq!(result2, InputProcessResult::Processed);
    assert_eq!(receiver.last_acked_seq(), 2);

    let result3 = receiver.process_input(&make_input(3, 300));
    assert_eq!(result3, InputProcessResult::Processed);
    assert_eq!(receiver.last_acked_seq(), 3);
}

#[test]
fn test_duplicate_input_ignored() {
    let mut receiver = InputReceiver::new();

    receiver.process_input(&make_input(1, 100));
    receiver.process_input(&make_input(2, 200));

    let result = receiver.process_input(&make_input(1, 100));
    assert_eq!(result, InputProcessResult::Duplicate);
    assert_eq!(receiver.last_acked_seq(), 2);

    let result2 = receiver.process_input(&make_input(2, 200));
    assert_eq!(result2, InputProcessResult::Duplicate);
    assert_eq!(receiver.last_acked_seq(), 2);
}

#[test]
fn test_out_of_order_handled() {
    let mut receiver = InputReceiver::new();

    receiver.process_input(&make_input(1, 100));

    let result = receiver.process_input(&make_input(3, 300));
    assert_eq!(
        result,
        InputProcessResult::OutOfOrder {
            expected: 2,
            received: 3
        }
    );
    assert_eq!(receiver.last_acked_seq(), 1);

    let result_zero = receiver.process_input(&make_input(0, 0));
    assert!(matches!(result_zero, InputProcessResult::OutOfOrder { .. }));
}

#[test]
fn test_cumulative_ack_semantics() {
    let mut receiver = InputReceiver::new();

    receiver.process_input(&make_input(1, 100));
    let ack1 = receiver.generate_ack();
    assert_eq!(ack1.acked_seq, 1);
    assert_eq!(ack1.rtt_sample_seq, 1);
    assert_eq!(ack1.echoed_client_time_ms, 100);

    receiver.process_input(&make_input(2, 200));
    receiver.process_input(&make_input(3, 300));

    let ack2 = receiver.generate_ack();
    assert_eq!(ack2.acked_seq, 3);
    assert_eq!(ack2.rtt_sample_seq, 3);
    assert_eq!(ack2.echoed_client_time_ms, 300);

    let ack3 = receiver.generate_ack();
    assert_eq!(ack3.acked_seq, 3);
    assert_eq!(ack3.rtt_sample_seq, 0);
    assert_eq!(ack3.echoed_client_time_ms, 0);
}

#[test]
fn test_inflight_window_limits() {
    TestClock::reset();

    let mut sender = InputSender::new(3);

    assert!(sender.can_send());
    assert_eq!(sender.inflight_count(), 0);

    sender.mark_sent(1, 100);
    assert!(sender.can_send());
    assert_eq!(sender.inflight_count(), 1);

    sender.mark_sent(2, 200);
    assert!(sender.can_send());
    assert_eq!(sender.inflight_count(), 2);

    sender.mark_sent(3, 300);
    assert!(!sender.can_send());
    assert_eq!(sender.inflight_count(), 3);

    assert!(!sender.can_send());
    assert_eq!(sender.next_seq(), 4);
}

#[test]
fn test_ack_clears_inflight() {
    use zellij_remote_protocol::InputAck;

    TestClock::reset();

    let mut sender = InputSender::new(5);

    sender.mark_sent(1, 100);
    sender.mark_sent(2, 200);
    sender.mark_sent(3, 300);
    assert_eq!(sender.inflight_count(), 3);

    TestClock::advance(Duration::from_millis(50));

    let ack = InputAck {
        acked_seq: 2,
        rtt_sample_seq: 2,
        echoed_client_time_ms: 200,
    };

    let result = sender.process_ack(&ack);
    assert_eq!(sender.inflight_count(), 1);

    match result {
        AckResult::Ok { rtt_sample } => {
            let sample = rtt_sample.unwrap();
            assert_eq!(sample.seq, 2);
            assert_eq!(sample.rtt_ms, 50);
        },
        _ => panic!("Expected Ok result"),
    }

    let ack_all = InputAck {
        acked_seq: 3,
        rtt_sample_seq: 3,
        echoed_client_time_ms: 300,
    };
    sender.process_ack(&ack_all);
    assert_eq!(sender.inflight_count(), 0);
    assert!(sender.can_send());
}

#[test]
fn test_sender_next_seq_increments() {
    TestClock::reset();

    let mut sender = InputSender::new(10);

    assert_eq!(sender.next_seq(), 1);
    sender.mark_sent(1, 100);
    assert_eq!(sender.next_seq(), 2);
    sender.mark_sent(2, 200);
    assert_eq!(sender.next_seq(), 3);
}

#[test]
fn test_ack_without_rtt_sample() {
    use zellij_remote_protocol::InputAck;

    TestClock::reset();

    let mut sender = InputSender::new(5);
    sender.mark_sent(1, 100);
    sender.mark_sent(2, 200);

    let ack = InputAck {
        acked_seq: 2,
        rtt_sample_seq: 0,
        echoed_client_time_ms: 0,
    };

    let result = sender.process_ack(&ack);
    assert_eq!(result, AckResult::Ok { rtt_sample: None });
    assert_eq!(sender.inflight_count(), 0);
}

#[test]
fn test_stale_ack() {
    use zellij_remote_protocol::InputAck;

    TestClock::reset();

    let mut sender = InputSender::new(5);

    let ack = InputAck {
        acked_seq: 0,
        rtt_sample_seq: 0,
        echoed_client_time_ms: 0,
    };

    let result = sender.process_ack(&ack);
    assert_eq!(result, AckResult::Stale);
}
