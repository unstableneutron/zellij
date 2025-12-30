use std::time::{SystemTime, UNIX_EPOCH};

use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

const PAYLOAD_SIZE: usize = 40;
const SIGNATURE_SIZE: usize = 32;
const SIGNED_TOKEN_SIZE: usize = PAYLOAD_SIZE + SIGNATURE_SIZE;
const DEFAULT_TOKEN_EXPIRY_MS: u64 = 300_000; // 5 minutes
const DEFAULT_MAX_CLOCK_SKEW_MS: u64 = 30_000; // 30 seconds

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResumeToken {
    pub session_id: u64,
    pub client_id: u64,
    pub last_applied_state_id: u64,
    pub last_acked_input_seq: u64,
    pub issued_at_ms: u64,
}

impl ResumeToken {
    pub fn new(
        session_id: u64,
        client_id: u64,
        last_applied_state_id: u64,
        last_acked_input_seq: u64,
    ) -> Self {
        let issued_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        Self {
            session_id,
            client_id,
            last_applied_state_id,
            last_acked_input_seq,
            issued_at_ms,
        }
    }

    pub fn encode_signed(&self, secret: &[u8]) -> Vec<u8> {
        let payload = self.encode_payload();
        let signature = hmac_sha256(secret, &payload);
        let mut result = Vec::with_capacity(SIGNED_TOKEN_SIZE);
        result.extend_from_slice(&payload);
        result.extend_from_slice(&signature);
        result
    }

    pub fn decode_signed(bytes: &[u8], secret: &[u8]) -> Option<Self> {
        if bytes.len() < SIGNED_TOKEN_SIZE {
            return None;
        }
        let (payload, signature) = bytes.split_at(bytes.len() - SIGNATURE_SIZE);
        let expected_sig = hmac_sha256(secret, payload);
        if !constant_time_eq(signature, &expected_sig) {
            return None;
        }
        Self::decode_payload(payload)
    }

    fn encode_payload(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(PAYLOAD_SIZE);
        buf.extend_from_slice(&self.session_id.to_le_bytes());
        buf.extend_from_slice(&self.client_id.to_le_bytes());
        buf.extend_from_slice(&self.last_applied_state_id.to_le_bytes());
        buf.extend_from_slice(&self.last_acked_input_seq.to_le_bytes());
        buf.extend_from_slice(&self.issued_at_ms.to_le_bytes());
        buf
    }

    fn decode_payload(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != PAYLOAD_SIZE {
            return None;
        }
        Some(Self {
            session_id: u64::from_le_bytes(bytes[0..8].try_into().ok()?),
            client_id: u64::from_le_bytes(bytes[8..16].try_into().ok()?),
            last_applied_state_id: u64::from_le_bytes(bytes[16..24].try_into().ok()?),
            last_acked_input_seq: u64::from_le_bytes(bytes[24..32].try_into().ok()?),
            issued_at_ms: u64::from_le_bytes(bytes[32..40].try_into().ok()?),
        })
    }

    pub fn is_expired(&self, max_age_ms: u64) -> bool {
        let current_time_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        self.is_expired_at(max_age_ms, current_time_ms)
    }

    pub fn is_expired_at(&self, max_age_ms: u64, current_time_ms: u64) -> bool {
        current_time_ms.saturating_sub(self.issued_at_ms) > max_age_ms
    }

    pub fn is_valid_timestamp(
        &self,
        max_age_ms: u64,
        current_time_ms: u64,
        max_skew_ms: u64,
    ) -> bool {
        if self.issued_at_ms > current_time_ms + max_skew_ms {
            return false;
        }
        current_time_ms.saturating_sub(self.issued_at_ms) <= max_age_ms
    }

    pub fn default_expiry_ms() -> u64 {
        DEFAULT_TOKEN_EXPIRY_MS
    }

    pub fn default_max_clock_skew_ms() -> u64 {
        DEFAULT_MAX_CLOCK_SKEW_MS
    }
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(data);
    mac.finalize().into_bytes().into()
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b.iter()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumeResult {
    Resumed {
        client_id: u64,
        baseline_state_id: u64,
    },
    InvalidToken,
    ExpiredToken,
    FutureDatedToken,
    SessionMismatch,
    StateNotFound,
    ClientIdInUse,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_decode_signed() {
        let secret = b"test_secret_key_12345678901234567890";
        let token = ResumeToken {
            session_id: 123,
            client_id: 456,
            last_applied_state_id: 789,
            last_acked_input_seq: 100,
            issued_at_ms: 1000000,
        };

        let encoded = token.encode_signed(secret);
        let decoded = ResumeToken::decode_signed(&encoded, secret).unwrap();

        assert_eq!(token, decoded);
    }

    #[test]
    fn test_tampered_signature_rejected() {
        let secret = b"test_secret_key_12345678901234567890";
        let token = ResumeToken {
            session_id: 123,
            client_id: 456,
            last_applied_state_id: 789,
            last_acked_input_seq: 100,
            issued_at_ms: 1000000,
        };

        let mut encoded = token.encode_signed(secret);
        encoded[PAYLOAD_SIZE] ^= 0xff;

        assert!(ResumeToken::decode_signed(&encoded, secret).is_none());
    }

    #[test]
    fn test_wrong_secret_rejected() {
        let secret1 = b"secret_one_123456789012345678901234";
        let secret2 = b"secret_two_123456789012345678901234";
        let token = ResumeToken {
            session_id: 123,
            client_id: 456,
            last_applied_state_id: 789,
            last_acked_input_seq: 100,
            issued_at_ms: 1000000,
        };

        let encoded = token.encode_signed(secret1);
        assert!(ResumeToken::decode_signed(&encoded, secret2).is_none());
    }

    #[test]
    fn test_tampered_payload_rejected() {
        let secret = b"test_secret_key_12345678901234567890";
        let token = ResumeToken {
            session_id: 123,
            client_id: 456,
            last_applied_state_id: 789,
            last_acked_input_seq: 100,
            issued_at_ms: 1000000,
        };

        let mut encoded = token.encode_signed(secret);
        encoded[0] ^= 0xff;

        assert!(ResumeToken::decode_signed(&encoded, secret).is_none());
    }

    #[test]
    fn test_short_token_rejected() {
        let secret = b"test_secret_key_12345678901234567890";
        let short_data = vec![0u8; SIGNED_TOKEN_SIZE - 1];

        assert!(ResumeToken::decode_signed(&short_data, secret).is_none());
    }

    #[test]
    fn test_is_valid_timestamp() {
        let token = ResumeToken {
            session_id: 1,
            client_id: 1,
            last_applied_state_id: 1,
            last_acked_input_seq: 0,
            issued_at_ms: 1000,
        };

        assert!(token.is_valid_timestamp(5000, 2000, 1000));
        assert!(!token.is_valid_timestamp(500, 2000, 1000));
        assert!(!token.is_valid_timestamp(5000, 0, 500));
    }

    #[test]
    fn test_future_dated_token_rejected() {
        let token = ResumeToken {
            session_id: 1,
            client_id: 1,
            last_applied_state_id: 1,
            last_acked_input_seq: 0,
            issued_at_ms: 10000,
        };

        assert!(!token.is_valid_timestamp(5000, 5000, 1000));
    }
}
