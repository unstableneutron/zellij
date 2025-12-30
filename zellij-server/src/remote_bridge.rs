//! Bridge between Zellij server and remote protocol.
//!
//! This module provides utilities to convert Zellij's Grid to the remote protocol's
//! FrameStore format for transmission to remote clients.

use std::collections::HashMap;
use std::rc::Rc;

use crate::panes::grid::{Grid, Row as ZellijRow};
#[cfg(test)]
use crate::panes::terminal_character::DEFAULT_STYLES;
use crate::panes::terminal_character::{
    AnsiCode, AnsiStyledUnderline, CharacterStyles, CursorShape as ZellijCursorShape, NamedColor,
    RcCharacterStyles, TerminalCharacter,
};
use zellij_remote_core::{Cell, Cursor, CursorShape, FrameStore, RowData, StyleTable};
use zellij_remote_protocol::{color, Color, DefaultColor, Rgb, Style, UnderlineStyle};

fn named_color_to_ansi256(color: NamedColor) -> u32 {
    match color {
        NamedColor::Black => 0,
        NamedColor::Red => 1,
        NamedColor::Green => 2,
        NamedColor::Yellow => 3,
        NamedColor::Blue => 4,
        NamedColor::Magenta => 5,
        NamedColor::Cyan => 6,
        NamedColor::White => 7,
        NamedColor::BrightBlack => 8,
        NamedColor::BrightRed => 9,
        NamedColor::BrightGreen => 10,
        NamedColor::BrightYellow => 11,
        NamedColor::BrightBlue => 12,
        NamedColor::BrightMagenta => 13,
        NamedColor::BrightCyan => 14,
        NamedColor::BrightWhite => 15,
    }
}

fn ansi_code_to_color(code: &Option<AnsiCode>) -> Option<Color> {
    match code {
        None => None,
        Some(AnsiCode::Reset) => Some(Color {
            value: Some(color::Value::DefaultColor(DefaultColor {})),
        }),
        Some(AnsiCode::NamedColor(named)) => Some(Color {
            value: Some(color::Value::Ansi256(named_color_to_ansi256(*named))),
        }),
        Some(AnsiCode::RgbCode((r, g, b))) => Some(Color {
            value: Some(color::Value::Rgb(Rgb {
                r: *r as u32,
                g: *g as u32,
                b: *b as u32,
            })),
        }),
        Some(AnsiCode::ColorIndex(idx)) => Some(Color {
            value: Some(color::Value::Ansi256(*idx as u32)),
        }),
        Some(AnsiCode::On | AnsiCode::Underline(_)) => None,
    }
}

fn ansi_code_to_underline_style(code: &AnsiCode) -> UnderlineStyle {
    match code {
        AnsiCode::On => UnderlineStyle::Single,
        AnsiCode::Underline(Some(styled)) => match styled {
            AnsiStyledUnderline::Double => UnderlineStyle::Double,
            AnsiStyledUnderline::Undercurl => UnderlineStyle::Curly,
            AnsiStyledUnderline::Underdotted => UnderlineStyle::Dotted,
            AnsiStyledUnderline::Underdashed => UnderlineStyle::Dashed,
        },
        AnsiCode::Underline(None) => UnderlineStyle::Single,
        _ => UnderlineStyle::None,
    }
}

fn character_styles_to_style(styles: &CharacterStyles) -> Style {
    Style {
        fg: ansi_code_to_color(&styles.foreground),
        bg: ansi_code_to_color(&styles.background),
        bold: styles
            .bold
            .as_ref()
            .map(|c| matches!(c, AnsiCode::On))
            .unwrap_or(false),
        dim: styles
            .dim
            .as_ref()
            .map(|c| matches!(c, AnsiCode::On))
            .unwrap_or(false),
        italic: styles
            .italic
            .as_ref()
            .map(|c| matches!(c, AnsiCode::On))
            .unwrap_or(false),
        reverse: styles
            .reverse
            .as_ref()
            .map(|c| matches!(c, AnsiCode::On))
            .unwrap_or(false),
        hidden: styles
            .hidden
            .as_ref()
            .map(|c| matches!(c, AnsiCode::On))
            .unwrap_or(false),
        strike: styles
            .strike
            .as_ref()
            .map(|c| matches!(c, AnsiCode::On))
            .unwrap_or(false),
        blink_slow: styles
            .slow_blink
            .as_ref()
            .map(|c| matches!(c, AnsiCode::On))
            .unwrap_or(false),
        blink_fast: styles
            .fast_blink
            .as_ref()
            .map(|c| matches!(c, AnsiCode::On))
            .unwrap_or(false),
        underline: styles
            .underline
            .as_ref()
            .map(|c| ansi_code_to_underline_style(c) as i32)
            .unwrap_or(UnderlineStyle::Unspecified as i32),
        underline_color: ansi_code_to_color(&styles.underline_color),
    }
}

