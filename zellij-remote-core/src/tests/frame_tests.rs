use crate::frame::{Cell, FrameStore, Row};
use std::sync::Arc;

#[test]
fn test_row_arc_sharing() {
    let row1 = Row::new(80);
    let row2 = row1.clone();

    // Same Arc pointer = instant equality check
    assert!(Arc::ptr_eq(&row1.0, &row2.0));
}

#[test]
fn test_row_modification_clones() {
    let row1 = Row::new(80);
    let mut row2 = row1.clone();

    // Modify row2
    row2.set_cell(
        0,
        Cell {
            codepoint: 'A' as u32,
            width: 1,
            style_id: 0,
        },
    );

    // Now they should be different Arcs
    assert!(!Arc::ptr_eq(&row1.0, &row2.0));
}

#[test]
fn test_frame_store_baseline_tracking() {
    let mut store = FrameStore::new(80, 24);

    // Initial state
    let frame1 = store.snapshot();
    let state_id_1 = store.current_state_id();

    // Modify a row
    store.update_row(0, |row| {
        row.set_cell(
            0,
            Cell {
                codepoint: 'X' as u32,
                width: 1,
                style_id: 0,
            },
        );
    });
    store.advance_state();

    let state_id_2 = store.current_state_id();
    assert!(state_id_2 > state_id_1);

    // Unchanged rows should still share Arc
    let frame2 = store.snapshot();
    assert!(Arc::ptr_eq(&frame1.data.rows[1].0, &frame2.data.rows[1].0));
    assert!(!Arc::ptr_eq(&frame1.data.rows[0].0, &frame2.data.rows[0].0));
}

#[test]
fn test_dirty_row_tracking() {
    let mut store = FrameStore::new(80, 24);

    store.update_row(5, |row| {
        row.set_cell(
            0,
            Cell {
                codepoint: 'A' as u32,
                width: 1,
                style_id: 0,
            },
        );
    });
    store.update_row(10, |row| {
        row.set_cell(
            0,
            Cell {
                codepoint: 'B' as u32,
                width: 1,
                style_id: 0,
            },
        );
    });

    let dirty = store.take_dirty_rows();
    assert!(dirty.contains(&5));
    assert!(dirty.contains(&10));
    assert_eq!(dirty.len(), 2);

    // After take, should be empty
    let dirty2 = store.take_dirty_rows();
    assert!(dirty2.is_empty());
}

// Resize edge cases

#[test]
fn test_resize_to_zero_rows() {
    let mut store = FrameStore::new(80, 24);
    store.resize(80, 0);
    assert_eq!(store.current_frame().rows.len(), 0);
}

#[test]
fn test_resize_to_zero_cols() {
    let mut store = FrameStore::new(80, 24);
    store.resize(0, 24);
    assert_eq!(store.current_frame().cols, 0);
    for row in &store.current_frame().rows {
        assert_eq!(row.cols(), 0);
    }
}

#[test]
fn test_resize_expand_rows() {
    let mut store = FrameStore::new(80, 10);
    store.resize(80, 20);
    assert_eq!(store.current_frame().rows.len(), 20);
    // New rows should be default-initialized
    for i in 10..20 {
        assert_eq!(
            store.current_frame().rows[i].get_cell(0).unwrap().codepoint,
            ' ' as u32
        );
    }
}

#[test]
fn test_resize_shrink_rows() {
    let mut store = FrameStore::new(80, 24);
    store.update_row(20, |row| {
        row.set_cell(
            0,
            Cell {
                codepoint: 'X' as u32,
                width: 1,
                style_id: 0,
            },
        );
    });
    store.resize(80, 10);
    assert_eq!(store.current_frame().rows.len(), 10);
    // Row 20 should be gone
}

#[test]
fn test_resize_marks_all_rows_dirty() {
    let mut store = FrameStore::new(80, 10);
    store.take_dirty_rows(); // Clear
    store.resize(80, 10); // Same size
    let dirty = store.take_dirty_rows();
    assert_eq!(dirty.len(), 10);
}

// Out-of-bounds behavior

#[test]
fn test_set_cell_out_of_bounds_ignored() {
    let mut row = Row::new(10);
    row.set_cell(
        100,
        Cell {
            codepoint: 'X' as u32,
            width: 1,
            style_id: 0,
        },
    );
    // Should not panic, cell at 100 doesn't exist
    assert!(row.get_cell(100).is_none());
}

#[test]
fn test_update_row_out_of_bounds_ignored() {
    let mut store = FrameStore::new(80, 10);
    store.update_row(100, |row| {
        row.set_cell(
            0,
            Cell {
                codepoint: 'X' as u32,
                width: 1,
                style_id: 0,
            },
        );
    });
    // Should not panic, dirty rows should not include 100
    let dirty = store.take_dirty_rows();
    assert!(!dirty.contains(&100));
}

#[test]
fn test_get_cell_out_of_bounds_returns_none() {
    let row = Row::new(10);
    assert!(row.get_cell(10).is_none());
    assert!(row.get_cell(100).is_none());
}
