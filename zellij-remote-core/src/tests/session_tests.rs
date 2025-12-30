use crate::frame::FrameData;
use crate::resume_token::{ResumeResult, ResumeToken};
use crate::session::{InputError, RemoteSession};
use zellij_remote_protocol::{DisplaySize, InputEvent, StateAck};

fn make_input(seq: u64, client_time_ms: u32) -> InputEvent {
    InputEvent {
        input_seq: seq,
        client_time_ms,
        payload: None,
    }
}

#[test]
fn test_input_rejected_from_non_controller() {
    let mut session = RemoteSession::new(80, 24);

    session.add_client(1, 4);
    session.add_client(2, 4);

    session
        .lease_manager
        .request_control(1, Some(DisplaySize { cols: 80, rows: 24 }), false);

    let result = session.process_input(2, &make_input(1, 100));
    assert_eq!(result, Err(InputError::NotController));

    let result = session.process_input(1, &make_input(1, 100));
    assert!(result.is_ok());
}

#[test]
fn test_delta_only_uses_acked_baseline() {
    use crate::client_state::ClientRenderState;
    use crate::style_table::StyleTable;

    let mut state = ClientRenderState::new(4);
    let mut style_table = StyleTable::new();
    let frame1 = FrameData::new(80, 24);
    let frame2 = FrameData::new(80, 24);
    let frame3 = FrameData::new(80, 24);

    let _ = state.prepare_snapshot(&frame1, 1, &mut style_table);

    let delta1 = state.prepare_delta(&frame2, 2, &mut style_table);
    assert!(delta1.is_some());
    let delta1 = delta1.unwrap();
    assert_eq!(delta1.base_state_id, 1);
    assert_eq!(delta1.state_id, 2);

    let delta2 = state.prepare_delta(&frame3, 3, &mut style_table);
    assert!(delta2.is_some());
    let delta2 = delta2.unwrap();
    assert_eq!(delta2.base_state_id, 1);
    assert_eq!(delta2.state_id, 3);

    let ack = StateAck {
        last_applied_state_id: 2,
        last_received_state_id: 2,
        client_time_ms: 0,
        estimated_loss_ppm: 0,
        srtt_ms: 0,
    };
    state.process_state_ack(&ack);
    state.advance_baseline(2, frame2.clone());

    let delta3 = state.prepare_delta(&frame3, 4, &mut style_table);
    assert!(delta3.is_some());
    let delta3 = delta3.unwrap();
    assert_eq!(delta3.base_state_id, 2);
    assert_eq!(delta3.state_id, 4);
}

#[test]
fn test_ack_beyond_newest_ignored() {
    use crate::backpressure::RenderWindow;

    let mut window = RenderWindow::new(4);

    window.mark_sent(1);
    window.mark_sent(2);
    window.mark_sent(3);
    assert_eq!(window.unacked_count(), 3);

    window.ack_received(100);
    assert_eq!(window.unacked_count(), 3);
    assert_eq!(window.oldest_unacked(), Some(1));

    window.ack_received(2);
    assert_eq!(window.unacked_count(), 1);
    assert_eq!(window.oldest_unacked(), Some(3));
}

#[test]
fn test_process_state_ack_records_rtt() {
    let mut session = RemoteSession::new(80, 24);

    session.add_client(1, 4);

    let _ = session.get_render_update(1);

    assert!(session.rtt_estimator.srtt_ms().is_none());

    let ack = StateAck {
        last_applied_state_id: 1,
        last_received_state_id: 1,
        client_time_ms: 100,
        estimated_loss_ppm: 0,
        srtt_ms: 50,
    };

    session.process_state_ack(1, &ack);

    assert_eq!(session.rtt_estimator.srtt_ms(), Some(50));
}

#[test]
fn test_per_client_input_receivers() {
    let mut session = RemoteSession::new(80, 24);

    session.add_client(1, 4);
    session.add_client(2, 4);

    session
        .lease_manager
        .request_control(1, Some(DisplaySize { cols: 80, rows: 24 }), false);

    let result1 = session.process_input(1, &make_input(1, 100));
    assert!(result1.is_ok());
    let ack1 = result1.unwrap();
    assert_eq!(ack1.acked_seq, 1);

    session
        .lease_manager
        .request_control(2, Some(DisplaySize { cols: 80, rows: 24 }), true);

    let result2 = session.process_input(2, &make_input(1, 200));
    assert!(result2.is_ok());
    let ack2 = result2.unwrap();
    assert_eq!(ack2.acked_seq, 1);
}