/// Cache style IDs by RcCharacterStyles pointer to avoid re-encoding
fn get_cached_style_id(
    styles: &RcCharacterStyles,
    style_table: &mut StyleTable,
    cache: &mut HashMap<usize, u16>,
) -> u16 {
    let ptr = match styles {
        RcCharacterStyles::Reset => 0,
        RcCharacterStyles::Rc(rc) => Rc::as_ptr(rc) as usize,
    };

    if let Some(&id) = cache.get(&ptr) {
        return id;
    }

    let style = character_styles_to_style(styles);
    let id = style_table.get_or_insert(&style);
    cache.insert(ptr, id);
    id
}

pub fn terminal_character_to_cell(tc: &TerminalCharacter, style_table: &mut StyleTable) -> Cell {
    let style = character_styles_to_style(&tc.styles);
    let style_id = style_table.get_or_insert(&style);

    Cell {
        codepoint: tc.character as u32,
        width: tc.width() as u8,
        style_id,
    }
}

fn row_to_frame_row(
    zellij_row: &ZellijRow,
    cols: usize,
    style_table: &mut StyleTable,
    style_cache: &mut HashMap<usize, u16>,
) -> RowData {
    let mut cells = Vec::with_capacity(cols);
    let mut col = 0;

    for tc in zellij_row.columns.iter() {
        if col >= cols {
            break;
        }

        let width = tc.width();
        let style_id = get_cached_style_id(&tc.styles, style_table, style_cache);

        cells.push(Cell {
            codepoint: tc.character as u32,
            width: width as u8,
            style_id,
        });
        col += 1;

        for _ in 1..width {
            if col >= cols {
                break;
            }
            cells.push(Cell {
                codepoint: 0,
                width: 0,
                style_id,
            });
            col += 1;
        }
    }

    while col < cols {
        cells.push(Cell::default());
        col += 1;
    }

    RowData { cells }
}

pub fn zellij_cursor_shape_to_zrp(shape: &ZellijCursorShape) -> (CursorShape, bool) {
    match shape {
        ZellijCursorShape::Initial | ZellijCursorShape::Block => (CursorShape::Block, false),
        ZellijCursorShape::BlinkingBlock => (CursorShape::Block, true),
        ZellijCursorShape::Underline => (CursorShape::Underline, false),
        ZellijCursorShape::BlinkingUnderline => (CursorShape::Underline, true),
        ZellijCursorShape::Beam => (CursorShape::Bar, false),
        ZellijCursorShape::BlinkingBeam => (CursorShape::Bar, true),
    }
}

pub fn grid_to_frame_store(grid: &Grid, style_table: &mut StyleTable) -> FrameStore {
    let cols = grid.width;
    let rows = grid.height;
    let mut store = FrameStore::new(cols, rows);
    let mut style_cache: HashMap<usize, u16> = HashMap::new();

    for (row_idx, zellij_row) in grid.viewport().iter().enumerate() {
        if row_idx >= rows {
            break;
        }
        let row_data = row_to_frame_row(zellij_row, cols, style_table, &mut style_cache);
        store.set_row(row_idx, row_data);
    }

    let (cursor_shape, cursor_blink) = zellij_cursor_shape_to_zrp(&grid.cursor_shape());

    let cursor = if let Some((x, y)) = grid.cursor_coordinates() {
        let clamped_row = y.min(rows.saturating_sub(1));
        let clamped_col = x.min(cols.saturating_sub(1));
        Cursor {
            row: clamped_row as u32,
            col: clamped_col as u32,
            visible: !grid.cursor_is_hidden() && rows > 0 && cols > 0,
            blink: cursor_blink,
            shape: cursor_shape,
        }
    } else {
        Cursor {
            row: 0,
            col: 0,
            visible: false,
            blink: false,
            shape: CursorShape::Block,
        }
    };

    store.set_cursor(cursor);
    store.advance_state();
    store
}

