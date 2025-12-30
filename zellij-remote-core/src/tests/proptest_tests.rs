use proptest::prelude::*;
use crate::delta::DeltaEngine;
use crate::frame::{Cell, FrameStore};
use crate::style_table::StyleTable;
use std::sync::Arc;

fn dimension_strategy() -> impl Strategy<Value = usize> {
    1usize..=200
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn prop_dirty_rows_within_bounds(cols in dimension_strategy(), rows in dimension_strategy()) {
        let mut store = FrameStore::new(cols, rows);

        for i in 0..std::cmp::min(rows, 10) {
            store.update_row(i, |row| {
                if cols > 0 {
                    row.set_cell(0, Cell { codepoint: 'A' as u32, width: 1, style_id: 0 });
                }
            });
        }

        let dirty = store.take_dirty_rows();
        for &row_idx in &dirty {
            prop_assert!(row_idx < rows, "Dirty row {} exceeds row count {}", row_idx, rows);
        }
    }

    #[test]
    fn prop_resize_marks_all_dirty(
        initial_cols in dimension_strategy(),
        initial_rows in dimension_strategy(),
        new_cols in dimension_strategy(),
        new_rows in dimension_strategy()
    ) {
        let mut store = FrameStore::new(initial_cols, initial_rows);
        store.take_dirty_rows();

        store.resize(new_cols, new_rows);
        let dirty = store.take_dirty_rows();

        prop_assert_eq!(dirty.len(), new_rows, "After resize, all {} rows should be dirty", new_rows);
    }

    #[test]
    fn prop_snapshot_row_count_matches(cols in dimension_strategy(), rows in dimension_strategy()) {
        let store = FrameStore::new(cols, rows);
        let snapshot = store.snapshot();

        prop_assert_eq!(snapshot.data.rows.len(), rows);
        prop_assert_eq!(snapshot.data.cols, cols);
    }

    #[test]
    fn prop_unchanged_rows_share_arc(cols in dimension_strategy(), rows in 2usize..=100) {
        let mut store = FrameStore::new(cols, rows);
        let baseline = store.snapshot();

        store.update_row(0, |row| {
            if cols > 0 {
                row.set_cell(0, Cell { codepoint: 'X' as u32, width: 1, style_id: 0 });
            }
        });
        store.advance_state();

        let current = store.snapshot();

        if cols > 0 {
            prop_assert!(!Arc::ptr_eq(&baseline.data.rows[0].0, &current.data.rows[0].0));
        }

        for i in 1..rows {
            prop_assert!(
                Arc::ptr_eq(&baseline.data.rows[i].0, &current.data.rows[i].0),
                "Row {} should share Arc", i
            );
        }
    }

    #[test]
    fn prop_delta_only_patches_changed_rows(cols in 1usize..=80, rows in 2usize..=24) {
        let mut store = FrameStore::new(cols, rows);
        let baseline = store.snapshot();

        store.update_row(0, |row| {
            row.set_cell(0, Cell { codepoint: 'M' as u32, width: 1, style_id: 0 });
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

        prop_assert_eq!(delta.row_patches.len(), 1);
        prop_assert_eq!(delta.row_patches[0].row, 0);
    }

    #[test]
    fn prop_delta_array_lengths_consistent(cols in 1usize..=80, rows in 1usize..=24) {
        let mut store = FrameStore::new(cols, rows);
        store.update_row(0, |row| {
            row.set_cell(0, Cell { codepoint: 'A' as u32, width: 1, style_id: 0 });
        });
        store.advance_state();

        let baseline = FrameStore::new(cols, rows).snapshot();
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
                prop_assert_eq!(
                    run.codepoints.len(),
                    run.widths.len(),
                    "codepoints and widths length mismatch"
                );
                prop_assert_eq!(
                    run.codepoints.len(),
                    run.style_ids.len(),
                    "codepoints and style_ids length mismatch"
                );
            }
        }
    }
}
