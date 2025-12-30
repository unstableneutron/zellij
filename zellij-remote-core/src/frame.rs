use std::collections::HashSet;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub codepoint: u32,
    pub width: u8,
    pub style_id: u16,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            codepoint: ' ' as u32,
            width: 1,
            style_id: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RowData {
    pub cells: Vec<Cell>,
}

impl RowData {
    pub fn new(cols: usize) -> Self {
        Self {
            cells: vec![Cell::default(); cols],
        }
    }
}

#[derive(Debug, Clone)]
pub struct Row(pub Arc<RowData>);

impl Row {
    pub fn new(cols: usize) -> Self {
        Self(Arc::new(RowData::new(cols)))
    }

    pub fn ptr_eq(&self, other: &Row) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    pub fn get_cell(&self, col: usize) -> Option<&Cell> {
        self.0.cells.get(col)
    }

    pub fn set_cell(&mut self, col: usize, cell: Cell) {
        let data = Arc::make_mut(&mut self.0);
        if col < data.cells.len() {
            data.cells[col] = cell;
        }
    }

    pub fn cols(&self) -> usize {
        self.0.cells.len()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cursor {
    pub row: u32,
    pub col: u32,
    pub visible: bool,
    pub blink: bool,
    pub shape: CursorShape,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorShape {
    Block = 0,
    Underline = 1,
    Bar = 2,
}

impl Default for Cursor {
    fn default() -> Self {
        Self {
            row: 0,
            col: 0,
            visible: true,
            blink: true,
            shape: CursorShape::Block,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FrameData {
    pub rows: Vec<Row>,
    pub cols: usize,
    pub cursor: Cursor,
}

impl FrameData {
    pub fn new(cols: usize, rows: usize) -> Self {
        Self {
            rows: (0..rows).map(|_| Row::new(cols)).collect(),
            cols,
            cursor: Cursor::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Frame {
    pub data: FrameData,
    pub state_id: u64,
}

pub struct FrameStore {
    current: FrameData,
    state_id: u64,
    dirty_rows: HashSet<usize>,
}

impl FrameStore {
    pub fn new(cols: usize, rows: usize) -> Self {
        Self {
            current: FrameData::new(cols, rows),
            state_id: 0,
            dirty_rows: HashSet::new(),
        }
    }

    pub fn current_frame(&self) -> &FrameData {
        &self.current
    }

    pub fn current_state_id(&self) -> u64 {
        self.state_id
    }

    pub fn update_row<F>(&mut self, row_idx: usize, f: F)
    where
        F: FnOnce(&mut Row),
    {
        if row_idx < self.current.rows.len() {
            f(&mut self.current.rows[row_idx]);
            self.dirty_rows.insert(row_idx);
        }
    }

    pub fn set_row(&mut self, row_idx: usize, row_data: RowData) {
        if row_idx < self.current.rows.len() {
            self.current.rows[row_idx] = Row(Arc::new(row_data));
            self.dirty_rows.insert(row_idx);
        }
    }

    pub fn set_cursor(&mut self, cursor: Cursor) {
        self.current.cursor = cursor;
    }

    pub fn advance_state(&mut self) {
        self.state_id += 1;
    }

    pub fn take_dirty_rows(&mut self) -> HashSet<usize> {
        std::mem::take(&mut self.dirty_rows)
    }

    pub fn snapshot(&self) -> Frame {
        Frame {
            data: self.current.clone(),
            state_id: self.state_id,
        }
    }

    pub fn resize(&mut self, new_cols: usize, new_rows: usize) {
        while self.current.rows.len() < new_rows {
            self.current.rows.push(Row::new(new_cols));
        }
        self.current.rows.truncate(new_rows);

        if new_cols != self.current.cols {
            for row in &mut self.current.rows {
                let data = Arc::make_mut(&mut row.0);
                data.cells.resize(new_cols, Cell::default());
            }
            self.current.cols = new_cols;
        }

        for i in 0..self.current.rows.len() {
            self.dirty_rows.insert(i);
        }
    }
}
