use std::collections::BTreeSet;

use zellij_remote_protocol::{input_event, key_event, InputEvent, KeyModifiers, SpecialKey};
use zellij_utils::data::{BareKey, KeyModifier, KeyWithModifier};
use zellij_utils::input::actions::Action;

pub fn translate_input(event: &InputEvent) -> Option<Action> {
    match &event.payload {
        Some(input_event::Payload::TextUtf8(bytes)) => Some(Action::Write {
            key_with_modifier: None,
            bytes: bytes.clone(),
            is_kitty_keyboard_protocol: false,
        }),
        Some(input_event::Payload::Key(key_event)) => translate_key_event(key_event),
        Some(input_event::Payload::RawBytes(bytes)) => Some(Action::Write {
            key_with_modifier: None,
            bytes: bytes.clone(),
            is_kitty_keyboard_protocol: false,
        }),
        Some(input_event::Payload::Mouse(_mouse_event)) => {
            // TODO: Mouse event translation
            None
        },
        None => None,
    }
}

fn translate_key_event(key: &zellij_remote_protocol::KeyEvent) -> Option<Action> {
    let key_with_modifier = match &key.key {
        Some(key_event::Key::UnicodeScalar(codepoint)) => {
            let ch = char::from_u32(*codepoint)?;
            let bare_key = BareKey::Char(ch);
            let modifiers = translate_modifiers(key.modifiers.as_ref());
            KeyWithModifier {
                bare_key,
                key_modifiers: modifiers,
            }
        },
        Some(key_event::Key::Special(special)) => {
            let bare_key = translate_special_key(*special)?;
            let modifiers = translate_modifiers(key.modifiers.as_ref());
            KeyWithModifier {
                bare_key,
                key_modifiers: modifiers,
            }
        },
        None => return None,
    };

    let bytes = key_to_bytes(&key_with_modifier);

    Some(Action::Write {
        key_with_modifier: Some(key_with_modifier),
        bytes,
        is_kitty_keyboard_protocol: false,
    })
}

fn translate_modifiers(mods: Option<&KeyModifiers>) -> BTreeSet<KeyModifier> {
    let mut result = BTreeSet::new();
    if let Some(mods) = mods {
        let bits = mods.bits;
        if bits & 1 != 0 {
            result.insert(KeyModifier::Shift);
        }
        if bits & 2 != 0 {
            result.insert(KeyModifier::Alt);
        }
        if bits & 4 != 0 {
            result.insert(KeyModifier::Ctrl);
        }
        if bits & 8 != 0 {
            result.insert(KeyModifier::Super);
        }
    }
    result
}

fn translate_special_key(special: i32) -> Option<BareKey> {
    match special {
        x if x == SpecialKey::Unspecified as i32 => None,
        x if x == SpecialKey::Enter as i32 => Some(BareKey::Enter),
        x if x == SpecialKey::Escape as i32 => Some(BareKey::Esc),
        x if x == SpecialKey::Backspace as i32 => Some(BareKey::Backspace),
        x if x == SpecialKey::Tab as i32 => Some(BareKey::Tab),
        x if x == SpecialKey::Left as i32 => Some(BareKey::Left),
        x if x == SpecialKey::Right as i32 => Some(BareKey::Right),
        x if x == SpecialKey::Up as i32 => Some(BareKey::Up),
        x if x == SpecialKey::Down as i32 => Some(BareKey::Down),
        x if x == SpecialKey::Home as i32 => Some(BareKey::Home),
        x if x == SpecialKey::End as i32 => Some(BareKey::End),
        x if x == SpecialKey::PageUp as i32 => Some(BareKey::PageUp),
        x if x == SpecialKey::PageDown as i32 => Some(BareKey::PageDown),
        x if x == SpecialKey::Insert as i32 => Some(BareKey::Insert),
        x if x == SpecialKey::Delete as i32 => Some(BareKey::Delete),
        x if x == SpecialKey::F1 as i32 => Some(BareKey::F(1)),
        x if x == SpecialKey::F2 as i32 => Some(BareKey::F(2)),
        x if x == SpecialKey::F3 as i32 => Some(BareKey::F(3)),
        x if x == SpecialKey::F4 as i32 => Some(BareKey::F(4)),
        x if x == SpecialKey::F5 as i32 => Some(BareKey::F(5)),
        x if x == SpecialKey::F6 as i32 => Some(BareKey::F(6)),
        x if x == SpecialKey::F7 as i32 => Some(BareKey::F(7)),
        x if x == SpecialKey::F8 as i32 => Some(BareKey::F(8)),
        x if x == SpecialKey::F9 as i32 => Some(BareKey::F(9)),
        x if x == SpecialKey::F10 as i32 => Some(BareKey::F(10)),
        x if x == SpecialKey::F11 as i32 => Some(BareKey::F(11)),
        x if x == SpecialKey::F12 as i32 => Some(BareKey::F(12)),
        _ => None,
    }
}

