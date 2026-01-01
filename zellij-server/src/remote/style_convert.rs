//! Style conversion utilities for the remote protocol.
//!
//! This module provides functions to convert Zellij's character styles
//! to the remote protocol's Style format.

use std::collections::HashMap;
use std::rc::Rc;

use crate::panes::terminal_character::NamedColor;
use crate::panes::terminal_character::{
    AnsiCode, AnsiStyledUnderline, CharacterStyles, RcCharacterStyles, TerminalCharacter,
};
use zellij_remote_core::{Cell, StyleTable};
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

/// Convert Zellij's CharacterStyles to a remote protocol Style
pub fn character_styles_to_style(styles: &CharacterStyles) -> Style {
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
#[allow(dead_code)]
pub fn get_cached_style_id(
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

/// Convert a TerminalCharacter to a Cell for the remote protocol
#[allow(dead_code)]
pub fn terminal_character_to_cell(tc: &TerminalCharacter, style_table: &mut StyleTable) -> Cell {
    character_styles_to_cell(tc.character, tc.width(), &tc.styles, style_table)
}

/// Convert character data with given styles to a Cell for the remote protocol.
/// This variant allows passing pre-adjusted styles (e.g., with selection highlighting applied).
pub fn character_styles_to_cell(
    character: char,
    width: usize,
    styles: &CharacterStyles,
    style_table: &mut StyleTable,
) -> Cell {
    let style = character_styles_to_style(styles);
    let style_id = style_table.get_or_insert(&style);

    Cell {
        codepoint: character as u32,
        width: width as u8,
        style_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panes::terminal_character::DEFAULT_STYLES;

    #[test]
    fn test_default_styles_to_style() {
        let style = character_styles_to_style(&DEFAULT_STYLES);
        assert!(!style.bold);
        assert!(!style.italic);
        assert!(style.fg.is_none());
        assert!(style.bg.is_none());
    }

    #[test]
    fn test_terminal_character_to_cell() {
        let mut style_table = StyleTable::new();
        let tc = TerminalCharacter::new('A');
        let cell = terminal_character_to_cell(&tc, &mut style_table);
        assert_eq!(cell.codepoint, 'A' as u32);
        assert_eq!(cell.width, 1);
    }

    #[test]
    fn test_wide_character_to_cell() {
        let mut style_table = StyleTable::new();
        let tc = TerminalCharacter::new('中');
        let cell = terminal_character_to_cell(&tc, &mut style_table);
        assert_eq!(cell.codepoint, '中' as u32);
        assert_eq!(cell.width, 2);
    }

    #[test]
    fn test_style_caching() {
        let mut style_table = StyleTable::new();
        let mut cache = HashMap::new();

        let styles = RcCharacterStyles::default();
        let id1 = get_cached_style_id(&styles, &mut style_table, &mut cache);
        let id2 = get_cached_style_id(&styles, &mut style_table, &mut cache);

        assert_eq!(id1, id2);
    }
}
