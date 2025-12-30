use crate::frame::{CursorShape, FrameData, Row};
use crate::style_table::StyleTable;
use std::sync::Arc;
use zellij_remote_protocol::{
    CellRun, CursorState, DisplaySize, RowData, RowPatch, ScreenDelta, ScreenSnapshot, StyleDef,
    CursorShape as ProtoCursorShape,
};

pub struct DeltaEngine;

impl DeltaEngine {
    pub fn compute_delta(
        baseline: &FrameData,
        current: &FrameData,
        style_table: &mut StyleTable,
        base_state_id: u64,
        current_state_id: u64,
    ) -> ScreenDelta {
        let mut row_patches = Vec::new();
        let style_baseline = style_table.current_count();

        for (row_idx, (base_row, curr_row)) in
            baseline.rows.iter().zip(current.rows.iter()).enumerate()
        {
            if !Arc::ptr_eq(&base_row.0, &curr_row.0) {
                let patch = Self::encode_row_patch(row_idx, curr_row);
                row_patches.push(patch);
            }
        }

        if current.rows.len() > baseline.rows.len() {
            for row_idx in baseline.rows.len()..current.rows.len() {
                let patch = Self::encode_row_patch(row_idx, &current.rows[row_idx]);
                row_patches.push(patch);
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

    fn encode_row_patch(row_idx: usize, row: &Row) -> RowPatch {
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

        RowPatch {
            row: row_idx as u32,
            runs: vec![CellRun {
                col_start: 0,
                codepoints,
                widths,
                style_ids,
            }],
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
