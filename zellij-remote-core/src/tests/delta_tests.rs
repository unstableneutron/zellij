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
        None,
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
        None,
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
        None,
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
        None,
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
        None,
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
        None,
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
        None,
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
        None,
    );

    let cursor = delta.cursor.unwrap();
    assert_eq!(cursor.shape, ProtoCursorShape::Beam as i32);
}

#[test]
fn test_intra_row_diff_single_char_change() {
    let mut store = FrameStore::new(80, 24);
    let baseline = store.snapshot();

    // Change only column 5
    store.update_row(0, |row| {
        row.set_cell(
            5,
            Cell {
                codepoint: 'X' as u32,
                width: 1,
                style_id: 0,
            },
        );
    });
    store.advance_state();
    let dirty = store.take_dirty_rows();

    let current = store.snapshot();
    let mut style_table = StyleTable::new();

    let delta = DeltaEngine::compute_delta(
        &baseline.data,
        &current.data,
        &mut style_table,
        baseline.state_id,
        current.state_id,
        Some(&dirty),
    );

    // Should have exactly 1 row patch
    assert_eq!(delta.row_patches.len(), 1);
    assert_eq!(delta.row_patches[0].row, 0);

    // Should have exactly 1 run starting at col 5 with 1 cell
    assert_eq!(delta.row_patches[0].runs.len(), 1);
    assert_eq!(delta.row_patches[0].runs[0].col_start, 5);
    assert_eq!(delta.row_patches[0].runs[0].codepoints.len(), 1);
    assert_eq!(delta.row_patches[0].runs[0].codepoints[0], 'X' as u32);
}

#[test]
fn test_intra_row_diff_non_contiguous_changes() {
    let mut store = FrameStore::new(80, 24);
    let baseline = store.snapshot();

    // Change columns 0 and 10 (non-contiguous)
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
            10,
            Cell {
                codepoint: 'B' as u32,
                width: 1,
                style_id: 0,
            },
        );
    });
    store.advance_state();
    let dirty = store.take_dirty_rows();

    let current = store.snapshot();
    let mut style_table = StyleTable::new();

    let delta = DeltaEngine::compute_delta(
        &baseline.data,
        &current.data,
        &mut style_table,
        baseline.state_id,
        current.state_id,
        Some(&dirty),
    );

    assert_eq!(delta.row_patches.len(), 1);

    // Should have 2 runs: one at col 0, one at col 10
    assert_eq!(delta.row_patches[0].runs.len(), 2);
    assert_eq!(delta.row_patches[0].runs[0].col_start, 0);
    assert_eq!(delta.row_patches[0].runs[0].codepoints[0], 'A' as u32);
    assert_eq!(delta.row_patches[0].runs[1].col_start, 10);
    assert_eq!(delta.row_patches[0].runs[1].codepoints[0], 'B' as u32);
}

#[test]
fn test_dirty_row_false_positive_produces_no_patch() {
    let store = FrameStore::new(80, 24);
    let baseline = store.snapshot();
    let current = store.snapshot(); // Identical to baseline

    let mut style_table = StyleTable::new();

    // Manually mark row 5 as dirty even though nothing changed
    let mut dirty = std::collections::HashSet::new();
    dirty.insert(5);

    let delta = DeltaEngine::compute_delta(
        &baseline.data,
        &current.data,
        &mut style_table,
        baseline.state_id,
        current.state_id,
        Some(&dirty),
    );

    // No actual changes, so no patches
    assert!(delta.row_patches.is_empty());
}

#[test]
fn test_intra_row_diff_contiguous_changes() {
    let mut store = FrameStore::new(80, 24);
    let baseline = store.snapshot();

    // Change columns 5-9 (contiguous span)
    store.update_row(0, |row| {
        for col in 5..10 {
            row.set_cell(
                col,
                Cell {
                    codepoint: ('A' as u32) + (col - 5) as u32,
                    width: 1,
                    style_id: 0,
                },
            );
        }
    });
    store.advance_state();
    let dirty = store.take_dirty_rows();

    let current = store.snapshot();
    let mut style_table = StyleTable::new();

    let delta = DeltaEngine::compute_delta(
        &baseline.data,
        &current.data,
        &mut style_table,
        baseline.state_id,
        current.state_id,
        Some(&dirty),
    );

    assert_eq!(delta.row_patches.len(), 1);

    // Should have 1 run with 5 cells starting at col 5
    assert_eq!(delta.row_patches[0].runs.len(), 1);
    assert_eq!(delta.row_patches[0].runs[0].col_start, 5);
    assert_eq!(delta.row_patches[0].runs[0].codepoints.len(), 5);
}

