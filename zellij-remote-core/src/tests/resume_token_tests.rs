use crate::resume_token::ResumeToken;

const TEST_SECRET: &[u8] = b"test_secret_key_12345678901234567890";

#[test]
fn test_encode_decode_signed_roundtrip() {
    let token = ResumeToken {
        session_id: 123456789,
        client_id: 42,
        last_applied_state_id: 100,
        last_acked_input_seq: 50,
        issued_at_ms: 1704067200000, // 2024-01-01 00:00:00 UTC
    };

    let encoded = token.encode_signed(TEST_SECRET);
    assert_eq!(encoded.len(), 72); // 40 byte payload + 32 byte signature

    let decoded = ResumeToken::decode_signed(&encoded, TEST_SECRET).expect("decode should succeed");

    assert_eq!(decoded.session_id, token.session_id);
    assert_eq!(decoded.client_id, token.client_id);
    assert_eq!(decoded.last_applied_state_id, token.last_applied_state_id);
    assert_eq!(decoded.last_acked_input_seq, token.last_acked_input_seq);
    assert_eq!(decoded.issued_at_ms, token.issued_at_ms);
}

#[test]
fn test_decode_invalid_length() {
    assert!(ResumeToken::decode_signed(&[], TEST_SECRET).is_none());
    assert!(ResumeToken::decode_signed(&[0u8; 16], TEST_SECRET).is_none());
    assert!(ResumeToken::decode_signed(&[0u8; 71], TEST_SECRET).is_none());
}

#[test]
fn test_decode_wrong_secret_fails() {
    let token = ResumeToken {
        session_id: 1,
        client_id: 1,
        last_applied_state_id: 1,
        last_acked_input_seq: 0,
        issued_at_ms: 1000,
    };

    let encoded = token.encode_signed(TEST_SECRET);
    let wrong_secret = b"wrong_secret_key_12345678901234567890";

    assert!(ResumeToken::decode_signed(&encoded, wrong_secret).is_none());
}

#[test]
fn test_tampered_payload_fails() {
    let token = ResumeToken {
        session_id: 1,
        client_id: 1,
        last_applied_state_id: 1,
        last_acked_input_seq: 0,
        issued_at_ms: 1000,
    };

    let mut encoded = token.encode_signed(TEST_SECRET);
    encoded[0] ^= 0xff;

    assert!(ResumeToken::decode_signed(&encoded, TEST_SECRET).is_none());
}

#[test]
fn test_tampered_signature_fails() {
    let token = ResumeToken {
        session_id: 1,
        client_id: 1,
        last_applied_state_id: 1,
        last_acked_input_seq: 0,
        issued_at_ms: 1000,
    };

    let mut encoded = token.encode_signed(TEST_SECRET);
    let last_idx = encoded.len() - 1;
    encoded[last_idx] ^= 0xff;

    assert!(ResumeToken::decode_signed(&encoded, TEST_SECRET).is_none());
}

#[test]
fn test_is_expired() {
    let token = ResumeToken {
        session_id: 1,
        client_id: 1,
        last_applied_state_id: 1,
        last_acked_input_seq: 0,
        issued_at_ms: 1000,
    };

    assert!(!token.is_expired_at(5000, 3000));
    assert!(!token.is_expired_at(5000, 6000));
    assert!(token.is_expired_at(5000, 6001));
    assert!(token.is_expired_at(5000, 10000));
}

#[test]
fn test_is_valid_timestamp_rejects_future() {
    let token = ResumeToken {
        session_id: 1,
        client_id: 1,
        last_applied_state_id: 1,
        last_acked_input_seq: 0,
        issued_at_ms: 10000,
    };

    assert!(!token.is_valid_timestamp(5000, 5000, 1000));
    assert!(token.is_valid_timestamp(5000, 9500, 1000));
}

#[test]
fn test_new_creates_current_timestamp() {
    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;

    let token = ResumeToken::new(1, 2, 3, 4);

    let after = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;

    assert!(token.issued_at_ms >= before);
    assert!(token.issued_at_ms <= after);
    assert_eq!(token.session_id, 1);
    assert_eq!(token.client_id, 2);
    assert_eq!(token.last_applied_state_id, 3);
    assert_eq!(token.last_acked_input_seq, 4);
}

#[test]
fn test_default_expiry_ms() {
    assert_eq!(ResumeToken::default_expiry_ms(), 300_000);
}