#[test]
fn test_resume_token_generation_and_validation() {
    let mut session = RemoteSession::with_session_id(80, 24, 42);

    session.add_client(1, 4);

    session.frame_store.advance_state();
    session.record_state_snapshot();

    let _ = session.get_render_update(1);

    let token_bytes = session.generate_resume_token(1);
    assert!(!token_bytes.is_empty());

    let token = ResumeToken::decode_signed(&token_bytes, session.token_secret())
        .expect("token should decode");
    assert_eq!(token.session_id, 42);
    assert_eq!(token.client_id, 1);
}

#[test]
fn test_resume_with_valid_token() {
    let mut session = RemoteSession::with_session_id(80, 24, 42);

    session.add_client(1, 4);
    session.frame_store.advance_state();
    session.record_state_snapshot();

    let _ = session.get_render_update(1);

    let token_bytes = session.generate_resume_token(1);

    session.remove_client(1);
    assert!(!session.has_client(1));

    let result = session.try_resume(&token_bytes, 4);
    assert!(matches!(
        result,
        ResumeResult::Resumed {
            client_id: 1,
            ..
        }
    ));
    assert!(session.has_client(1));
}

#[test]
fn test_resume_with_invalid_token() {
    let mut session = RemoteSession::with_session_id(80, 24, 42);

    let result = session.try_resume(&[0u8; 10], 4);
    assert!(matches!(result, ResumeResult::InvalidToken));
}

#[test]
fn test_resume_with_session_mismatch() {
    let mut session = RemoteSession::with_session_id(80, 24, 42);

    session.add_client(1, 4);
    session.frame_store.advance_state();
    session.record_state_snapshot();

    let token = ResumeToken::new(99, 1, 1, 0);
    let token_bytes = token.encode_signed(session.token_secret());

    let result = session.try_resume(&token_bytes, 4);
    assert!(matches!(result, ResumeResult::SessionMismatch));
}

#[test]
fn test_resume_with_state_not_found() {
    let mut session = RemoteSession::with_session_id(80, 24, 42);

    session.add_client(1, 4);
    session.frame_store.advance_state();
    session.record_state_snapshot();

    session.remove_client(1);

    let token = ResumeToken::new(42, 1, 999, 0);
    let token_bytes = token.encode_signed(session.token_secret());

    let result = session.try_resume(&token_bytes, 4);
    assert!(matches!(result, ResumeResult::StateNotFound));
}

#[test]
fn test_resume_with_client_id_in_use() {
    let mut session = RemoteSession::with_session_id(80, 24, 42);

    session.add_client(1, 4);
    session.frame_store.advance_state();
    session.record_state_snapshot();

    let _ = session.get_render_update(1);

    let token_bytes = session.generate_resume_token(1);

    let result = session.try_resume(&token_bytes, 4);
    assert!(matches!(result, ResumeResult::ClientIdInUse));
}

#[test]
fn test_resumed_client_gets_delta_not_snapshot() {
    let mut session = RemoteSession::with_session_id(80, 24, 42);

    session.add_client(1, 4);
    session.frame_store.advance_state();
    session.record_state_snapshot();

    let _ = session.get_render_update(1);
    let token_bytes = session.generate_resume_token(1);

    session.remove_client(1);

    session.frame_store.advance_state();
    session.record_state_snapshot();

    let result = session.try_resume(&token_bytes, 4);
    assert!(matches!(result, ResumeResult::Resumed { .. }));

    let update = session.get_render_update(1);
    assert!(matches!(update, Some(crate::session::RenderUpdate::Delta(_))));
}

#[test]
fn test_resume_restores_input_seq() {
    let mut session = RemoteSession::with_session_id(80, 24, 42);

    session.add_client(1, 4);
    session
        .lease_manager
        .request_control(1, Some(DisplaySize { cols: 80, rows: 24 }), false);

    for seq in 1..=5 {
        let _ = session.process_input(1, &make_input(seq, 100));
    }

    session.frame_store.advance_state();
    session.record_state_snapshot();
    let _ = session.get_render_update(1);

    let token_bytes = session.generate_resume_token(1);
    session.remove_client(1);

    let result = session.try_resume(&token_bytes, 4);
    assert!(matches!(result, ResumeResult::Resumed { .. }));

    session
        .lease_manager
        .request_control(1, Some(DisplaySize { cols: 80, rows: 24 }), false);

    let result = session.process_input(1, &make_input(6, 100));
    assert!(result.is_ok());

    let result = session.process_input(1, &make_input(5, 100));
    assert!(matches!(result, Err(InputError::Duplicate)));
}