#[test]
fn test_style_only_change_produces_run() {
    let mut store = FrameStore::new(80, 24);

    // Set initial cell with style 0
    store.update_row(0, |row| {
        row.set_cell(
            5,
            Cell {
                codepoint: 'X' as u32,
                width: 1,
                style_id: 0,
            },
        );
    });
    store.advance_state();
    let baseline = store.snapshot();
    store.take_dirty_rows(); // Clear

    // Change only style (same codepoint/width)
    store.update_row(0, |row| {
        row.set_cell(
            5,
            Cell {
                codepoint: 'X' as u32,
                width: 1,
                style_id: 1,
            },
        ); // Different style
    });
    store.advance_state();
    let dirty = store.take_dirty_rows();

    let current = store.snapshot();
    let mut style_table = StyleTable::new();

    let delta = DeltaEngine::compute_delta(
        &baseline.data,
        &current.data,
        &mut style_table,
        baseline.state_id,
        current.state_id,
        Some(&dirty),
    );

    // Should detect style change
    assert_eq!(delta.row_patches.len(), 1);
    assert_eq!(delta.row_patches[0].runs.len(), 1);
    assert_eq!(delta.row_patches[0].runs[0].style_ids[0], 1);
}

#[test]
fn test_multiple_dirty_rows_ordered() {
    let mut store = FrameStore::new(80, 24);
    let baseline = store.snapshot();

    // Change rows 10, 5, 15 (out of order)
    store.update_row(10, |row| {
        row.set_cell(
            0,
            Cell {
                codepoint: 'A' as u32,
                width: 1,
                style_id: 0,
            },
        );
    });
    store.update_row(5, |row| {
        row.set_cell(
            0,
            Cell {
                codepoint: 'B' as u32,
                width: 1,
                style_id: 0,
            },
        );
    });
    store.update_row(15, |row| {
        row.set_cell(
            0,
            Cell {
                codepoint: 'C' as u32,
                width: 1,
                style_id: 0,
            },
        );
    });
    store.advance_state();
    let dirty = store.take_dirty_rows();

    let current = store.snapshot();
    let mut style_table = StyleTable::new();

    let delta = DeltaEngine::compute_delta(
        &baseline.data,
        &current.data,
        &mut style_table,
        baseline.state_id,
        current.state_id,
        Some(&dirty),
    );

    // Should have 3 patches in sorted order
    assert_eq!(delta.row_patches.len(), 3);
    assert_eq!(delta.row_patches[0].row, 5);
    assert_eq!(delta.row_patches[1].row, 10);
    assert_eq!(delta.row_patches[2].row, 15);
}

#[test]
fn test_new_rows_not_duplicated_when_dirty_rows_provided() {
    // Baseline has 10 rows, current has 12 rows
    // dirty_rows includes the new rows (10, 11)
    // Should NOT duplicate patches for rows 10 and 11
    let baseline_store = FrameStore::new(80, 10);
    let baseline = baseline_store.snapshot();

    let mut current_store = FrameStore::new(80, 12);
    // Modify new rows to make them "dirty"
    current_store.update_row(10, |row| {
        row.set_cell(
            0,
            Cell {
                codepoint: 'X' as u32,
                width: 1,
                style_id: 0,
            },
        );
    });
    current_store.update_row(11, |row| {
        row.set_cell(
            0,
            Cell {
                codepoint: 'Y' as u32,
                width: 1,
                style_id: 0,
            },
        );
    });
    current_store.advance_state();

    // Manually create dirty_rows that includes the new rows
    let mut dirty = std::collections::HashSet::new();
    dirty.insert(10);
    dirty.insert(11);

    let current = current_store.snapshot();
    let mut style_table = StyleTable::new();

    let delta = DeltaEngine::compute_delta(
        &baseline.data,
        &current.data,
        &mut style_table,
        baseline.state_id,
        current.state_id,
        Some(&dirty),
    );

    // Should have exactly 2 patches (one for row 10, one for row 11)
    // NOT 4 patches (which would happen if we double-emitted)
    assert_eq!(delta.row_patches.len(), 2);
    assert_eq!(delta.row_patches[0].row, 10);
    assert_eq!(delta.row_patches[1].row, 11);
}
