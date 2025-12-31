//! Output to FrameStore conversion for remote protocol.
//!
//! This module converts Zellij's Output (CharacterChunks) to a FrameStore
//! for transmission to remote clients. This captures the full composited
//! screen including all panes, floating windows, and UI elements.

use crate::output::CharacterChunk;
use crate::panes::terminal_character::{AnsiCode, CharacterStyles};
use crate::panes::Selection;
use zellij_remote_core::{Cell, FrameStore, StyleTable};

use super::style_convert::character_styles_to_cell;

/// Apply selection styling to a character's styles if it falls within a selection region.
/// This mirrors the logic from `adjust_styles_for_possible_selection` in output/mod.rs.
fn apply_selection_styling(
    selection_and_colors: &[(Selection, AnsiCode, Option<AnsiCode>)],
    character_styles: CharacterStyles,
    chunk_y: usize,
    chunk_x: usize,
) -> CharacterStyles {
    selection_and_colors
        .iter()
        .find(|(selection, _background_color, _foreground_color)| {
            selection.contains(chunk_y, chunk_x)
        })
        .map(|(_selection, background_color, foreground_color)| {
            let mut styles = character_styles.background(Some(*background_color));
            if let Some(fg) = foreground_color {
                styles = styles.foreground(Some(*fg));
            }
            styles
        })
        .unwrap_or(character_styles)
}