fn key_to_bytes(key: &KeyWithModifier) -> Vec<u8> {
    let has_ctrl = key.key_modifiers.contains(&KeyModifier::Ctrl);

    match &key.bare_key {
        BareKey::Char(c) => {
            if has_ctrl && c.is_ascii_alphabetic() {
                vec![(c.to_ascii_lowercase() as u8) - b'a' + 1]
            } else {
                let mut s = String::new();
                s.push(*c);
                s.into_bytes()
            }
        },
        BareKey::Enter => vec![b'\r'],
        BareKey::Tab => vec![b'\t'],
        BareKey::Backspace => vec![0x7f],
        BareKey::Esc => vec![0x1b],
        BareKey::Left => b"\x1b[D".to_vec(),
        BareKey::Right => b"\x1b[C".to_vec(),
        BareKey::Up => b"\x1b[A".to_vec(),
        BareKey::Down => b"\x1b[B".to_vec(),
        BareKey::Home => b"\x1b[H".to_vec(),
        BareKey::End => b"\x1b[F".to_vec(),
        BareKey::PageUp => b"\x1b[5~".to_vec(),
        BareKey::PageDown => b"\x1b[6~".to_vec(),
        BareKey::Insert => b"\x1b[2~".to_vec(),
        BareKey::Delete => b"\x1b[3~".to_vec(),
        BareKey::F(n) => match n {
            1 => b"\x1bOP".to_vec(),
            2 => b"\x1bOQ".to_vec(),
            3 => b"\x1bOR".to_vec(),
            4 => b"\x1bOS".to_vec(),
            5 => b"\x1b[15~".to_vec(),
            6 => b"\x1b[17~".to_vec(),
            7 => b"\x1b[18~".to_vec(),
            8 => b"\x1b[19~".to_vec(),
            9 => b"\x1b[20~".to_vec(),
            10 => b"\x1b[21~".to_vec(),
            11 => b"\x1b[23~".to_vec(),
            12 => b"\x1b[24~".to_vec(),
            _ => vec![],
        },
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zellij_remote_protocol::KeyEvent;

    #[test]
    fn test_translate_text_utf8() {
        let event = InputEvent {
            input_seq: 1,
            client_time_ms: 0,
            payload: Some(input_event::Payload::TextUtf8(b"hello".to_vec())),
        };

        let action = translate_input(&event).unwrap();
        match action {
            Action::Write { bytes, .. } => {
                assert_eq!(bytes, b"hello".to_vec());
            },
            _ => panic!("Expected Write action"),
        }
    }

    #[test]
    fn test_translate_unicode_key() {
        let event = InputEvent {
            input_seq: 1,
            client_time_ms: 0,
            payload: Some(input_event::Payload::Key(KeyEvent {
                modifiers: None,
                key: Some(key_event::Key::UnicodeScalar('a' as u32)),
            })),
        };

        let action = translate_input(&event).unwrap();
        match action {
            Action::Write {
                key_with_modifier,
                bytes,
                ..
            } => {
                assert!(key_with_modifier.is_some());
                assert_eq!(bytes, vec![b'a']);
            },
            _ => panic!("Expected Write action"),
        }
    }

    #[test]
    fn test_translate_special_key_enter() {
        let event = InputEvent {
            input_seq: 1,
            client_time_ms: 0,
            payload: Some(input_event::Payload::Key(KeyEvent {
                modifiers: None,
                key: Some(key_event::Key::Special(SpecialKey::Enter as i32)),
            })),
        };

        let action = translate_input(&event).unwrap();
        match action {
            Action::Write { bytes, .. } => {
                assert_eq!(bytes, vec![b'\r']);
            },
            _ => panic!("Expected Write action"),
        }
    }

    #[test]
    fn test_translate_ctrl_c() {
        let event = InputEvent {
            input_seq: 1,
            client_time_ms: 0,
            payload: Some(input_event::Payload::Key(KeyEvent {
                modifiers: Some(KeyModifiers { bits: 4 }), // Ctrl
                key: Some(key_event::Key::UnicodeScalar('c' as u32)),
            })),
        };

        let action = translate_input(&event).unwrap();
        match action {
            Action::Write { bytes, .. } => {
                assert_eq!(bytes, vec![0x03]); // Ctrl+C = 0x03
            },
            _ => panic!("Expected Write action"),
        }
    }
}
