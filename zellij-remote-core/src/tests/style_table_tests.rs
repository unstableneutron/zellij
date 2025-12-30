use crate::style_table::StyleTable;
use zellij_remote_protocol::{Color, Rgb, Style};

fn make_style(fg_r: u8, fg_g: u8, fg_b: u8) -> Style {
    Style {
        fg: Some(Color {
            value: Some(zellij_remote_protocol::color::Value::Rgb(Rgb {
                r: fg_r as u32,
                g: fg_g as u32,
                b: fg_b as u32,
            })),
        }),
        bg: None,
        bold: false,
        dim: false,
        italic: false,
        reverse: false,
        hidden: false,
        strike: false,
        blink_slow: false,
        blink_fast: false,
        underline: 0,
        underline_color: None,
    }
}

#[test]
fn test_get_or_insert_new_style() {
    let mut table = StyleTable::new();

    let style = make_style(255, 0, 0);
    let id1 = table.get_or_insert(&style);

    // First style gets ID 1 (0 is reserved for default)
    assert_eq!(id1, 1);
}

#[test]
fn test_get_or_insert_existing_style() {
    let mut table = StyleTable::new();

    let style = make_style(255, 0, 0);
    let id1 = table.get_or_insert(&style);
    let id2 = table.get_or_insert(&style);

    // Same style should return same ID
    assert_eq!(id1, id2);
}

#[test]
fn test_different_styles_get_different_ids() {
    let mut table = StyleTable::new();

    let red = make_style(255, 0, 0);
    let green = make_style(0, 255, 0);

    let id_red = table.get_or_insert(&red);
    let id_green = table.get_or_insert(&green);

    assert_ne!(id_red, id_green);
}

#[test]
fn test_lookup_by_id() {
    let mut table = StyleTable::new();

    let style = make_style(128, 128, 128);
    let id = table.get_or_insert(&style);

    let retrieved = table.get(id);
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap(), &style);
}

#[test]
fn test_default_style_is_id_zero() {
    let table = StyleTable::new();

    // ID 0 should return default/reset style
    let default = table.get(0);
    assert!(default.is_some());
}

#[test]
fn test_styles_since_baseline() {
    let mut table = StyleTable::new();

    // Add some styles
    let s1 = make_style(1, 0, 0);
    let s2 = make_style(2, 0, 0);
    table.get_or_insert(&s1);

    let baseline = table.current_count();

    table.get_or_insert(&s2);
    let s3 = make_style(3, 0, 0);
    table.get_or_insert(&s3);

    // Get styles added since baseline
    let new_styles = table.styles_since(baseline);
    assert_eq!(new_styles.len(), 2);
}
