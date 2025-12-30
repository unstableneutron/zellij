use crate::lease::{Duration, LeaseEvent, LeaseManager, LeaseResult, TestClock};
use zellij_remote_protocol::{ControllerPolicy, DisplaySize};

fn setup() {
    TestClock::reset();
}

#[test]
fn test_initial_request_granted() {
    setup();
    let mut mgr = LeaseManager::new(ControllerPolicy::ExplicitOnly, Duration::from_secs(60));

    let result = mgr.request_control(
        1,
        Some(DisplaySize {
            cols: 120,
            rows: 40,
        }),
        false,
    );

    match result {
        LeaseResult::Granted(lease) => {
            assert_eq!(lease.owner_client_id, 1);
            assert_eq!(lease.lease_id, 1);
            let size = lease.current_size.unwrap();
            assert_eq!(size.cols, 120);
            assert_eq!(size.rows, 40);
        },
        _ => panic!("Expected Granted, got {:?}", result),
    }

    assert!(mgr.is_controller(1));
}

#[test]
fn test_second_client_denied() {
    setup();
    let mut mgr = LeaseManager::new(ControllerPolicy::ExplicitOnly, Duration::from_secs(60));

    let _ = mgr.request_control(1, Some(DisplaySize { cols: 80, rows: 24 }), false);

    let result = mgr.request_control(
        2,
        Some(DisplaySize {
            cols: 100,
            rows: 30,
        }),
        false,
    );

    match result {
        LeaseResult::Denied {
            reason,
            current_lease,
        } => {
            assert!(reason.contains("client 1"));
            assert!(current_lease.is_some());
            let lease = current_lease.unwrap();
            assert_eq!(lease.owner_client_id, 1);
        },
        _ => panic!("Expected Denied, got {:?}", result),
    }

    assert!(mgr.is_controller(1));
    assert!(!mgr.is_controller(2));
}

#[test]
fn test_last_writer_wins_takeover() {
    setup();
    let mut mgr = LeaseManager::new(ControllerPolicy::LastWriterWins, Duration::from_secs(60));

    let result1 = mgr.request_control(1, Some(DisplaySize { cols: 80, rows: 24 }), false);
    assert!(matches!(result1, LeaseResult::Granted(_)));

    let result2 = mgr.request_control(
        2,
        Some(DisplaySize {
            cols: 100,
            rows: 30,
        }),
        false,
    );

    match result2 {
        LeaseResult::Granted(lease) => {
            assert_eq!(lease.owner_client_id, 2);
            assert_eq!(lease.lease_id, 2);
            assert_eq!(lease.current_size.unwrap().cols, 100);
        },
        _ => panic!("Expected Granted for takeover, got {:?}", result2),
    }

    assert!(!mgr.is_controller(1));
    assert!(mgr.is_controller(2));
    assert!(mgr.is_viewer(1));
}

#[test]
fn test_keepalive_extends_lease() {
    setup();
    let mut mgr = LeaseManager::new(ControllerPolicy::ExplicitOnly, Duration::from_secs(60));

    let result = mgr.request_control(1, None, false);
    let lease_id = match result {
        LeaseResult::Granted(lease) => lease.lease_id,
        _ => panic!("Expected Granted"),
    };

    TestClock::advance(Duration::from_secs(30));

    assert!(mgr.keepalive(1, lease_id));

    TestClock::advance(Duration::from_secs(40));

    let event = mgr.tick();
    assert!(event.is_none(), "Lease should not expire after keepalive");

    assert!(mgr.is_controller(1));
}

#[test]
fn test_lease_expires_without_keepalive() {
    setup();
    let mut mgr = LeaseManager::new(ControllerPolicy::ExplicitOnly, Duration::from_secs(60));

    let result = mgr.request_control(1, None, false);
    let lease_id = match result {
        LeaseResult::Granted(lease) => lease.lease_id,
        _ => panic!("Expected Granted"),
    };

    TestClock::advance(Duration::from_secs(61));

    let event = mgr.tick();
    match event {
        Some(LeaseEvent::Expired {
            lease_id: id,
            owner,
        }) => {
            assert_eq!(id, lease_id);
            assert_eq!(owner, 1);
        },
        _ => panic!("Expected Expired event, got {:?}", event),
    }

    assert!(!mgr.is_controller(1));
}

#[test]
fn test_release_frees_lease() {
    setup();
    let mut mgr = LeaseManager::new(ControllerPolicy::ExplicitOnly, Duration::from_secs(60));

    let result = mgr.request_control(1, None, false);
    let lease_id = match result {
        LeaseResult::Granted(lease) => lease.lease_id,
        _ => panic!("Expected Granted"),
    };

    assert!(mgr.release_control(1, lease_id));
    assert!(!mgr.is_controller(1));

    let result2 = mgr.request_control(2, None, false);
    match result2 {
        LeaseResult::Granted(lease) => {
            assert_eq!(lease.owner_client_id, 2);
        },
        _ => panic!("Expected second client to get lease after release"),
    }
}

#[test]
fn test_size_change_by_controller() {
    setup();
    let mut mgr = LeaseManager::new(ControllerPolicy::ExplicitOnly, Duration::from_secs(60));

    let result = mgr.request_control(1, Some(DisplaySize { cols: 80, rows: 24 }), false);
    let lease_id = match result {
        LeaseResult::Granted(lease) => lease.lease_id,
        _ => panic!("Expected Granted"),
    };

    assert!(mgr.set_size(
        1,
        lease_id,
        DisplaySize {
            cols: 120,
            rows: 40
        }
    ));

    let size = mgr.current_size().unwrap();
    assert_eq!(size.cols, 120);
    assert_eq!(size.rows, 40);
}