/// Convert Output's character chunks to a FrameStore
///
/// This captures the full composited screen including all panes,
/// floating windows, and UI elements. Applies selection highlighting
/// using the same logic as the VTE serialization path.
pub fn chunks_to_frame_store(
    chunks: &[CharacterChunk],
    cols: usize,
    rows: usize,
    style_table: &mut StyleTable,
) -> FrameStore {
    let mut store = FrameStore::new(cols, rows);

    for chunk in chunks {
        let chunk_y = chunk.y;
        if chunk_y >= rows {
            continue;
        }

        let selection_and_colors = chunk.selection_and_colors();

        let mut col = chunk.x;
        for tc in &chunk.terminal_characters {
            if col >= cols {
                break;
            }

            let adjusted_styles =
                apply_selection_styling(&selection_and_colors, *tc.styles, chunk_y, col);
            let cell = character_styles_to_cell(tc.character, tc.width(), &adjusted_styles, style_table);
            let width = tc.width();

            store.update_row(chunk_y, |row| {
                row.set_cell(col, cell.clone());
            });

            for offset in 1..width {
                if col + offset >= cols {
                    break;
                }
                let continuation_cell = Cell {
                    codepoint: 0,
                    width: 0,
                    style_id: cell.style_id,
                };
                store.update_row(chunk_y, |row| {
                    row.set_cell(col + offset, continuation_cell);
                });
            }

            col += width;
        }
    }

    store.advance_state();
    store
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panes::terminal_character::TerminalCharacter;

    #[test]
    fn test_empty_chunks() {
        let mut style_table = StyleTable::new();
        let store = chunks_to_frame_store(&[], 80, 24, &mut style_table);
        assert_eq!(store.current_state_id(), 1);
    }

    #[test]
    fn test_single_character_chunk() {
        let mut style_table = StyleTable::new();
        let tc = TerminalCharacter::new('A');
        let chunk = CharacterChunk::new(vec![tc], 5, 3);

        let store = chunks_to_frame_store(&[chunk], 80, 24, &mut style_table);

        let frame = store.current_frame();
        let cell = frame.rows[3].get_cell(5).unwrap();
        assert_eq!(cell.codepoint, 'A' as u32);
    }

    #[test]
    fn test_multiple_characters_in_chunk() {
        let mut style_table = StyleTable::new();
        let chars: Vec<TerminalCharacter> = "Hello"
            .chars()
            .map(TerminalCharacter::new)
            .collect();
        let chunk = CharacterChunk::new(chars, 10, 5);

        let store = chunks_to_frame_store(&[chunk], 80, 24, &mut style_table);

        let frame = store.current_frame();
        assert_eq!(frame.rows[5].get_cell(10).unwrap().codepoint, 'H' as u32);
        assert_eq!(frame.rows[5].get_cell(11).unwrap().codepoint, 'e' as u32);
        assert_eq!(frame.rows[5].get_cell(12).unwrap().codepoint, 'l' as u32);
        assert_eq!(frame.rows[5].get_cell(13).unwrap().codepoint, 'l' as u32);
        assert_eq!(frame.rows[5].get_cell(14).unwrap().codepoint, 'o' as u32);
    }

    #[test]
    fn test_wide_character_chunk() {
        let mut style_table = StyleTable::new();
        let tc = TerminalCharacter::new('中');
        let chunk = CharacterChunk::new(vec![tc], 5, 3);

        let store = chunks_to_frame_store(&[chunk], 80, 24, &mut style_table);

        let frame = store.current_frame();
        let main_cell = frame.rows[3].get_cell(5).unwrap();
        assert_eq!(main_cell.codepoint, '中' as u32);
        assert_eq!(main_cell.width, 2);

        let continuation_cell = frame.rows[3].get_cell(6).unwrap();
        assert_eq!(continuation_cell.codepoint, 0);
        assert_eq!(continuation_cell.width, 0);
        assert_eq!(continuation_cell.style_id, main_cell.style_id);
    }

    #[test]
    fn test_chunk_at_right_edge() {
        let mut style_table = StyleTable::new();
        let chars: Vec<TerminalCharacter> = "Test"
            .chars()
            .map(TerminalCharacter::new)
            .collect();
        let chunk = CharacterChunk::new(chars, 78, 0);

        let store = chunks_to_frame_store(&[chunk], 80, 24, &mut style_table);

        let frame = store.current_frame();
        assert_eq!(frame.rows[0].get_cell(78).unwrap().codepoint, 'T' as u32);
        assert_eq!(frame.rows[0].get_cell(79).unwrap().codepoint, 'e' as u32);
    }

    #[test]
    fn test_chunk_outside_visible_area() {
        let mut style_table = StyleTable::new();
        let tc = TerminalCharacter::new('X');
        let chunk = CharacterChunk::new(vec![tc], 5, 100);

        let store = chunks_to_frame_store(&[chunk], 80, 24, &mut style_table);

        assert_eq!(store.current_state_id(), 1);
    }

    #[test]
    fn test_multiple_chunks() {
        let mut style_table = StyleTable::new();
        let chunk1 = CharacterChunk::new(
            vec![TerminalCharacter::new('A')],
            0,
            0,
        );
        let chunk2 = CharacterChunk::new(
            vec![TerminalCharacter::new('B')],
            10,
            5,
        );
        let chunk3 = CharacterChunk::new(
            vec![TerminalCharacter::new('C')],
            20,
            10,
        );

        let store = chunks_to_frame_store(&[chunk1, chunk2, chunk3], 80, 24, &mut style_table);

        let frame = store.current_frame();
        assert_eq!(frame.rows[0].get_cell(0).unwrap().codepoint, 'A' as u32);
        assert_eq!(frame.rows[5].get_cell(10).unwrap().codepoint, 'B' as u32);
        assert_eq!(frame.rows[10].get_cell(20).unwrap().codepoint, 'C' as u32);
    }

    #[test]
    fn test_overlapping_chunks() {
        let mut style_table = StyleTable::new();
        let chunk1 = CharacterChunk::new(
            vec![TerminalCharacter::new('X')],
            5,
            3,
        );
        let chunk2 = CharacterChunk::new(
            vec![TerminalCharacter::new('Y')],
            5,
            3,
        );

        let store = chunks_to_frame_store(&[chunk1, chunk2], 80, 24, &mut style_table);

        let frame = store.current_frame();
        assert_eq!(frame.rows[3].get_cell(5).unwrap().codepoint, 'Y' as u32);
    }

    #[test]
    fn test_wide_char_at_edge_truncated() {
        let mut style_table = StyleTable::new();
        let tc = TerminalCharacter::new('中');
        let chunk = CharacterChunk::new(vec![tc], 79, 0);

        let store = chunks_to_frame_store(&[chunk], 80, 24, &mut style_table);

        let frame = store.current_frame();
        let cell = frame.rows[0].get_cell(79).unwrap();
        assert_eq!(cell.codepoint, '中' as u32);
    }
}
