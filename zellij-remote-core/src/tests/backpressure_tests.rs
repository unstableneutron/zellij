use crate::backpressure::RenderWindow;
use crate::client_state::ClientRenderState;
use crate::frame::FrameData;
use crate::style_table::StyleTable;
use proptest::prelude::*;
use zellij_remote_protocol::StateAck;

#[test]
fn test_window_tracks_unacked_states() {
    let mut window = RenderWindow::new(4);

    assert_eq!(window.unacked_count(), 0);
    assert!(window.oldest_unacked().is_none());

    window.mark_sent(1);
    assert_eq!(window.unacked_count(), 1);
    assert_eq!(window.oldest_unacked(), Some(1));

    window.mark_sent(2);
    assert_eq!(window.unacked_count(), 2);
    assert_eq!(window.oldest_unacked(), Some(1));

    window.mark_sent(3);
    assert_eq!(window.unacked_count(), 3);
    assert_eq!(window.oldest_unacked(), Some(1));
}

#[test]
fn test_window_exhausted_when_full() {
    let mut window = RenderWindow::new(4);

    assert!(window.can_send());
    assert!(!window.is_window_exhausted());

    window.mark_sent(1);
    assert!(window.can_send());
    window.mark_sent(2);
    assert!(window.can_send());
    window.mark_sent(3);
    assert!(window.can_send());
    window.mark_sent(4);

    assert!(!window.can_send());
    assert!(window.is_window_exhausted());
    assert_eq!(window.unacked_count(), 4);
}

#[test]
fn test_ack_slides_window() {
    let mut window = RenderWindow::new(4);

    window.mark_sent(1);
    window.mark_sent(2);
    window.mark_sent(3);
    assert_eq!(window.unacked_count(), 3);
    assert_eq!(window.oldest_unacked(), Some(1));

    window.ack_received(1);
    assert_eq!(window.unacked_count(), 2);
    assert_eq!(window.oldest_unacked(), Some(2));

    window.ack_received(2);
    assert_eq!(window.unacked_count(), 1);
    assert_eq!(window.oldest_unacked(), Some(3));
}

#[test]
fn test_cumulative_ack_clears_old_states() {
    let mut window = RenderWindow::new(4);

    window.mark_sent(1);
    window.mark_sent(2);
    window.mark_sent(3);
    window.mark_sent(4);
    assert_eq!(window.unacked_count(), 4);

    window.ack_received(3);
    assert_eq!(window.unacked_count(), 1);
    assert_eq!(window.oldest_unacked(), Some(4));

    window.ack_received(4);
    assert_eq!(window.unacked_count(), 0);
    assert!(window.oldest_unacked().is_none());
}

#[test]
fn test_force_snapshot_on_exhaustion() {
    let mut window = RenderWindow::new(2);

    assert!(!window.should_force_snapshot());

    window.mark_sent(1);
    assert!(!window.should_force_snapshot());

    window.mark_sent(2);
    assert!(window.should_force_snapshot());

    window.reset_for_snapshot(10);
    assert_eq!(window.oldest_unacked(), Some(10));
    assert_eq!(window.unacked_count(), 1);
    assert!(!window.should_force_snapshot());
}

#[test]
fn test_default_window_size() {
    let window = RenderWindow::default();
    assert_eq!(window.window_size(), 4);
}

#[test]
fn test_client_state_process_ack() {
    let mut state = ClientRenderState::new(4);
    let mut style_table = StyleTable::new();
    let frame = FrameData::new(80, 24);

    let _ = state.prepare_snapshot(&frame, 1, &mut style_table);

    let ack = StateAck {
        last_applied_state_id: 1,
        last_received_state_id: 1,
        client_time_ms: 0,
        estimated_loss_ppm: 0,
        srtt_ms: 0,
    };

    state.process_state_ack(&ack);
    assert_eq!(state.render_window().unacked_count(), 0);
}

#[test]
fn test_client_state_should_send_snapshot() {
    let state = ClientRenderState::new(4);
    assert!(state.should_send_snapshot());
}

#[test]
fn test_client_state_prepare_snapshot_sets_baseline() {
    let mut state = ClientRenderState::new(4);
    let mut style_table = StyleTable::new();
    let frame = FrameData::new(80, 24);

    assert!(!state.has_baseline());

    let snapshot = state.prepare_snapshot(&frame, 5, &mut style_table);
    assert_eq!(snapshot.state_id, 5);
    assert!(state.has_baseline());
    assert_eq!(state.baseline_state_id(), 5);
}

#[test]
fn test_client_state_prepare_delta_requires_baseline() {
    let mut state = ClientRenderState::new(4);
    let mut style_table = StyleTable::new();
    let frame = FrameData::new(80, 24);

    let delta = state.prepare_delta(&frame, 1, &mut style_table);
    assert!(delta.is_none());
}

#[test]
fn test_client_state_prepare_delta_after_snapshot() {
    let mut state = ClientRenderState::new(4);
    let mut style_table = StyleTable::new();
    let frame1 = FrameData::new(80, 24);
    let frame2 = FrameData::new(80, 24);

    let _ = state.prepare_snapshot(&frame1, 1, &mut style_table);

    let delta = state.prepare_delta(&frame2, 2, &mut style_table);
    assert!(delta.is_some());
    let delta = delta.unwrap();
    assert_eq!(delta.base_state_id, 1);
    assert_eq!(delta.state_id, 2);
}

#[test]
fn test_client_state_blocks_delta_when_exhausted() {
    let mut state = ClientRenderState::new(2);
    let mut style_table = StyleTable::new();
    let frame = FrameData::new(80, 24);

    let _ = state.prepare_snapshot(&frame, 1, &mut style_table);
    let _ = state.prepare_delta(&frame, 2, &mut style_table);

    assert!(!state.can_send());
    let delta = state.prepare_delta(&frame, 3, &mut style_table);
    assert!(delta.is_none());
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn test_proptest_window_invariants(
        window_size in 1u32..=16,
        ops in proptest::collection::vec(
            prop_oneof![
                Just(true).prop_map(|_| (true, 0u64)),
                (1u64..=100).prop_map(|id| (false, id)),
            ],
            0..50
        )
    ) {
        let mut window = RenderWindow::new(window_size);
        let mut max_sent: u64 = 0;

        for (is_send, id) in ops {
            if is_send && window.can_send() {
                let new_id = max_sent + 1;
                max_sent = new_id;
                window.mark_sent(new_id);
            } else if !is_send && max_sent > 0 {
                let ack_id = id.min(max_sent);
                window.ack_received(ack_id);
            }

            if let Some(oldest) = window.oldest_unacked() {
                prop_assert!(oldest > 0, "oldest_unacked should be > 0 when Some");
            }

            if window.unacked_count() >= window_size {
                prop_assert!(window.is_window_exhausted());
                prop_assert!(!window.can_send());
            }

            if window.unacked_count() < window_size {
                prop_assert!(!window.is_window_exhausted());
                prop_assert!(window.can_send());
            }
        }
    }
}
