use bytes::BytesMut;
use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt};

use zellij_remote_bridge::{
    build_server_hello, decode_envelope, encode_envelope, run_handshake, DecodeResult,
};
use zellij_remote_protocol::{
    stream_envelope, Capabilities, ClientHello, ProtocolVersion, ScreenDelta, ScreenSnapshot,
    SessionState, StreamEnvelope,
};

fn make_client_hello() -> ClientHello {
    ClientHello {
        version: Some(ProtocolVersion { major: 1, minor: 0 }),
        capabilities: Some(Capabilities {
            supports_datagrams: true,
            max_datagram_bytes: 1200,
            supports_style_dictionary: true,
            supports_styled_underlines: false,
            supports_prediction: true,
            supports_images: false,
            supports_clipboard: false,
            supports_hyperlinks: false,
        }),
        client_name: "integration-test".to_string(),
        bearer_token: vec![],
        resume_token: vec![],
    }
}

#[tokio::test]
async fn test_full_handshake_flow_over_duplex() {
    let (client_stream, server_stream) = duplex(8192);
    let (client_read, mut client_write) = tokio::io::split(client_stream);
    let (server_read, server_write) = tokio::io::split(server_stream);

    let server_handle = tokio::spawn(async move {
        run_handshake(server_read, server_write, "test-session".to_string(), 42).await
    });

    let client_hello = make_client_hello();
    let envelope = StreamEnvelope {
        msg: Some(stream_envelope::Msg::ClientHello(client_hello.clone())),
    };
    let encoded = encode_envelope(&envelope).unwrap();
    client_write.write_all(&encoded).await.unwrap();

    let mut client_read = client_read;
    let mut buffer = BytesMut::new();
    let mut chunk = [0u8; 4096];
    let n = client_read.read(&mut chunk).await.unwrap();
    buffer.extend_from_slice(&chunk[..n]);

    let response = match decode_envelope(&mut buffer).unwrap() {
        DecodeResult::Complete(env) => env,
        DecodeResult::Incomplete => panic!("incomplete response"),
    };

    match response.msg {
        Some(stream_envelope::Msg::ServerHello(hello)) => {
            assert_eq!(hello.client_id, 42);
            assert_eq!(hello.session_name, "test-session");
            assert_eq!(hello.session_state, SessionState::Running as i32);
            assert!(hello.negotiated_capabilities.unwrap().supports_datagrams);
            assert!(hello.lease.is_some());
        },
        _ => panic!("expected ServerHello"),
    }

    let result = server_handle.await.unwrap().unwrap();
    assert_eq!(result.client_hello.client_name, "integration-test");
}

#[tokio::test]
async fn test_multiple_messages_in_sequence() {
    let (client_stream, server_stream) = duplex(8192);
    let (mut client_read, mut client_write) = tokio::io::split(client_stream);
    let (server_read, server_write) = tokio::io::split(server_stream);

    let server_handle = tokio::spawn(async move {
        run_handshake(server_read, server_write, "seq-test".to_string(), 1).await
    });

    let client_hello = make_client_hello();
    let envelope = StreamEnvelope {
        msg: Some(stream_envelope::Msg::ClientHello(client_hello)),
    };
    let encoded = encode_envelope(&envelope).unwrap();
    client_write.write_all(&encoded).await.unwrap();

    let mut buffer = BytesMut::new();
    let mut chunk = [0u8; 4096];
    let n = client_read.read(&mut chunk).await.unwrap();
    buffer.extend_from_slice(&chunk[..n]);

    match decode_envelope(&mut buffer).unwrap() {
        DecodeResult::Complete(env) => {
            assert!(matches!(
                env.msg,
                Some(stream_envelope::Msg::ServerHello(_))
            ));
        },
        DecodeResult::Incomplete => panic!("incomplete"),
    }

    assert!(buffer.is_empty(), "buffer should be fully consumed");
    server_handle.await.unwrap().unwrap();
}

#[test]
fn test_screen_snapshot_encode_decode_via_framing() {
    use zellij_remote_protocol::{CursorState, DisplaySize, RowData, StyleDef};

    let snapshot = ScreenSnapshot {
        state_id: 12345,
        size: Some(DisplaySize { cols: 80, rows: 24 }),
        style_table_reset: true,
        styles: vec![StyleDef {
            style_id: 1,
            style: None,
        }],
        rows: vec![RowData {
            row: 0,
            codepoints: vec![72, 101, 108, 108, 111],
            widths: vec![1, 1, 1, 1, 1],
            style_ids: vec![0, 0, 0, 0, 0],
        }],
        cursor: Some(CursorState {
            row: 0,
            col: 5,
            visible: true,
            blink: true,
            shape: 1,
        }),
        delivered_input_watermark: 100,
    };

    let envelope = StreamEnvelope {
        msg: Some(stream_envelope::Msg::ScreenSnapshot(snapshot.clone())),
    };

    let encoded = encode_envelope(&envelope).unwrap();
    let mut buf = BytesMut::from(&encoded[..]);

    match decode_envelope(&mut buf).unwrap() {
        DecodeResult::Complete(decoded) => match decoded.msg {
            Some(stream_envelope::Msg::ScreenSnapshot(s)) => {
                assert_eq!(s.state_id, 12345);
                assert_eq!(s.rows.len(), 1);
                assert_eq!(s.rows[0].codepoints, vec![72, 101, 108, 108, 111]);
            },
            _ => panic!("wrong message type"),
        },
        DecodeResult::Incomplete => panic!("incomplete"),
    }
}

