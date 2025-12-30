//! Client-side prediction for low-latency input feel.
//!
//! The prediction engine maintains an overlay of unconfirmed input effects
//! on top of the server's confirmed state. When the server acknowledges
//! input via `delivered_input_watermark`, predictions are confirmed or
//! rolled back if they don't match.

use crate::frame::{Cell, Cursor, FrameData};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

#[derive(Clone, Debug)]
pub struct Prediction {
    pub input_seq: u64,
    pub cursor: Cursor,
    pub cells: Vec<(usize, usize, Cell)>,
    pub timestamp: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Confidence {
    High,
    Medium,
    Low,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReconcileResult {
    NoChange,
    Confirmed,
    Misprediction,
}

pub struct PredictionEngine {
    pending: VecDeque<Prediction>,
    last_confirmed_seq: u64,
    enabled: bool,
    max_pending: usize,
    misprediction_count: u32,
    misprediction_threshold: u32,
}

impl Default for PredictionEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl PredictionEngine {
    pub fn new() -> Self {
        Self {
            pending: VecDeque::new(),
            last_confirmed_seq: 0,
            enabled: true,
            max_pending: 100,
            misprediction_count: 0,
            misprediction_threshold: 5,
        }
    }

    pub fn predict_char(
        &mut self,
        ch: char,
        input_seq: u64,
        cursor: &Cursor,
        cols: usize,
    ) -> Option<Prediction> {
        if !self.enabled || self.pending.len() >= self.max_pending {
            return None;
        }

        if self.confidence(ch) == Confidence::None {
            return None;
        }

        let width = char_display_width(ch) as usize;
        let cell = Cell {
            codepoint: ch as u32,
            width: width as u8,
            style_id: 0,
        };

        let new_col = (cursor.col as usize + width).min(cols.saturating_sub(1));
        let new_cursor = Cursor {
            col: new_col as u32,
            ..*cursor
        };

        let mut cells = vec![(cursor.col as usize, cursor.row as usize, cell)];

        if width > 1 {
            for i in 1..width {
                let continuation = Cell {
                    codepoint: 0,
                    width: 0,
                    style_id: 0,
                };
                cells.push((cursor.col as usize + i, cursor.row as usize, continuation));
            }
        }

        let prediction = Prediction {
            input_seq,
            cursor: new_cursor,
            cells,
            timestamp: Instant::now(),
        };

        self.pending.push_back(prediction.clone());
        Some(prediction)
    }

    pub fn apply_overlay(&self, base: &FrameData) -> FrameData {
        if self.pending.is_empty() {
            return base.clone();
        }

        let mut overlay = base.clone();
        for pred in &self.pending {
            for &(col, row, ref cell) in &pred.cells {
                if row < overlay.rows.len() {
                    let row_data = Arc::make_mut(&mut overlay.rows[row].0);
                    if col < row_data.cells.len() {
                        row_data.cells[col] = *cell;
                    }
                }
            }
            overlay.cursor = pred.cursor;
        }
        overlay
    }

    pub fn reconcile(&mut self, delivered_watermark: u64, server_cursor: &Cursor) -> ReconcileResult {
        if delivered_watermark <= self.last_confirmed_seq {
            return ReconcileResult::NoChange;
        }

        self.last_confirmed_seq = delivered_watermark;

        let mut confirmed_count = 0u32;
        let mut last_confirmed_cursor: Option<Cursor> = None;
        while let Some(pred) = self.pending.front() {
            if pred.input_seq <= delivered_watermark {
                last_confirmed_cursor = Some(self.pending.pop_front().unwrap().cursor);
                confirmed_count += 1;
            } else {
                break;
            }
        }

        if confirmed_count == 0 {
            return ReconcileResult::NoChange;
        }

        if let Some(confirmed_cursor) = last_confirmed_cursor {
            if confirmed_cursor.col != server_cursor.col || confirmed_cursor.row != server_cursor.row {
                self.misprediction_count += 1;
                self.pending.clear();

                if self.misprediction_count >= self.misprediction_threshold {
                    self.enabled = false;
                }

                return ReconcileResult::Misprediction;
            }
        }

        self.misprediction_count = self.misprediction_count.saturating_sub(1);
        ReconcileResult::Confirmed
    }

    pub fn confidence(&self, ch: char) -> Confidence {
        if !self.enabled {
            return Confidence::None;
        }

        match ch {
            ' '..='~' => Confidence::High,
            '\x00'..='\x1f' | '\x7f' => Confidence::None,
            _ => Confidence::Medium,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn disable(&mut self) {
        self.enabled = false;
        self.pending.clear();
    }

    pub fn enable(&mut self) {
        self.enabled = true;
        self.misprediction_count = 0;
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    pub fn last_confirmed_seq(&self) -> u64 {
        self.last_confirmed_seq
    }

    pub fn misprediction_count(&self) -> u32 {
        self.misprediction_count
    }

    pub fn clear(&mut self) {
        self.pending.clear();
    }

    pub fn pending_predictions(&self) -> impl Iterator<Item = &Prediction> {
        self.pending.iter()
    }
}

fn char_display_width(ch: char) -> u8 {
    if ch.is_ascii() {
        1
    } else {
        match ch {
            '\u{1100}'..='\u{115F}'
            | '\u{2329}'..='\u{232A}'
            | '\u{2E80}'..='\u{303E}'
            | '\u{3040}'..='\u{A4CF}'
            | '\u{AC00}'..='\u{D7A3}'
            | '\u{F900}'..='\u{FAFF}'
            | '\u{FE10}'..='\u{FE1F}'
            | '\u{FE30}'..='\u{FE6F}'
            | '\u{FF00}'..='\u{FF60}'
            | '\u{FFE0}'..='\u{FFE6}'
            | '\u{20000}'..='\u{2FFFD}'
            | '\u{30000}'..='\u{3FFFD}' => 2,
            _ => 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_cursor(col: u32, row: u32) -> Cursor {
        Cursor {
            col,
            row,
            visible: true,
            blink: true,
            shape: crate::frame::CursorShape::Block,
        }
    }

    #[test]
    fn test_predict_char_creates_overlay() {
        let mut engine = PredictionEngine::new();
        let cursor = make_cursor(5, 0);

        let pred = engine.predict_char('a', 1, &cursor, 80).unwrap();

        assert_eq!(pred.input_seq, 1);
        assert_eq!(pred.cursor.col, 6);
        assert_eq!(pred.cells.len(), 1);
        assert_eq!(pred.cells[0], (5, 0, Cell { codepoint: 'a' as u32, width: 1, style_id: 0 }));

        let base = FrameData::new(80, 24);
        let overlay = engine.apply_overlay(&base);

        assert_eq!(overlay.cursor.col, 6);
        assert_eq!(overlay.rows[0].get_cell(5).unwrap().codepoint, 'a' as u32);
    }

    #[test]
    fn test_reconcile_confirms_predictions() {
        let mut engine = PredictionEngine::new();
        let cursor = make_cursor(0, 0);

        engine.predict_char('a', 1, &cursor, 80);
        engine.predict_char('b', 2, &make_cursor(1, 0), 80);
        engine.predict_char('c', 3, &make_cursor(2, 0), 80);

        assert_eq!(engine.pending_count(), 3);

        let server_cursor = make_cursor(2, 0);
        let result = engine.reconcile(2, &server_cursor);

        assert_eq!(result, ReconcileResult::Confirmed);
        assert_eq!(engine.pending_count(), 1);
        assert_eq!(engine.last_confirmed_seq(), 2);
    }

    #[test]
    fn test_misprediction_clears_pending() {
        let mut engine = PredictionEngine::new();
        let cursor = make_cursor(0, 0);

        engine.predict_char('a', 1, &cursor, 80);
        engine.predict_char('b', 2, &make_cursor(1, 0), 80);

        let wrong_cursor = make_cursor(10, 0);
        let result = engine.reconcile(1, &wrong_cursor);

        assert_eq!(result, ReconcileResult::Misprediction);
        assert_eq!(engine.pending_count(), 0);
        assert_eq!(engine.misprediction_count(), 1);
    }

    #[test]
    fn test_max_pending_stops_prediction() {
        let mut engine = PredictionEngine::new();
        engine.max_pending = 3;

        for i in 0..5 {
            engine.predict_char('x', i, &make_cursor(i as u32, 0), 80);
        }

        assert_eq!(engine.pending_count(), 3);
    }

    #[test]
    fn test_confidence_levels() {
        let engine = PredictionEngine::new();

        assert_eq!(engine.confidence('a'), Confidence::High);
        assert_eq!(engine.confidence(' '), Confidence::High);
        assert_eq!(engine.confidence('~'), Confidence::High);
        assert_eq!(engine.confidence('\n'), Confidence::None);
        assert_eq!(engine.confidence('\x1b'), Confidence::None);
        assert_eq!(engine.confidence('日'), Confidence::Medium);
    }

    #[test]
    fn test_control_chars_not_predicted() {
        let mut engine = PredictionEngine::new();
        let cursor = make_cursor(0, 0);

        assert!(engine.predict_char('\n', 1, &cursor, 80).is_none());
        assert!(engine.predict_char('\x1b', 2, &cursor, 80).is_none());
        assert!(engine.predict_char('\r', 3, &cursor, 80).is_none());

        assert_eq!(engine.pending_count(), 0);
    }

    #[test]
    fn test_disable_after_mispredictions() {
        let mut engine = PredictionEngine::new();
        engine.misprediction_threshold = 2;

        let cursor = make_cursor(0, 0);
        engine.predict_char('a', 1, &cursor, 80);
        engine.reconcile(1, &make_cursor(10, 0));
        engine.predict_char('b', 2, &make_cursor(0, 0), 80);
        engine.reconcile(2, &make_cursor(20, 0));

        assert!(!engine.is_enabled());
        assert!(engine.predict_char('c', 3, &cursor, 80).is_none());
    }

    #[test]
    fn test_wide_char_prediction() {
        let mut engine = PredictionEngine::new();
        let cursor = make_cursor(0, 0);

        let pred = engine.predict_char('日', 1, &cursor, 80).unwrap();

        assert_eq!(pred.cursor.col, 2);
        assert_eq!(pred.cells.len(), 2);
        assert_eq!(pred.cells[0].2.width, 2);
        assert_eq!(pred.cells[1].2.codepoint, 0);
        assert_eq!(pred.cells[1].2.width, 0);
    }

    #[test]
    fn test_enable_resets_misprediction_count() {
        let mut engine = PredictionEngine::new();
        engine.misprediction_threshold = 1;

        let cursor = make_cursor(0, 0);
        engine.predict_char('a', 1, &cursor, 80);
        engine.reconcile(1, &make_cursor(10, 0));

        assert!(!engine.is_enabled());

        engine.enable();

        assert!(engine.is_enabled());
        assert_eq!(engine.misprediction_count(), 0);
    }

    #[test]
    fn test_cursor_clamps_at_screen_edge() {
        let mut engine = PredictionEngine::new();
        let cursor = make_cursor(79, 0);

        let pred = engine.predict_char('a', 1, &cursor, 80).unwrap();

        assert_eq!(pred.cursor.col, 79);
    }

    #[test]
    fn test_misprediction_decay_on_confirmation() {
        let mut engine = PredictionEngine::new();
        engine.misprediction_threshold = 5;

        let cursor = make_cursor(0, 0);
        engine.predict_char('a', 1, &cursor, 80);
        engine.reconcile(1, &make_cursor(10, 0));
        assert_eq!(engine.misprediction_count(), 1);

        engine.predict_char('b', 2, &make_cursor(0, 0), 80);
        engine.reconcile(2, &make_cursor(1, 0));
        assert_eq!(engine.misprediction_count(), 0);
    }

    #[test]
    fn test_reconcile_returns_no_change_when_nothing_confirmed() {
        let mut engine = PredictionEngine::new();

        engine.predict_char('a', 5, &make_cursor(0, 0), 80);

        let result = engine.reconcile(3, &make_cursor(0, 0));
        assert_eq!(result, ReconcileResult::NoChange);
        assert_eq!(engine.pending_count(), 1);
    }
}