pub fn viewport_to_frame_store<'a, I>(
    viewport: I,
    cursor_x: usize,
    cursor_y: usize,
    cursor_shape: CursorShape,
    cursor_blink: bool,
    cursor_visible: bool,
    cols: usize,
    rows: usize,
    style_table: &mut StyleTable,
) -> FrameStore
where
    I: Iterator<Item = &'a ZellijRow>,
{
    let mut store = FrameStore::new(cols, rows);
    let mut style_cache: HashMap<usize, u16> = HashMap::new();

    for (row_idx, zellij_row) in viewport.enumerate() {
        if row_idx >= rows {
            break;
        }
        let row_data = row_to_frame_row(zellij_row, cols, style_table, &mut style_cache);
        store.set_row(row_idx, row_data);
    }

    let clamped_row = cursor_y.min(rows.saturating_sub(1));
    let clamped_col = cursor_x.min(cols.saturating_sub(1));
    store.set_cursor(Cursor {
        row: clamped_row as u32,
        col: clamped_col as u32,
        visible: cursor_visible && rows > 0 && cols > 0,
        blink: cursor_blink,
        shape: cursor_shape,
    });

    store.advance_state();
    store
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_named_color_mapping() {
        assert_eq!(named_color_to_ansi256(NamedColor::Black), 0);
        assert_eq!(named_color_to_ansi256(NamedColor::White), 7);
        assert_eq!(named_color_to_ansi256(NamedColor::BrightWhite), 15);
    }

    #[test]
    fn test_ansi_code_to_color() {
        let rgb = Some(AnsiCode::RgbCode((255, 128, 0)));
        let color = ansi_code_to_color(&rgb).unwrap();
        match color.value {
            Some(color::Value::Rgb(rgb)) => {
                assert_eq!(rgb.r, 255);
                assert_eq!(rgb.g, 128);
                assert_eq!(rgb.b, 0);
            },
            _ => panic!("Expected RGB color"),
        }

        let indexed = Some(AnsiCode::ColorIndex(196));
        let color = ansi_code_to_color(&indexed).unwrap();
        match color.value {
            Some(color::Value::Ansi256(idx)) => {
                assert_eq!(idx, 196);
            },
            _ => panic!("Expected ansi256 color"),
        }
    }

    #[test]
    fn test_ansi_reset_to_default_color() {
        let reset = Some(AnsiCode::Reset);
        let color = ansi_code_to_color(&reset).unwrap();
        match color.value {
            Some(color::Value::DefaultColor(_)) => {},
            _ => panic!("Expected DefaultColor for Reset"),
        }
    }

    #[test]
    fn test_none_color_returns_none() {
        let none: Option<AnsiCode> = None;
        assert!(ansi_code_to_color(&none).is_none());
    }

    #[test]
    fn test_underline_style_conversion() {
        assert_eq!(
            ansi_code_to_underline_style(&AnsiCode::On),
            UnderlineStyle::Single
        );
        assert_eq!(
            ansi_code_to_underline_style(&AnsiCode::Underline(Some(
                AnsiStyledUnderline::Undercurl
            ))),
            UnderlineStyle::Curly
        );
        assert_eq!(
            ansi_code_to_underline_style(&AnsiCode::Underline(Some(AnsiStyledUnderline::Double))),
            UnderlineStyle::Double
        );
    }

    #[test]
    fn test_default_underline_is_unspecified() {
        let styles = DEFAULT_STYLES;
        let style = character_styles_to_style(&styles);
        assert_eq!(style.underline, UnderlineStyle::Unspecified as i32);
    }

    #[test]
    fn test_cursor_shape_conversion() {
        let (shape, blink) = zellij_cursor_shape_to_zrp(&ZellijCursorShape::Block);
        assert_eq!(shape, CursorShape::Block);
        assert!(!blink);

        let (shape, blink) = zellij_cursor_shape_to_zrp(&ZellijCursorShape::BlinkingBeam);
        assert_eq!(shape, CursorShape::Bar);
        assert!(blink);
    }

    #[test]
    fn test_terminal_character_conversion() {
        let mut style_table = StyleTable::new();
        let tc = TerminalCharacter::new('A');
        let cell = terminal_character_to_cell(&tc, &mut style_table);
        assert_eq!(cell.codepoint, 'A' as u32);
        assert_eq!(cell.width, 1);
    }

    #[test]
    fn test_style_caching() {
        let mut style_table = StyleTable::new();
        let mut cache: HashMap<usize, u16> = HashMap::new();

        let styles1 = RcCharacterStyles::default();
        let styles2 = styles1.clone();

        let id1 = get_cached_style_id(&styles1, &mut style_table, &mut cache);
        let id2 = get_cached_style_id(&styles2, &mut style_table, &mut cache);

        assert_eq!(id1, id2);
        assert_eq!(cache.len(), 1);
    }
}