#[test]
fn test_screen_delta_encode_decode_via_framing() {
    use zellij_remote_protocol::{CellRun, CursorState, RowPatch, StyleDef};

    let delta = ScreenDelta {
        base_state_id: 100,
        state_id: 101,
        styles_added: vec![StyleDef {
            style_id: 5,
            style: None,
        }],
        row_patches: vec![RowPatch {
            row: 10,
            runs: vec![CellRun {
                col_start: 0,
                codepoints: vec![88, 89, 90],
                widths: vec![1, 1, 1],
                style_ids: vec![5, 5, 5],
            }],
        }],
        cursor: Some(CursorState {
            row: 10,
            col: 3,
            visible: true,
            blink: false,
            shape: 2,
        }),
        delivered_input_watermark: 50,
    };

    let envelope = StreamEnvelope {
        msg: Some(stream_envelope::Msg::ScreenDeltaStream(delta.clone())),
    };

    let encoded = encode_envelope(&envelope).unwrap();
    let mut buf = BytesMut::from(&encoded[..]);

    match decode_envelope(&mut buf).unwrap() {
        DecodeResult::Complete(decoded) => match decoded.msg {
            Some(stream_envelope::Msg::ScreenDeltaStream(d)) => {
                assert_eq!(d.base_state_id, 100);
                assert_eq!(d.state_id, 101);
                assert_eq!(d.row_patches.len(), 1);
                assert_eq!(d.row_patches[0].row, 10);
            },
            _ => panic!("wrong message type"),
        },
        DecodeResult::Incomplete => panic!("incomplete"),
    }
}

#[test]
fn test_large_snapshot_framing() {
    use zellij_remote_protocol::{DisplaySize, RowData};

    let rows: Vec<RowData> = (0..100)
        .map(|i| RowData {
            row: i,
            codepoints: vec![32; 200],
            widths: vec![1; 200],
            style_ids: vec![0; 200],
        })
        .collect();

    let snapshot = ScreenSnapshot {
        state_id: 999,
        size: Some(DisplaySize {
            cols: 200,
            rows: 100,
        }),
        style_table_reset: true,
        styles: vec![],
        rows,
        cursor: None,
        delivered_input_watermark: 0,
    };

    let envelope = StreamEnvelope {
        msg: Some(stream_envelope::Msg::ScreenSnapshot(snapshot)),
    };

    let encoded = encode_envelope(&envelope).unwrap();
    assert!(encoded.len() > 10000, "should be a large message");

    let mut buf = BytesMut::from(&encoded[..]);
    match decode_envelope(&mut buf).unwrap() {
        DecodeResult::Complete(decoded) => match decoded.msg {
            Some(stream_envelope::Msg::ScreenSnapshot(s)) => {
                assert_eq!(s.rows.len(), 100);
            },
            _ => panic!("wrong type"),
        },
        DecodeResult::Incomplete => panic!("incomplete"),
    }
}

#[test]
fn test_build_server_hello_negotiates_capabilities() {
    let client_hello_with_datagrams = ClientHello {
        version: Some(ProtocolVersion { major: 1, minor: 0 }),
        capabilities: Some(Capabilities {
            supports_datagrams: true,
            max_datagram_bytes: 1400,
            supports_style_dictionary: false,
            supports_styled_underlines: true,
            supports_prediction: false,
            supports_images: true,
            supports_clipboard: true,
            supports_hyperlinks: true,
        }),
        client_name: "test".to_string(),
        bearer_token: vec![],
        resume_token: vec![],
    };

    let hello = build_server_hello(&client_hello_with_datagrams, "session", 1);

    let caps = hello.negotiated_capabilities.unwrap();
    assert!(
        caps.supports_datagrams,
        "should honor client datagram support"
    );
    assert!(caps.supports_style_dictionary, "server always enables");
    assert!(!caps.supports_images, "server doesn't support images yet");
    assert!(
        !caps.supports_clipboard,
        "server doesn't support clipboard yet"
    );
}
