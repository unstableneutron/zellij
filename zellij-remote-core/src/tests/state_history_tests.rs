use crate::frame::FrameData;
use crate::state_history::StateHistory;
use std::time::Duration;

fn make_frame(cols: usize, rows: usize) -> FrameData {
    FrameData::new(cols, rows)
}

#[test]
fn test_push_and_get() {
    let mut history = StateHistory::new(10);

    let frame1 = make_frame(80, 24);
    let frame2 = make_frame(80, 24);

    history.push(1, frame1.clone());
    history.push(2, frame2.clone());

    assert!(history.get(1).is_some());
    assert!(history.get(2).is_some());
    assert!(history.get(3).is_none());
}

#[test]
fn test_history_size_limit() {
    let mut history = StateHistory::new(3);

    for i in 1..=5 {
        history.push(i, make_frame(80, 24));
    }

    assert_eq!(history.len(), 3);
    assert!(history.get(1).is_none());
    assert!(history.get(2).is_none());
    assert!(history.get(3).is_some());
    assert!(history.get(4).is_some());
    assert!(history.get(5).is_some());
}

#[test]
fn test_oldest_newest_state_id() {
    let mut history = StateHistory::new(10);

    assert!(history.oldest_state_id().is_none());
    assert!(history.newest_state_id().is_none());

    history.push(5, make_frame(80, 24));
    history.push(10, make_frame(80, 24));
    history.push(15, make_frame(80, 24));

    assert_eq!(history.oldest_state_id(), Some(5));
    assert_eq!(history.newest_state_id(), Some(15));
}

#[test]
fn test_can_resume_from() {
    let mut history = StateHistory::new(10);

    history.push(100, make_frame(80, 24));
    history.push(200, make_frame(80, 24));

    assert!(history.can_resume_from(100));
    assert!(history.can_resume_from(200));
    assert!(!history.can_resume_from(50));
    assert!(!history.can_resume_from(300));
}

#[test]
fn test_prune_older_than() {
    let mut history = StateHistory::new(10);

    history.push(1, make_frame(80, 24));
    history.push(2, make_frame(80, 24));

    std::thread::sleep(Duration::from_millis(50));

    history.push(3, make_frame(80, 24));

    history.prune_older_than(Duration::from_millis(25));

    assert!(!history.can_resume_from(1));
    assert!(!history.can_resume_from(2));
    assert!(history.can_resume_from(3));
}

#[test]
fn test_is_empty_and_clear() {
    let mut history = StateHistory::new(10);

    assert!(history.is_empty());
    assert_eq!(history.len(), 0);

    history.push(1, make_frame(80, 24));

    assert!(!history.is_empty());
    assert_eq!(history.len(), 1);

    history.clear();

    assert!(history.is_empty());
    assert_eq!(history.len(), 0);
}

#[test]
fn test_default() {
    let history = StateHistory::default();
    assert!(history.is_empty());
}
