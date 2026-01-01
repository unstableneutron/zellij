use crate::frame::{CursorShape, FrameData, Row};
use crate::style_table::StyleTable;
use std::collections::HashSet;
use std::sync::Arc;
use zellij_remote_protocol::{
    CellRun, CursorShape as ProtoCursorShape, CursorState, DisplaySize, RowData, RowPatch,
    ScreenDelta, ScreenSnapshot, StyleDef,
};

pub struct DeltaEngine;

impl DeltaEngine {
    pub fn compute_delta(
        baseline: &FrameData,
        current: &FrameData,
        style_table: &mut StyleTable,
        base_state_id: u64,
        current_state_id: u64,
        dirty_rows: Option<&HashSet<usize>>,
    ) -> ScreenDelta {
        let mut row_patches = Vec::new();
        let style_baseline = style_table.current_count();

        // Collect candidate rows: dirty_rows if provided, else fall back to all rows
        let mut candidate_rows: Vec<usize> = if let Some(dirty) = dirty_rows {
            // Only consider rows marked dirty (filtered to valid range)
            dirty
                .iter()
                .filter(|&idx| *idx < current.rows.len())
                .copied()
                .collect()
        } else {
            // Fallback: compare all overlapping rows using Arc::ptr_eq
            (0..std::cmp::min(baseline.rows.len(), current.rows.len()))
                .filter(|&idx| !Arc::ptr_eq(&baseline.rows[idx].0, &current.rows[idx].0))
                .collect()
        };

        // Sort for deterministic ordering (HashSet iteration is nondeterministic)
        candidate_rows.sort_unstable();

        // Process candidate rows
        for row_idx in candidate_rows {
            let baseline_row = baseline.rows.get(row_idx);
            let current_row = &current.rows[row_idx];

            if let Some(patch) = Self::encode_row_patch(row_idx, baseline_row, current_row) {
                row_patches.push(patch);
            }
        }

        // Handle new rows (current has more rows than baseline)
        // Only add these when NOT using dirty_rows, since dirty_rows already includes new rows
        // (and we already handled them above with baseline_row=None)
        if dirty_rows.is_none() && current.rows.len() > baseline.rows.len() {
            for row_idx in baseline.rows.len()..current.rows.len() {
                if let Some(patch) = Self::encode_row_patch(row_idx, None, &current.rows[row_idx]) {
                    row_patches.push(patch);
                }
            }
        }

        let styles_added: Vec<StyleDef> = style_table
            .styles_since(style_baseline)
            .into_iter()
            .map(|(id, style)| StyleDef {
                style_id: id as u32,
                style: Some(style.clone()),
            })
            .collect();

        let cursor = if baseline.cursor != current.cursor {
            Some(Self::encode_cursor(&current.cursor))
        } else {
            None
        };

        ScreenDelta {
            base_state_id,
            state_id: current_state_id,
            row_patches,
            cursor,
            styles_added,
            delivered_input_watermark: 0,
        }
    }

    pub fn compute_snapshot(
        frame: &FrameData,
        style_table: &mut StyleTable,
        state_id: u64,
    ) -> ScreenSnapshot {
        let mut rows = Vec::with_capacity(frame.rows.len());

        for (row_idx, row) in frame.rows.iter().enumerate() {
            rows.push(Self::encode_row_data(row_idx, row));
        }

        let styles: Vec<StyleDef> = style_table
            .all_styles()
            .map(|(id, style)| StyleDef {
                style_id: id as u32,
                style: Some(style.clone()),
            })
            .collect();

        ScreenSnapshot {
            state_id,
            size: Some(DisplaySize {
                cols: frame.cols as u32,
                rows: frame.rows.len() as u32,
            }),
            rows,
            cursor: Some(Self::encode_cursor(&frame.cursor)),
            styles,
            style_table_reset: true,
            delivered_input_watermark: 0,
        }
    }

    /// Encode a row patch with sparse CellRuns containing only changed cells.
    /// Returns None if no cells changed (handles dirty false positives).
    fn encode_row_patch(row_idx: usize, baseline: Option<&Row>, current: &Row) -> Option<RowPatch> {
        let cols = current.cols();
        let mut runs: Vec<CellRun> = Vec::new();

        let mut col = 0;
        while col < cols {
            // Find start of changed region
            while col < cols && !Self::cell_changed(baseline, current, col) {
                col += 1;
            }

            if col >= cols {
                break;
            }

            // Found a changed cell - find the extent of the changed region
            let start_col = col;
            let mut codepoints = Vec::new();
            let mut widths = Vec::new();
            let mut style_ids = Vec::new();

            while col < cols && Self::cell_changed(baseline, current, col) {
                if let Some(cell) = current.get_cell(col) {
                    codepoints.push(cell.codepoint);
                    widths.push(cell.width as u32);
                    style_ids.push(cell.style_id as u32);
                }
                col += 1;
            }

            if !codepoints.is_empty() {
                runs.push(CellRun {
                    col_start: start_col as u32,
                    codepoints,
                    widths,
                    style_ids,
                });
            }
        }

        if runs.is_empty() {
            None
        } else {
            Some(RowPatch {
                row: row_idx as u32,
                runs,
            })
        }
    }

    /// Check if a cell has changed between baseline and current.
    /// Returns true if baseline is None (new row) or cell values differ.
    fn cell_changed(baseline: Option<&Row>, current: &Row, col: usize) -> bool {
        match baseline {
            None => true, // New row - all cells are "changed"
            Some(base_row) => {
                match (base_row.get_cell(col), current.get_cell(col)) {
                    (Some(base), Some(curr)) => {
                        base.codepoint != curr.codepoint
                            || base.width != curr.width
                            || base.style_id != curr.style_id
                    },
                    (None, Some(_)) => true, // New column
                    (Some(_), None) => true, // Deleted column
                    (None, None) => false,
                }
            },
        }
    }

    fn encode_row_data(row_idx: usize, row: &Row) -> RowData {
        let mut codepoints = Vec::with_capacity(row.cols());
        let mut widths = Vec::with_capacity(row.cols());
        let mut style_ids = Vec::with_capacity(row.cols());

        for i in 0..row.cols() {
            if let Some(cell) = row.get_cell(i) {
                codepoints.push(cell.codepoint);
                widths.push(cell.width as u32);
                style_ids.push(cell.style_id as u32);
            }
        }

        RowData {
            row: row_idx as u32,
            codepoints,
            widths,
            style_ids,
        }
    }

    fn encode_cursor(cursor: &crate::frame::Cursor) -> CursorState {
        CursorState {
            row: cursor.row,
            col: cursor.col,
            visible: cursor.visible,
            blink: cursor.blink,
            shape: match cursor.shape {
                CursorShape::Block => ProtoCursorShape::Block as i32,
                CursorShape::Underline => ProtoCursorShape::Underline as i32,
                CursorShape::Bar => ProtoCursorShape::Beam as i32,
            },
        }
    }
}