#[test]
fn test_size_change_by_non_controller_rejected() {
    setup();
    let mut mgr = LeaseManager::new(ControllerPolicy::ExplicitOnly, Duration::from_secs(60));

    let result = mgr.request_control(1, Some(DisplaySize { cols: 80, rows: 24 }), false);
    let lease_id = match result {
        LeaseResult::Granted(lease) => lease.lease_id,
        _ => panic!("Expected Granted"),
    };

    assert!(!mgr.set_size(
        2,
        lease_id,
        DisplaySize {
            cols: 120,
            rows: 40
        }
    ));
    assert!(!mgr.set_size(
        1,
        lease_id + 1,
        DisplaySize {
            cols: 120,
            rows: 40
        }
    ));

    let size = mgr.current_size().unwrap();
    assert_eq!(size.cols, 80);
    assert_eq!(size.rows, 24);
}

#[test]
fn test_viewer_mode_receives_updates() {
    setup();
    let mut mgr = LeaseManager::new(ControllerPolicy::ExplicitOnly, Duration::from_secs(60));

    let _ = mgr.request_control(1, Some(DisplaySize { cols: 80, rows: 24 }), false);

    mgr.add_viewer(2);
    mgr.add_viewer(3);

    assert!(mgr.is_controller(1));
    assert!(mgr.is_viewer(2));
    assert!(mgr.is_viewer(3));
    assert!(!mgr.is_viewer(1));

    assert_eq!(mgr.viewer_count(), 2);

    let size = mgr.current_size();
    assert!(size.is_some());
}

#[test]
fn test_remove_controller_frees_lease() {
    setup();
    let mut mgr = LeaseManager::new(ControllerPolicy::ExplicitOnly, Duration::from_secs(60));

    let result = mgr.request_control(1, None, false);
    let lease_id = match result {
        LeaseResult::Granted(lease) => lease.lease_id,
        _ => panic!("Expected Granted"),
    };

    let event = mgr.remove_client(1);
    match event {
        Some(LeaseEvent::Revoked {
            lease_id: id,
            owner,
            reason,
        }) => {
            assert_eq!(id, lease_id);
            assert_eq!(owner, 1);
            assert_eq!(reason, "disconnect");
        },
        _ => panic!("Expected Revoked event"),
    }

    assert!(!mgr.is_controller(1));
}

#[test]
fn test_remove_viewer_no_event() {
    setup();
    let mut mgr = LeaseManager::new(ControllerPolicy::ExplicitOnly, Duration::from_secs(60));

    let _ = mgr.request_control(1, None, false);
    mgr.add_viewer(2);

    let event = mgr.remove_client(2);
    assert!(event.is_none());
    assert!(!mgr.is_viewer(2));
    assert!(mgr.is_controller(1));
}

#[test]
fn test_force_takeover_explicit_only() {
    setup();
    let mut mgr = LeaseManager::new(ControllerPolicy::ExplicitOnly, Duration::from_secs(60));

    let _ = mgr.request_control(1, None, false);

    let result = mgr.request_control(2, None, true);

    match result {
        LeaseResult::Granted(lease) => {
            assert_eq!(lease.owner_client_id, 2);
        },
        _ => panic!("Expected force takeover to succeed"),
    }
}

#[test]
fn test_keepalive_wrong_lease_id_fails() {
    setup();
    let mut mgr = LeaseManager::new(ControllerPolicy::ExplicitOnly, Duration::from_secs(60));

    let _ = mgr.request_control(1, None, false);

    assert!(!mgr.keepalive(1, 999));
    assert!(!mgr.keepalive(2, 1));
}

#[test]
fn test_release_wrong_credentials_fails() {
    setup();
    let mut mgr = LeaseManager::new(ControllerPolicy::ExplicitOnly, Duration::from_secs(60));

    let result = mgr.request_control(1, None, false);
    let lease_id = match result {
        LeaseResult::Granted(lease) => lease.lease_id,
        _ => panic!("Expected Granted"),
    };

    assert!(!mgr.release_control(2, lease_id));
    assert!(!mgr.release_control(1, lease_id + 1));
    assert!(mgr.is_controller(1));
}

#[test]
fn test_get_current_lease() {
    setup();
    let mut mgr = LeaseManager::new(ControllerPolicy::ExplicitOnly, Duration::from_secs(60));

    assert!(mgr.get_current_lease().is_none());

    let _ = mgr.request_control(1, Some(DisplaySize { cols: 80, rows: 24 }), false);

    let lease = mgr.get_current_lease().unwrap();
    assert_eq!(lease.owner_client_id, 1);
    assert_eq!(lease.lease_id, 1);
    assert!(lease.remaining_ms <= 60000);
}

#[test]
fn test_same_client_re_request_returns_existing() {
    setup();
    let mut mgr = LeaseManager::new(ControllerPolicy::ExplicitOnly, Duration::from_secs(60));

    let result1 = mgr.request_control(1, None, false);
    let lease_id = match result1 {
        LeaseResult::Granted(lease) => lease.lease_id,
        _ => panic!("Expected Granted"),
    };

    let result2 = mgr.request_control(1, None, false);
    match result2 {
        LeaseResult::Granted(lease) => {
            assert_eq!(lease.lease_id, lease_id);
            assert_eq!(lease.owner_client_id, 1);
        },
        _ => panic!("Expected same lease returned"),
    }
}
