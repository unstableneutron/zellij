use prost::Message;

use crate::proto::*;

// =============================================================================
// HANDSHAKE ROUNDTRIPS
// =============================================================================

#[test]
fn test_protocol_version_roundtrip() {
    let original = ProtocolVersion { major: 1, minor: 2 };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = ProtocolVersion::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_capabilities_roundtrip() {
    let original = Capabilities {
        supports_datagrams: true,
        max_datagram_bytes: 1200,
        supports_style_dictionary: true,
        supports_styled_underlines: true,
        supports_prediction: false,
        supports_images: true,
        supports_clipboard: true,
        supports_hyperlinks: false,
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = Capabilities::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_capabilities_all_false() {
    let original = Capabilities {
        supports_datagrams: false,
        max_datagram_bytes: 0,
        supports_style_dictionary: false,
        supports_styled_underlines: false,
        supports_prediction: false,
        supports_images: false,
        supports_clipboard: false,
        supports_hyperlinks: false,
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = Capabilities::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_capabilities_all_true() {
    let original = Capabilities {
        supports_datagrams: true,
        max_datagram_bytes: u32::MAX,
        supports_style_dictionary: true,
        supports_styled_underlines: true,
        supports_prediction: true,
        supports_images: true,
        supports_clipboard: true,
        supports_hyperlinks: true,
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = Capabilities::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_client_hello_roundtrip() {
    let original = ClientHello {
        version: Some(ProtocolVersion { major: 1, minor: 0 }),
        capabilities: Some(Capabilities {
            supports_datagrams: true,
            max_datagram_bytes: 1200,
            supports_style_dictionary: true,
            supports_styled_underlines: true,
            supports_prediction: false,
            supports_images: false,
            supports_clipboard: true,
            supports_hyperlinks: false,
        }),
        client_name: "ios".to_string(),
        bearer_token: vec![0x01, 0x02, 0x03, 0x04],
        resume_token: vec![0xAA, 0xBB],
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = ClientHello::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_client_hello_empty_fields() {
    let original = ClientHello {
        version: None,
        capabilities: None,
        client_name: String::new(),
        bearer_token: vec![],
        resume_token: vec![],
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = ClientHello::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_server_hello_roundtrip() {
    let original = ServerHello {
        negotiated_version: Some(ProtocolVersion { major: 1, minor: 0 }),
        negotiated_capabilities: Some(Capabilities {
            supports_datagrams: true,
            max_datagram_bytes: 1200,
            supports_style_dictionary: true,
            supports_styled_underlines: false,
            supports_prediction: false,
            supports_images: false,
            supports_clipboard: false,
            supports_hyperlinks: false,
        }),
        client_id: 12345,
        session_name: "my-session".to_string(),
        session_state: SessionState::Running as i32,
        lease: Some(ControllerLease {
            lease_id: 1,
            owner_client_id: 12345,
            policy: ControllerPolicy::LastWriterWins as i32,
            current_size: Some(DisplaySize { cols: 80, rows: 24 }),
            remaining_ms: 30000,
            duration_ms: 60000,
        }),
        resume_token: vec![0x11, 0x22, 0x33],
        snapshot_interval_ms: 5000,
        max_inflight_inputs: 16,
        render_window: 4,
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = ServerHello::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_server_hello_all_session_states() {
    for state in [
        SessionState::Unspecified,
        SessionState::Running,
        SessionState::Created,
        SessionState::Resurrected,
    ] {
        let original = ServerHello {
            negotiated_version: None,
            negotiated_capabilities: None,
            client_id: 1,
            session_name: String::new(),
            session_state: state as i32,
            lease: None,
            resume_token: vec![],
            snapshot_interval_ms: 0,
            max_inflight_inputs: 0,
            render_window: 0,
        };
        let mut buf = Vec::new();
        original.encode(&mut buf).unwrap();
        let decoded = ServerHello::decode(&buf[..]).unwrap();
        assert_eq!(original, decoded);
    }
}

// =============================================================================
// ATTACH FLOW ROUNDTRIPS
// =============================================================================

#[test]
fn test_attach_request_roundtrip() {
    let original = AttachRequest {
        mode: AttachMode::Resume as i32,
        last_applied_state_id: 100,
        last_acked_input_seq: 50,
        desired_role: ClientRole::Controller as i32,
        desired_size: Some(DisplaySize {
            cols: 120,
            rows: 40,
        }),
        read_only: false,
        force_snapshot: false,
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = AttachRequest::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_attach_request_all_modes() {
    for mode in [
        AttachMode::Unspecified,
        AttachMode::Resume,
        AttachMode::Fresh,
    ] {
        let original = AttachRequest {
            mode: mode as i32,
            last_applied_state_id: 0,
            last_acked_input_seq: 0,
            desired_role: ClientRole::Viewer as i32,
            desired_size: None,
            read_only: true,
            force_snapshot: true,
        };
        let mut buf = Vec::new();
        original.encode(&mut buf).unwrap();
        let decoded = AttachRequest::decode(&buf[..]).unwrap();
        assert_eq!(original, decoded);
    }
}

#[test]
fn test_attach_response_roundtrip() {
    let original = AttachResponse {
        ok: true,
        error_message: String::new(),
        lease: Some(ControllerLease {
            lease_id: 42,
            owner_client_id: 1,
            policy: ControllerPolicy::ExplicitOnly as i32,
            current_size: Some(DisplaySize { cols: 80, rows: 24 }),
            remaining_ms: 10000,
            duration_ms: 30000,
        }),
        current_state_id: 999,
        will_send_snapshot: true,
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = AttachResponse::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_attach_response_error() {
    let original = AttachResponse {
        ok: false,
        error_message: "Session not found".to_string(),
        lease: None,
        current_state_id: 0,
        will_send_snapshot: false,
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = AttachResponse::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

// =============================================================================
// CONTROLLER LEASE ROUNDTRIPS
// =============================================================================

#[test]
fn test_controller_lease_roundtrip() {
    let original = ControllerLease {
        lease_id: u64::MAX,
        owner_client_id: u64::MAX,
        policy: ControllerPolicy::LastWriterWins as i32,
        current_size: Some(DisplaySize {
            cols: u32::MAX,
            rows: u32::MAX,
        }),
        remaining_ms: u32::MAX,
        duration_ms: u32::MAX,
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = ControllerLease::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_request_control_roundtrip() {
    let original = RequestControl {
        reason: "User resize".to_string(),
        desired_size: Some(DisplaySize {
            cols: 200,
            rows: 50,
        }),
        force: true,
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = RequestControl::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_grant_control_roundtrip() {
    let original = GrantControl {
        lease: Some(ControllerLease {
            lease_id: 1,
            owner_client_id: 2,
            policy: ControllerPolicy::ExplicitOnly as i32,
            current_size: Some(DisplaySize { cols: 80, rows: 24 }),
            remaining_ms: 5000,
            duration_ms: 10000,
        }),
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = GrantControl::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_deny_control_roundtrip() {
    let original = DenyControl {
        reason: "Already controlled".to_string(),
        lease: Some(ControllerLease {
            lease_id: 99,
            owner_client_id: 42,
            policy: ControllerPolicy::ExplicitOnly as i32,
            current_size: Some(DisplaySize { cols: 80, rows: 24 }),
            remaining_ms: 1000,
            duration_ms: 30000,
        }),
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = DenyControl::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_release_control_roundtrip() {
    let original = ReleaseControl { lease_id: 12345 };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = ReleaseControl::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_set_controller_size_roundtrip() {
    let original = SetControllerSize {
        size: Some(DisplaySize {
            cols: 132,
            rows: 43,
        }),
        request_snapshot: true,
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = SetControllerSize::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_keep_alive_lease_roundtrip() {
    let original = KeepAliveLease {
        lease_id: 999,
        client_time_ms: 123456,
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = KeepAliveLease::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_lease_revoked_roundtrip() {
    let original = LeaseRevoked {
        lease_id: 42,
        reason: "timeout".to_string(),
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = LeaseRevoked::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

// =============================================================================
// INPUT ROUNDTRIPS
// =============================================================================

#[test]
fn test_key_event_unicode_roundtrip() {
    let original = KeyEvent {
        modifiers: Some(KeyModifiers { bits: 5 }), // SHIFT | CTRL
        key: Some(key_event::Key::UnicodeScalar(0x1F600)),
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = KeyEvent::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_key_event_special_roundtrip() {
    let original = KeyEvent {
        modifiers: Some(KeyModifiers { bits: 2 }), // ALT
        key: Some(key_event::Key::Special(SpecialKey::F12 as i32)),
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = KeyEvent::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_key_event_all_special_keys() {
    let special_keys = [
        SpecialKey::Unspecified,
        SpecialKey::Enter,
        SpecialKey::Escape,
        SpecialKey::Backspace,
        SpecialKey::Tab,
        SpecialKey::Left,
        SpecialKey::Right,
        SpecialKey::Up,
        SpecialKey::Down,
        SpecialKey::Home,
        SpecialKey::End,
        SpecialKey::PageUp,
        SpecialKey::PageDown,
        SpecialKey::Insert,
        SpecialKey::Delete,
        SpecialKey::F1,
        SpecialKey::F2,
        SpecialKey::F3,
        SpecialKey::F4,
        SpecialKey::F5,
        SpecialKey::F6,
        SpecialKey::F7,
        SpecialKey::F8,
        SpecialKey::F9,
        SpecialKey::F10,
        SpecialKey::F11,
        SpecialKey::F12,
    ];
    for key in special_keys {
        let original = KeyEvent {
            modifiers: None,
            key: Some(key_event::Key::Special(key as i32)),
        };
        let mut buf = Vec::new();
        original.encode(&mut buf).unwrap();
        let decoded = KeyEvent::decode(&buf[..]).unwrap();
        assert_eq!(original, decoded);
    }
}

#[test]
fn test_mouse_event_roundtrip() {
    let original = MouseEvent {
        kind: MouseKind::Down as i32,
        col: 40,
        row: 12,
        button: MouseButton::Left as i32,
        scroll_delta: 0,
        modifiers: Some(KeyModifiers { bits: 0 }),
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = MouseEvent::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_mouse_event_scroll_roundtrip() {
    let original = MouseEvent {
        kind: MouseKind::Scroll as i32,
        col: 0,
        row: 0,
        button: MouseButton::Unspecified as i32,
        scroll_delta: -3,
        modifiers: Some(KeyModifiers { bits: 4 }), // CTRL
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = MouseEvent::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_input_event_text_roundtrip() {
    let original = InputEvent {
        input_seq: 42,
        client_time_ms: 1000,
        payload: Some(input_event::Payload::TextUtf8(
            "Hello, 世界!".as_bytes().to_vec(),
        )),
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = InputEvent::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_input_event_key_roundtrip() {
    let original = InputEvent {
        input_seq: 100,
        client_time_ms: 2000,
        payload: Some(input_event::Payload::Key(KeyEvent {
            modifiers: Some(KeyModifiers { bits: 1 }),
            key: Some(key_event::Key::UnicodeScalar('a' as u32)),
        })),
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = InputEvent::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_input_event_raw_bytes_roundtrip() {
    let original = InputEvent {
        input_seq: 200,
        client_time_ms: 3000,
        payload: Some(input_event::Payload::RawBytes(vec![0x1b, 0x5b, 0x41])), // ESC [ A
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = InputEvent::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_input_event_mouse_roundtrip() {
    let original = InputEvent {
        input_seq: 300,
        client_time_ms: 4000,
        payload: Some(input_event::Payload::Mouse(MouseEvent {
            kind: MouseKind::Move as i32,
            col: 50,
            row: 20,
            button: MouseButton::Unspecified as i32,
            scroll_delta: 0,
            modifiers: None,
        })),
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = InputEvent::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_input_ack_roundtrip() {
    let original = InputAck {
        acked_seq: 999,
        rtt_sample_seq: 998,
        echoed_client_time_ms: 12345,
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = InputAck::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

// =============================================================================
// RENDER ROUNDTRIPS
// =============================================================================

#[test]
fn test_display_size_roundtrip() {
    let original = DisplaySize { cols: 80, rows: 24 };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = DisplaySize::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_color_default_roundtrip() {
    let original = Color {
        value: Some(color::Value::DefaultColor(DefaultColor {})),
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = Color::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_color_ansi256_roundtrip() {
    let original = Color {
        value: Some(color::Value::Ansi256(196)),
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = Color::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_color_rgb_roundtrip() {
    let original = Color {
        value: Some(color::Value::Rgb(Rgb {
            r: 255,
            g: 128,
            b: 64,
        })),
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = Color::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_style_roundtrip() {
    let original = Style {
        fg: Some(Color {
            value: Some(color::Value::Rgb(Rgb {
                r: 255,
                g: 255,
                b: 255,
            })),
        }),
        bg: Some(Color {
            value: Some(color::Value::Ansi256(0)),
        }),
        bold: true,
        dim: false,
        italic: true,
        reverse: false,
        hidden: false,
        strike: true,
        blink_slow: false,
        blink_fast: false,
        underline: UnderlineStyle::Curly as i32,
        underline_color: Some(Color {
            value: Some(color::Value::Rgb(Rgb { r: 255, g: 0, b: 0 })),
        }),
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = Style::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_style_all_underline_styles() {
    let styles = [
        UnderlineStyle::Unspecified,
        UnderlineStyle::None,
        UnderlineStyle::Single,
        UnderlineStyle::Double,
        UnderlineStyle::Dotted,
        UnderlineStyle::Dashed,
        UnderlineStyle::Curly,
    ];
    for underline in styles {
        let original = Style {
            fg: None,
            bg: None,
            bold: false,
            dim: false,
            italic: false,
            reverse: false,
            hidden: false,
            strike: false,
            blink_slow: false,
            blink_fast: false,
            underline: underline as i32,
            underline_color: None,
        };
        let mut buf = Vec::new();
        original.encode(&mut buf).unwrap();
        let decoded = Style::decode(&buf[..]).unwrap();
        assert_eq!(original, decoded);
    }
}

#[test]
fn test_style_all_boolean_combinations() {
    for bits in 0..=255u8 {
        let original = Style {
            fg: None,
            bg: None,
            bold: bits & 1 != 0,
            dim: bits & 2 != 0,
            italic: bits & 4 != 0,
            reverse: bits & 8 != 0,
            hidden: bits & 16 != 0,
            strike: bits & 32 != 0,
            blink_slow: bits & 64 != 0,
            blink_fast: bits & 128 != 0,
            underline: UnderlineStyle::None as i32,
            underline_color: None,
        };
        let mut buf = Vec::new();
        original.encode(&mut buf).unwrap();
        let decoded = Style::decode(&buf[..]).unwrap();
        assert_eq!(original, decoded);
    }
}

#[test]
fn test_cursor_state_roundtrip() {
    let original = CursorState {
        row: 10,
        col: 20,
        visible: true,
        blink: true,
        shape: CursorShape::Beam as i32,
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = CursorState::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_cursor_state_all_shapes() {
    for shape in [
        CursorShape::Unspecified,
        CursorShape::Block,
        CursorShape::Beam,
        CursorShape::Underline,
    ] {
        let original = CursorState {
            row: 0,
            col: 0,
            visible: false,
            blink: false,
            shape: shape as i32,
        };
        let mut buf = Vec::new();
        original.encode(&mut buf).unwrap();
        let decoded = CursorState::decode(&buf[..]).unwrap();
        assert_eq!(original, decoded);
    }
}

#[test]
fn test_row_data_roundtrip() {
    let original = RowData {
        row: 5,
        codepoints: vec!['H' as u32, 'e' as u32, 'l' as u32, 'l' as u32, 'o' as u32],
        widths: vec![1, 1, 1, 1, 1],
        style_ids: vec![0, 0, 1, 1, 0],
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = RowData::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_row_data_large_vectors() {
    let size = 1000;
    let original = RowData {
        row: 0,
        codepoints: (0..size).map(|i| ('A' as u32) + (i % 26)).collect(),
        widths: vec![1; size as usize],
        style_ids: (0..size).collect(),
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = RowData::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_row_data_empty_vectors() {
    let original = RowData {
        row: 0,
        codepoints: vec![],
        widths: vec![],
        style_ids: vec![],
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = RowData::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_cell_run_roundtrip() {
    let original = CellRun {
        col_start: 10,
        codepoints: vec!['W' as u32, 'o' as u32, 'r' as u32, 'l' as u32, 'd' as u32],
        widths: vec![1, 1, 1, 1, 1],
        style_ids: vec![2, 2, 2, 2, 2],
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = CellRun::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_row_patch_roundtrip() {
    let original = RowPatch {
        row: 3,
        runs: vec![
            CellRun {
                col_start: 0,
                codepoints: vec!['>' as u32, ' ' as u32],
                widths: vec![1, 1],
                style_ids: vec![1, 0],
            },
            CellRun {
                col_start: 10,
                codepoints: vec!['$' as u32],
                widths: vec![1],
                style_ids: vec![2],
            },
        ],
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = RowPatch::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_screen_delta_roundtrip() {
    let original = ScreenDelta {
        base_state_id: 100,
        state_id: 101,
        styles_added: vec![StyleDef {
            style_id: 5,
            style: Some(Style {
                fg: Some(Color {
                    value: Some(color::Value::Ansi256(1)),
                }),
                bg: None,
                bold: true,
                dim: false,
                italic: false,
                reverse: false,
                hidden: false,
                strike: false,
                blink_slow: false,
                blink_fast: false,
                underline: UnderlineStyle::None as i32,
                underline_color: None,
            }),
        }],
        row_patches: vec![RowPatch {
            row: 0,
            runs: vec![CellRun {
                col_start: 0,
                codepoints: vec!['X' as u32],
                widths: vec![1],
                style_ids: vec![5],
            }],
        }],
        cursor: Some(CursorState {
            row: 0,
            col: 1,
            visible: true,
            blink: false,
            shape: CursorShape::Block as i32,
        }),
        delivered_input_watermark: 50,
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = ScreenDelta::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_screen_delta_empty() {
    let original = ScreenDelta {
        base_state_id: 0,
        state_id: 1,
        styles_added: vec![],
        row_patches: vec![],
        cursor: None,
        delivered_input_watermark: 0,
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = ScreenDelta::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_screen_snapshot_roundtrip() {
    let original = ScreenSnapshot {
        state_id: 500,
        size: Some(DisplaySize { cols: 80, rows: 24 }),
        style_table_reset: true,
        styles: vec![StyleDef {
            style_id: 0,
            style: Some(Style {
                fg: Some(Color {
                    value: Some(color::Value::DefaultColor(DefaultColor {})),
                }),
                bg: Some(Color {
                    value: Some(color::Value::DefaultColor(DefaultColor {})),
                }),
                bold: false,
                dim: false,
                italic: false,
                reverse: false,
                hidden: false,
                strike: false,
                blink_slow: false,
                blink_fast: false,
                underline: UnderlineStyle::None as i32,
                underline_color: None,
            }),
        }],
        rows: vec![RowData {
            row: 0,
            codepoints: vec![' ' as u32; 80],
            widths: vec![1; 80],
            style_ids: vec![0; 80],
        }],
        cursor: Some(CursorState {
            row: 0,
            col: 0,
            visible: true,
            blink: true,
            shape: CursorShape::Block as i32,
        }),
        delivered_input_watermark: 100,
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = ScreenSnapshot::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_screen_snapshot_large() {
    let cols = 200u32;
    let row_count = 50;
    let original = ScreenSnapshot {
        state_id: 1000,
        size: Some(DisplaySize {
            cols,
            rows: row_count,
        }),
        style_table_reset: true,
        styles: (0..10)
            .map(|i| StyleDef {
                style_id: i,
                style: Some(Style {
                    fg: Some(Color {
                        value: Some(color::Value::Ansi256(i)),
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
                    underline: UnderlineStyle::None as i32,
                    underline_color: None,
                }),
            })
            .collect(),
        rows: (0..row_count)
            .map(|r| RowData {
                row: r,
                codepoints: vec!['.' as u32; cols as usize],
                widths: vec![1; cols as usize],
                style_ids: vec![0; cols as usize],
            })
            .collect(),
        cursor: Some(CursorState {
            row: 25,
            col: 100,
            visible: true,
            blink: false,
            shape: CursorShape::Underline as i32,
        }),
        delivered_input_watermark: 999,
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = ScreenSnapshot::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_state_ack_roundtrip() {
    let original = StateAck {
        last_applied_state_id: 100,
        last_received_state_id: 105,
        client_time_ms: 50000,
        estimated_loss_ppm: 1000,
        srtt_ms: 50,
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = StateAck::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

// =============================================================================
// RESYNC & ERRORS
// =============================================================================

#[test]
fn test_request_snapshot_roundtrip() {
    let original = RequestSnapshot {
        reason: request_snapshot::Reason::BaseMismatch as i32,
        known_state_id: 42,
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = RequestSnapshot::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_request_snapshot_all_reasons() {
    for reason in [
        request_snapshot::Reason::Unspecified,
        request_snapshot::Reason::BaseMismatch,
        request_snapshot::Reason::Periodic,
        request_snapshot::Reason::DecodeError,
        request_snapshot::Reason::UserRequest,
    ] {
        let original = RequestSnapshot {
            reason: reason as i32,
            known_state_id: 0,
        };
        let mut buf = Vec::new();
        original.encode(&mut buf).unwrap();
        let decoded = RequestSnapshot::decode(&buf[..]).unwrap();
        assert_eq!(original, decoded);
    }
}

#[test]
fn test_protocol_error_roundtrip() {
    let original = ProtocolError {
        code: protocol_error::Code::Unauthorized as i32,
        message: "Invalid token".to_string(),
        fatal: true,
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = ProtocolError::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_protocol_error_all_codes() {
    for code in [
        protocol_error::Code::Unspecified,
        protocol_error::Code::Unauthorized,
        protocol_error::Code::BadVersion,
        protocol_error::Code::BadMessage,
        protocol_error::Code::FlowControl,
        protocol_error::Code::SessionNotFound,
        protocol_error::Code::LeaseDenied,
        protocol_error::Code::Internal,
    ] {
        let original = ProtocolError {
            code: code as i32,
            message: String::new(),
            fatal: false,
        };
        let mut buf = Vec::new();
        original.encode(&mut buf).unwrap();
        let decoded = ProtocolError::decode(&buf[..]).unwrap();
        assert_eq!(original, decoded);
    }
}

// =============================================================================
// KEEPALIVE
// =============================================================================

#[test]
fn test_ping_roundtrip() {
    let original = Ping {
        ping_id: 12345,
        client_time_ms: 99999,
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = Ping::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_pong_roundtrip() {
    let original = Pong {
        ping_id: 12345,
        echoed_client_time_ms: 99999,
        server_time_ms: 100000,
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = Pong::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_unsupported_feature_notice_roundtrip() {
    let original = UnsupportedFeatureNotice {
        feature: "images".to_string(),
        behavior: "placeholder".to_string(),
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = UnsupportedFeatureNotice::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

// =============================================================================
// STREAM ENVELOPE ONEOF TESTS
// =============================================================================

#[test]
fn test_stream_envelope_client_hello() {
    let original = StreamEnvelope {
        msg: Some(stream_envelope::Msg::ClientHello(ClientHello {
            version: Some(ProtocolVersion { major: 1, minor: 0 }),
            capabilities: None,
            client_name: "test".to_string(),
            bearer_token: vec![],
            resume_token: vec![],
        })),
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = StreamEnvelope::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_stream_envelope_server_hello() {
    let original = StreamEnvelope {
        msg: Some(stream_envelope::Msg::ServerHello(ServerHello {
            negotiated_version: Some(ProtocolVersion { major: 1, minor: 0 }),
            negotiated_capabilities: None,
            client_id: 1,
            session_name: "session".to_string(),
            session_state: SessionState::Running as i32,
            lease: None,
            resume_token: vec![],
            snapshot_interval_ms: 5000,
            max_inflight_inputs: 16,
            render_window: 4,
        })),
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = StreamEnvelope::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_stream_envelope_attach_request() {
    let original = StreamEnvelope {
        msg: Some(stream_envelope::Msg::AttachRequest(AttachRequest {
            mode: AttachMode::Fresh as i32,
            last_applied_state_id: 0,
            last_acked_input_seq: 0,
            desired_role: ClientRole::Controller as i32,
            desired_size: Some(DisplaySize { cols: 80, rows: 24 }),
            read_only: false,
            force_snapshot: true,
        })),
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = StreamEnvelope::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_stream_envelope_attach_response() {
    let original = StreamEnvelope {
        msg: Some(stream_envelope::Msg::AttachResponse(AttachResponse {
            ok: true,
            error_message: String::new(),
            lease: None,
            current_state_id: 100,
            will_send_snapshot: true,
        })),
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = StreamEnvelope::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_stream_envelope_request_control() {
    let original = StreamEnvelope {
        msg: Some(stream_envelope::Msg::RequestControl(RequestControl {
            reason: "resize".to_string(),
            desired_size: Some(DisplaySize {
                cols: 120,
                rows: 40,
            }),
            force: false,
        })),
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = StreamEnvelope::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_stream_envelope_grant_control() {
    let original = StreamEnvelope {
        msg: Some(stream_envelope::Msg::GrantControl(GrantControl {
            lease: Some(ControllerLease {
                lease_id: 1,
                owner_client_id: 1,
                policy: ControllerPolicy::ExplicitOnly as i32,
                current_size: Some(DisplaySize { cols: 80, rows: 24 }),
                remaining_ms: 30000,
                duration_ms: 60000,
            }),
        })),
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = StreamEnvelope::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_stream_envelope_deny_control() {
    let original = StreamEnvelope {
        msg: Some(stream_envelope::Msg::DenyControl(DenyControl {
            reason: "already controlled".to_string(),
            lease: None,
        })),
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = StreamEnvelope::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_stream_envelope_release_control() {
    let original = StreamEnvelope {
        msg: Some(stream_envelope::Msg::ReleaseControl(ReleaseControl {
            lease_id: 42,
        })),
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = StreamEnvelope::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_stream_envelope_set_controller_size() {
    let original = StreamEnvelope {
        msg: Some(stream_envelope::Msg::SetControllerSize(SetControllerSize {
            size: Some(DisplaySize {
                cols: 132,
                rows: 43,
            }),
            request_snapshot: false,
        })),
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = StreamEnvelope::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_stream_envelope_keep_alive_lease() {
    let original = StreamEnvelope {
        msg: Some(stream_envelope::Msg::KeepAliveLease(KeepAliveLease {
            lease_id: 1,
            client_time_ms: 50000,
        })),
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = StreamEnvelope::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_stream_envelope_lease_revoked() {
    let original = StreamEnvelope {
        msg: Some(stream_envelope::Msg::LeaseRevoked(LeaseRevoked {
            lease_id: 1,
            reason: "takeover".to_string(),
        })),
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = StreamEnvelope::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_stream_envelope_request_snapshot() {
    let original = StreamEnvelope {
        msg: Some(stream_envelope::Msg::RequestSnapshot(RequestSnapshot {
            reason: request_snapshot::Reason::BaseMismatch as i32,
            known_state_id: 50,
        })),
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = StreamEnvelope::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_stream_envelope_ping() {
    let original = StreamEnvelope {
        msg: Some(stream_envelope::Msg::Ping(Ping {
            ping_id: 123,
            client_time_ms: 10000,
        })),
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = StreamEnvelope::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_stream_envelope_pong() {
    let original = StreamEnvelope {
        msg: Some(stream_envelope::Msg::Pong(Pong {
            ping_id: 123,
            echoed_client_time_ms: 10000,
            server_time_ms: 10005,
        })),
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = StreamEnvelope::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_stream_envelope_protocol_error() {
    let original = StreamEnvelope {
        msg: Some(stream_envelope::Msg::ProtocolError(ProtocolError {
            code: protocol_error::Code::BadMessage as i32,
            message: "Invalid field".to_string(),
            fatal: false,
        })),
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = StreamEnvelope::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_stream_envelope_unsupported_notice() {
    let original = StreamEnvelope {
        msg: Some(stream_envelope::Msg::UnsupportedNotice(
            UnsupportedFeatureNotice {
                feature: "clipboard".to_string(),
                behavior: "stripped".to_string(),
            },
        )),
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = StreamEnvelope::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_stream_envelope_screen_snapshot() {
    let original = StreamEnvelope {
        msg: Some(stream_envelope::Msg::ScreenSnapshot(ScreenSnapshot {
            state_id: 1,
            size: Some(DisplaySize { cols: 80, rows: 24 }),
            style_table_reset: true,
            styles: vec![],
            rows: vec![],
            cursor: None,
            delivered_input_watermark: 0,
        })),
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = StreamEnvelope::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_stream_envelope_screen_delta_stream() {
    let original = StreamEnvelope {
        msg: Some(stream_envelope::Msg::ScreenDeltaStream(ScreenDelta {
            base_state_id: 1,
            state_id: 2,
            styles_added: vec![],
            row_patches: vec![],
            cursor: None,
            delivered_input_watermark: 0,
        })),
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = StreamEnvelope::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_stream_envelope_input_event() {
    let original = StreamEnvelope {
        msg: Some(stream_envelope::Msg::InputEvent(InputEvent {
            input_seq: 1,
            client_time_ms: 1000,
            payload: Some(input_event::Payload::TextUtf8(b"hello".to_vec())),
        })),
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = StreamEnvelope::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_stream_envelope_input_ack() {
    let original = StreamEnvelope {
        msg: Some(stream_envelope::Msg::InputAck(InputAck {
            acked_seq: 10,
            rtt_sample_seq: 9,
            echoed_client_time_ms: 5000,
        })),
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = StreamEnvelope::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_stream_envelope_empty() {
    let original = StreamEnvelope { msg: None };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = StreamEnvelope::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

// =============================================================================
// DATAGRAM ENVELOPE ONEOF TESTS
// =============================================================================

#[test]
fn test_datagram_envelope_screen_delta() {
    let original = DatagramEnvelope {
        msg: Some(datagram_envelope::Msg::ScreenDelta(ScreenDelta {
            base_state_id: 100,
            state_id: 101,
            styles_added: vec![],
            row_patches: vec![RowPatch {
                row: 5,
                runs: vec![CellRun {
                    col_start: 0,
                    codepoints: vec!['X' as u32],
                    widths: vec![1],
                    style_ids: vec![0],
                }],
            }],
            cursor: Some(CursorState {
                row: 5,
                col: 1,
                visible: true,
                blink: false,
                shape: CursorShape::Block as i32,
            }),
            delivered_input_watermark: 50,
        })),
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = DatagramEnvelope::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_datagram_envelope_state_ack() {
    let original = DatagramEnvelope {
        msg: Some(datagram_envelope::Msg::StateAck(StateAck {
            last_applied_state_id: 100,
            last_received_state_id: 102,
            client_time_ms: 50000,
            estimated_loss_ppm: 500,
            srtt_ms: 25,
        })),
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = DatagramEnvelope::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_datagram_envelope_ping() {
    let original = DatagramEnvelope {
        msg: Some(datagram_envelope::Msg::Ping(Ping {
            ping_id: 999,
            client_time_ms: 12345,
        })),
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = DatagramEnvelope::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_datagram_envelope_pong() {
    let original = DatagramEnvelope {
        msg: Some(datagram_envelope::Msg::Pong(Pong {
            ping_id: 999,
            echoed_client_time_ms: 12345,
            server_time_ms: 12350,
        })),
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = DatagramEnvelope::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_datagram_envelope_empty() {
    let original = DatagramEnvelope { msg: None };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = DatagramEnvelope::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

// =============================================================================
// EDGE CASES
// =============================================================================

#[test]
fn test_max_u64_values() {
    let original = ScreenDelta {
        base_state_id: u64::MAX,
        state_id: u64::MAX,
        styles_added: vec![],
        row_patches: vec![],
        cursor: None,
        delivered_input_watermark: u64::MAX,
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = ScreenDelta::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_max_u32_values() {
    let original = DisplaySize {
        cols: u32::MAX,
        rows: u32::MAX,
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = DisplaySize::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_zero_values() {
    let original = StateAck {
        last_applied_state_id: 0,
        last_received_state_id: 0,
        client_time_ms: 0,
        estimated_loss_ppm: 0,
        srtt_ms: 0,
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = StateAck::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_unicode_strings() {
    let original = ClientHello {
        version: None,
        capabilities: None,
        client_name: "客户端-العميل-クライアント".to_string(),
        bearer_token: "🔐🔑🗝️".as_bytes().to_vec(),
        resume_token: vec![],
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = ClientHello::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_large_bearer_token() {
    let original = ClientHello {
        version: None,
        capabilities: None,
        client_name: String::new(),
        bearer_token: vec![0xAB; 10000],
        resume_token: vec![0xCD; 10000],
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = ClientHello::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_negative_scroll_delta() {
    let original = MouseEvent {
        kind: MouseKind::Scroll as i32,
        col: 0,
        row: 0,
        button: MouseButton::Unspecified as i32,
        scroll_delta: i32::MIN,
        modifiers: None,
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = MouseEvent::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_wide_character_widths() {
    let original = RowData {
        row: 0,
        codepoints: vec![0x4E2D, 0x6587, 0x5B57], // 中文字
        widths: vec![2, 2, 2],
        style_ids: vec![0, 0, 0],
    };
    let mut buf = Vec::new();
    original.encode(&mut buf).unwrap();
    let decoded = RowData::decode(&buf[..]).unwrap();
    assert_eq!(original, decoded);
}
