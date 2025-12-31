use anyhow::Result;
use bytes::{Buf, Bytes, BytesMut};
use prost::Message;
use zellij_remote_protocol::{DatagramEnvelope, StreamEnvelope};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeResult<T> {
    Complete(T),
    Incomplete,
}

pub fn encode_envelope(envelope: &StreamEnvelope) -> Result<Vec<u8>> {
    let len = envelope.encoded_len();
    let mut buf = BytesMut::with_capacity(len + 5);
    prost::encoding::encode_varint(len as u64, &mut buf);
    envelope.encode(&mut buf)?;
    Ok(buf.to_vec())
}

/// Encode a DatagramEnvelope to Bytes (no length prefix for datagrams)
/// Returns Bytes for compatibility with wtransport send_datagram
pub fn encode_datagram_envelope(envelope: &DatagramEnvelope) -> Bytes {
    let mut buf = Vec::with_capacity(envelope.encoded_len());
    envelope.encode(&mut buf).expect("Vec write cannot fail");
    Bytes::from(buf)
}

/// Decode a DatagramEnvelope from bytes (no length prefix)
pub fn decode_datagram_envelope(bytes: &[u8]) -> Result<DatagramEnvelope, prost::DecodeError> {
    DatagramEnvelope::decode(bytes)
}

pub fn decode_envelope(buf: &mut BytesMut) -> Result<DecodeResult<StreamEnvelope>> {
    if buf.is_empty() {
        return Ok(DecodeResult::Incomplete);
    }

    let mut peek = &buf[..];
    let len = match prost::encoding::decode_varint(&mut peek) {
        Ok(len) => len as usize,
        Err(_) => {
            if buf.len() < 10 {
                return Ok(DecodeResult::Incomplete);
            }
            anyhow::bail!("invalid varint in frame header");
        },
    };

    let varint_len = buf.len() - peek.len();
    let total_len = varint_len + len;

    if buf.len() < total_len {
        return Ok(DecodeResult::Incomplete);
    }

    buf.advance(varint_len);
    let frame_data = buf.split_to(len);
    let envelope = StreamEnvelope::decode(&frame_data[..])?;
    Ok(DecodeResult::Complete(envelope))
}

#[cfg(test)]
mod tests {
    use super::*;
    use zellij_remote_protocol::{
        stream_envelope, Capabilities, ClientHello, ProtocolVersion, ServerHello,
    };

    fn make_client_hello() -> StreamEnvelope {
        StreamEnvelope {
            msg: Some(stream_envelope::Msg::ClientHello(ClientHello {
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
                client_name: "test-client".to_string(),
                bearer_token: vec![],
                resume_token: vec![],
            })),
        }
    }

    #[test]
    fn test_encode_decode_roundtrip() {
        let original = make_client_hello();
        let encoded = encode_envelope(&original).unwrap();
        let mut buf = BytesMut::from(&encoded[..]);

        match decode_envelope(&mut buf).unwrap() {
            DecodeResult::Complete(decoded) => {
                assert_eq!(original, decoded);
            },
            DecodeResult::Incomplete => panic!("expected complete decode"),
        }

        assert!(buf.is_empty(), "buffer should be consumed");
    }

    #[test]
    fn test_partial_varint_returns_incomplete() {
        // Feed only 0 bytes
        let mut buf = BytesMut::new();
        assert!(matches!(
            decode_envelope(&mut buf).unwrap(),
            DecodeResult::Incomplete
        ));
    }

    #[test]
    fn test_partial_body_returns_incomplete() {
        let original = make_client_hello();
        let encoded = encode_envelope(&original).unwrap();

        // Feed varint + partial body
        let partial_len = encoded.len() / 2;
        let mut buf = BytesMut::from(&encoded[..partial_len]);

        assert!(matches!(
            decode_envelope(&mut buf).unwrap(),
            DecodeResult::Incomplete
        ));
    }

    #[test]
    fn test_multiple_frames_in_buffer() {
        let msg1 = make_client_hello();
        let msg2 = StreamEnvelope {
            msg: Some(stream_envelope::Msg::ServerHello(ServerHello {
                negotiated_version: Some(ProtocolVersion { major: 1, minor: 0 }),
                negotiated_capabilities: None,
                client_id: 42,
                session_name: "test".to_string(),
                session_state: 1,
                lease: None,
                resume_token: vec![],
                snapshot_interval_ms: 5000,
                max_inflight_inputs: 256,
                render_window: 4,
            })),
        };

        let encoded1 = encode_envelope(&msg1).unwrap();
        let encoded2 = encode_envelope(&msg2).unwrap();

        let mut buf = BytesMut::new();
        buf.extend_from_slice(&encoded1);
        buf.extend_from_slice(&encoded2);

        // Decode first
        match decode_envelope(&mut buf).unwrap() {
            DecodeResult::Complete(decoded) => assert_eq!(msg1, decoded),
            DecodeResult::Incomplete => panic!("expected first message"),
        }

        // Decode second
        match decode_envelope(&mut buf).unwrap() {
            DecodeResult::Complete(decoded) => assert_eq!(msg2, decoded),
            DecodeResult::Incomplete => panic!("expected second message"),
        }

        assert!(buf.is_empty());
    }

    #[test]
    fn test_feed_one_byte_at_a_time() {
        let original = make_client_hello();
        let encoded = encode_envelope(&original).unwrap();

        let mut buf = BytesMut::new();

        for (i, &byte) in encoded.iter().enumerate() {
            buf.extend_from_slice(&[byte]);

            match decode_envelope(&mut buf) {
                Ok(DecodeResult::Incomplete) => {
                    // Expected for all but the last byte
                    if i == encoded.len() - 1 {
                        panic!("should have completed on last byte");
                    }
                },
                Ok(DecodeResult::Complete(decoded)) => {
                    assert_eq!(i, encoded.len() - 1, "should only complete on last byte");
                    assert_eq!(original, decoded);
                    return;
                },
                Err(e) => panic!("unexpected error: {}", e),
            }
        }

        panic!("never completed");
    }

    #[test]
    fn test_invalid_varint_after_max_length() {
        // Create a buffer with invalid varint (all high bits set, more than 10 bytes)
        let mut buf = BytesMut::from(&[0xFF; 11][..]);

        let result = decode_envelope(&mut buf);
        assert!(result.is_err(), "should error on invalid varint");
    }

    #[test]
    fn test_decode_corrupted_protobuf() {
        // Valid varint (length 5) followed by garbage
        let mut buf = BytesMut::from(&[5u8, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF][..]);

        let result = decode_envelope(&mut buf);
        assert!(result.is_err(), "should error on corrupted protobuf");
    }

    #[test]
    fn test_empty_envelope() {
        let envelope = StreamEnvelope { msg: None };
        let encoded = encode_envelope(&envelope).unwrap();
        let mut buf = BytesMut::from(&encoded[..]);

        match decode_envelope(&mut buf).unwrap() {
            DecodeResult::Complete(decoded) => {
                assert_eq!(envelope, decoded);
            },
            DecodeResult::Incomplete => panic!("expected complete"),
        }
    }
}
