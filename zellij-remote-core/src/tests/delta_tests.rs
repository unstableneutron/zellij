use crate::delta::DeltaEngine;
use crate::frame::{Cell, Cursor, CursorShape, FrameStore};
use crate::style_table::StyleTable;

#[test]
fn test_delta_detects_changed_rows() {
    let mut store = FrameStore::new(80, 24);
    let baseline = store.snapshot();

    store.update_row(5, |row| {
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

    let current = store.snapshot();
    let mut style_table = StyleTable::new();

    let delta = DeltaEngine::compute_delta(
        &baseline.data,
        &current.data,
        &mut style_table,
        baseline.state_id,
        current.state_id,
    );

    assert_eq!(delta.row_patches.len(), 1);
    assert_eq!(delta.row_patches[0].row, 5);
}

#[test]
fn test_delta_uses_arc_pointer_equality() {
    let mut store = FrameStore::new(80, 24);
    let baseline = store.snapshot();

    store.update_row(0, |row| {
        row.set_cell(
            0,
            Cell {
                codepoint: 'A' as u32,
                width: 1,
                style_id: 0,
            },
        );
    });
    store.advance_state();

    let current = store.snapshot();
    let mut style_table = StyleTable::new();

    let delta = DeltaEngine::compute_delta(
        &baseline.data,
        &current.data,
        &mut style_table,
        baseline.state_id,
        current.state_id,
    );

    assert_eq!(delta.row_patches.len(), 1);
    assert_eq!(delta.row_patches[0].row, 0);
}

#[test]
fn test_delta_includes_cursor_change() {
    let mut store = FrameStore::new(80, 24);
    let baseline = store.snapshot();

    store.set_cursor(Cursor {
        row: 10,
        col: 20,
        visible: true,
        blink: false,
        shape: CursorShape::Underline,
    });
    store.advance_state();

    let current = store.snapshot();
    let mut style_table = StyleTable::new();

    let delta = DeltaEngine::compute_delta(
        &baseline.data,
        &current.data,
        &mut style_table,
        baseline.state_id,
        current.state_id,
    );

    assert!(delta.cursor.is_some());
    let cursor = delta.cursor.unwrap();
    assert_eq!(cursor.row, 10);
    assert_eq!(cursor.col, 20);
}

#[test]
fn test_snapshot_includes_all_rows() {
    let mut store = FrameStore::new(80, 24);

    store.update_row(0, |row| {
        row.set_cell(
            0,
            Cell {
                codepoint: 'A' as u32,
                width: 1,
                style_id: 0,
            },
        );
    });
    store.update_row(23, |row| {
        row.set_cell(
            79,
            Cell {
                codepoint: 'Z' as u32,
                width: 1,
                style_id: 0,
            },
        );
    });
    store.advance_state();

    let frame = store.snapshot();
    let mut style_table = StyleTable::new();

    let snapshot = DeltaEngine::compute_snapshot(&frame.data, &mut style_table, frame.state_id);

    assert_eq!(snapshot.rows.len(), 24);
    assert_eq!(snapshot.state_id, frame.state_id);
}

#[test]
fn test_delta_state_ids() {
    let mut store = FrameStore::new(80, 24);
    let baseline = store.snapshot();

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

    let current = store.snapshot();
    let mut style_table = StyleTable::new();

    let delta = DeltaEngine::compute_delta(
        &baseline.data,
        &current.data,
        &mut style_table,
        baseline.state_id,
        current.state_id,
    );

    assert_eq!(delta.base_state_id, baseline.state_id);
    assert_eq!(delta.state_id, current.state_id);
}

#[test]
fn test_row_patch_array_lengths_match() {
    let mut store = FrameStore::new(80, 24);
    store.update_row(0, |row| {
        row.set_cell(
            0,
            Cell {
                codepoint: 'A' as u32,
                width: 1,
                style_id: 0,
            },
        );
        row.set_cell(
            79,
            Cell {
                codepoint: 'Z' as u32,
                width: 1,
                style_id: 0,
            },
        );
    });
    store.advance_state();

    let baseline = FrameStore::new(80, 24).snapshot();
    let current = store.snapshot();
    let mut style_table = StyleTable::new();

    let delta = DeltaEngine::compute_delta(
        &baseline.data,
        &current.data,
        &mut style_table,
        baseline.state_id,
        current.state_id,
    );

    for patch in &delta.row_patches {
        for run in &patch.runs {
            assert_eq!(run.codepoints.len(), run.widths.len());
            assert_eq!(run.codepoints.len(), run.style_ids.len());
        }
    }
}

#[test]
fn test_snapshot_row_data_array_lengths_match() {
    let mut store = FrameStore::new(80, 24);
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

    let frame = store.snapshot();
    let mut style_table = StyleTable::new();

    let snapshot = DeltaEngine::compute_snapshot(&frame.data, &mut style_table, frame.state_id);

    for row_data in &snapshot.rows {
        assert_eq!(row_data.codepoints.len(), row_data.widths.len());
        assert_eq!(row_data.codepoints.len(), row_data.style_ids.len());
        assert_eq!(row_data.codepoints.len(), 80); // cols
    }
}

#[test]
fn test_delta_with_fewer_rows_than_baseline() {
    let baseline_store = FrameStore::new(80, 24);
    let baseline = baseline_store.snapshot();

    let mut current_store = FrameStore::new(80, 10); // Fewer rows
    current_store.advance_state();
    let current = current_store.snapshot();

    let mut style_table = StyleTable::new();

    let delta = DeltaEngine::compute_delta(
        &baseline.data,
        &current.data,
        &mut style_table,
        baseline.state_id,
        current.state_id,
    );

    // Delta should only contain patches for rows that exist in current
    for patch in &delta.row_patches {
        assert!(
            patch.row < 10,
            "Patch row {} exceeds current row count 10",
            patch.row
        );
    }
}

#[test]
fn test_delta_with_more_rows_than_baseline() {
    let baseline_store = FrameStore::new(80, 10);
    let baseline = baseline_store.snapshot();

    let mut current_store = FrameStore::new(80, 24); // More rows
    current_store.update_row(20, |row| {
        row.set_cell(
            0,
            Cell {
                codepoint: 'X' as u32,
                width: 1,
                style_id: 0,
            },
        );
    });
    current_store.advance_state();
    let current = current_store.snapshot();

    let mut style_table = StyleTable::new();

    let delta = DeltaEngine::compute_delta(
        &baseline.data,
        &current.data,
        &mut style_table,
        baseline.state_id,
        current.state_id,
    );

    // Should include patches for new rows (10-23)
    let new_row_patches: Vec<_> = delta.row_patches.iter().filter(|p| p.row >= 10).collect();
    assert_eq!(
        new_row_patches.len(),
        14,
        "Should have patches for rows 10-23"
    );
}

#[test]
fn test_cursor_shape_bar_maps_to_beam() {
    use zellij_remote_protocol::CursorShape as ProtoCursorShape;

    let mut store = FrameStore::new(80, 24);
    store.set_cursor(Cursor {
        row: 0,
        col: 0,
        visible: true,
        blink: false,
        shape: CursorShape::Bar,
    });
    store.advance_state();

    let baseline = FrameStore::new(80, 24).snapshot();
    let current = store.snapshot();
    let mut style_table = StyleTable::new();

    let delta = DeltaEngine::compute_delta(
        &baseline.data,
        &current.data,
        &mut style_table,
        baseline.state_id,
        current.state_id,
    );

    let cursor = delta.cursor.unwrap();
    assert_eq!(cursor.shape, ProtoCursorShape::Beam as i32);
}
