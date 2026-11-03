//! Authenticated opaque numeric identifiers.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use sha2::Sha256;

const TAG_BYTES: usize = 16;
const MAX_TOKEN_BYTES: usize = 256;
type HmacSha256 = Hmac<Sha256>;

pub(crate) fn bounded_cursor<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    let value = <Option<String> as serde::Deserialize>::deserialize(deserializer)?;
    if value
        .as_ref()
        .is_some_and(|value| value.chars().count() > MAX_TOKEN_BYTES)
    {
        return Err(serde::de::Error::custom("cursor exceeds limit"));
    }
    Ok(value)
}

/// Opaque-ID validation failed without revealing whether a token was issued before.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpaqueError;

impl std::fmt::Display for OpaqueError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("invalid opaque identifier")
    }
}
impl std::error::Error for OpaqueError {}

/// Encodes IDs with an endpoint kind bound into their HMAC.
pub struct Opaque {
    secret: Vec<u8>,
}

impl Opaque {
    #[must_use]
    pub fn new(secret: impl AsRef<[u8]>) -> Self {
        Self {
            secret: secret.as_ref().to_vec(),
        }
    }

    #[must_use]
    pub fn encode(&self, kind: &str, value: i64) -> String {
        self.encode_payload(kind, &value.to_be_bytes())
    }

    fn encode_payload(&self, kind: &str, payload: &[u8]) -> String {
        let tag = self.tag(kind, payload);
        format!(
            "{kind}_{}",
            URL_SAFE_NO_PAD.encode([payload, &tag[..TAG_BYTES]].concat())
        )
    }

    /// Encode a composite cursor, binding both components to its endpoint.
    #[must_use]
    pub fn encode_pair(&self, kind: &str, first: i64, second: i64) -> String {
        self.encode_payload(kind, &[first.to_be_bytes(), second.to_be_bytes()].concat())
    }

    /// Encode a timestamp and identifier for a time-ordered cursor.
    ///
    /// # Errors
    /// The timestamp is empty or exceeds the 64-byte cursor limit.
    pub fn encode_timestamp(
        &self,
        kind: &str,
        timestamp: &str,
        id: i64,
    ) -> Result<String, OpaqueError> {
        if !valid_timestamp(timestamp) {
            return Err(OpaqueError);
        }
        Ok(self.encode_payload(
            kind,
            &[id.to_be_bytes().as_slice(), timestamp.as_bytes()].concat(),
        ))
    }

    /// Decode a token only when its kind and authentication tag match.
    ///
    /// # Errors
    /// The token is malformed, too long, from another kind or tampered.
    pub fn decode(&self, token: &str, kind: &str) -> Result<i64, OpaqueError> {
        let payload = self.decode_payload(token, kind, 8)?;
        Ok(i64::from_be_bytes(
            payload.as_slice().try_into().map_err(|_| OpaqueError)?,
        ))
    }

    /// Decode a composite cursor only for the issuing secret and endpoint.
    ///
    /// # Errors
    /// The token is malformed, too long, has the wrong kind, or was altered.
    pub fn decode_pair(&self, token: &str, kind: &str) -> Result<(i64, i64), OpaqueError> {
        let payload = self.decode_payload(token, kind, 16)?;
        let first = i64::from_be_bytes(payload[..8].try_into().map_err(|_| OpaqueError)?);
        let second = i64::from_be_bytes(payload[8..].try_into().map_err(|_| OpaqueError)?);
        Ok((first, second))
    }

    /// Decode a time-ordered cursor issued for this endpoint.
    ///
    /// # Errors
    /// The token is invalid, has the wrong kind, or contains an invalid timestamp.
    pub fn decode_timestamp(&self, token: &str, kind: &str) -> Result<(String, i64), OpaqueError> {
        let payload = self.decode_payload_range(token, kind, 9, 72)?;
        let id = i64::from_be_bytes(payload[..8].try_into().map_err(|_| OpaqueError)?);
        let timestamp = std::str::from_utf8(&payload[8..]).map_err(|_| OpaqueError)?;
        if !valid_timestamp(timestamp) {
            return Err(OpaqueError);
        }
        Ok((timestamp.to_owned(), id))
    }

    fn decode_payload(
        &self,
        token: &str,
        kind: &str,
        length: usize,
    ) -> Result<Vec<u8>, OpaqueError> {
        self.decode_payload_range(token, kind, length, length)
    }

    fn decode_payload_range(
        &self,
        token: &str,
        kind: &str,
        min: usize,
        max: usize,
    ) -> Result<Vec<u8>, OpaqueError> {
        if token.len() > MAX_TOKEN_BYTES {
            return Err(OpaqueError);
        }
        let encoded = token.strip_prefix(&format!("{kind}_")).ok_or(OpaqueError)?;
        let bytes = URL_SAFE_NO_PAD.decode(encoded).map_err(|_| OpaqueError)?;
        if bytes.len() < min + TAG_BYTES || bytes.len() > max + TAG_BYTES {
            return Err(OpaqueError);
        }
        let (payload, tag) = bytes.split_at(bytes.len() - TAG_BYTES);
        let expected = self.tag(kind, payload);
        if !bool::from(tag.ct_eq(&expected[..TAG_BYTES])) {
            return Err(OpaqueError);
        }
        Ok(payload.to_vec())
    }

    fn tag(&self, kind: &str, payload: &[u8]) -> [u8; 32] {
        let mut mac =
            HmacSha256::new_from_slice(&self.secret).expect("HMAC accepts arbitrary key sizes");
        mac.update(kind.as_bytes());
        mac.update(&[0]);
        mac.update(payload);
        mac.finalize().into_bytes().into()
    }
}

fn valid_timestamp(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.bytes().all(|b| {
            b.is_ascii_digit() || matches!(b, b'-' | b'+' | b':' | b'.' | b' ' | b'T' | b'Z')
        })
}

trait ConstantTimeEq {
    fn ct_eq(&self, other: &[u8]) -> subtle::Choice;
}
impl ConstantTimeEq for [u8] {
    fn ct_eq(&self, other: &[u8]) -> subtle::Choice {
        subtle::ConstantTimeEq::ct_eq(self, other)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ids_are_bound_to_kind_and_secret() {
        let a = Opaque::new(b"secret");
        let id = a.encode("j", 42);
        assert_eq!(a.decode(&id, "j"), Ok(42));
        assert!(a.decode(&id, "m").is_err());
        assert!(Opaque::new(b"other").decode(&id, "j").is_err());
    }

    #[test]
    fn composite_cursors_are_bound_to_kind_secret_and_payload_shape() {
        let opaque = Opaque::new(b"secret");
        let cursor = opaque.encode_pair("cursor:job-items", 3, 42);
        assert_eq!(opaque.decode_pair(&cursor, "cursor:job-items"), Ok((3, 42)));
        assert!(opaque.decode_pair(&cursor, "cursor:other").is_err());
        assert!(
            Opaque::new(b"other")
                .decode_pair(&cursor, "cursor:job-items")
                .is_err()
        );
        assert!(opaque.decode(&cursor, "cursor:job-items").is_err());
        let mut tampered = cursor.clone().into_bytes();
        let last = tampered.len() - 3;
        tampered[last] = if tampered[last] == b'A' { b'B' } else { b'A' };
        assert!(
            opaque
                .decode_pair(std::str::from_utf8(&tampered).unwrap(), "cursor:job-items")
                .is_err()
        );
    }

    #[test]
    fn timestamp_cursors_are_authenticated_and_length_limited() {
        let opaque = Opaque::new(b"secret");
        let value = opaque
            .encode_timestamp("cursor:activity", "2026-11-01 12:34:56", 9)
            .unwrap();
        assert_eq!(
            opaque.decode_timestamp(&value, "cursor:activity"),
            Ok(("2026-11-01 12:34:56".to_owned(), 9))
        );
        assert!(opaque.decode_timestamp(&value, "cursor:other").is_err());
        assert!(
            opaque
                .encode_timestamp("cursor:activity", &"x".repeat(65), 9)
                .is_err()
        );
        assert!(opaque.encode_timestamp("cursor:activity", "", 9).is_err());
        assert!(
            opaque
                .encode_timestamp("cursor:activity", "/private", 9)
                .is_err()
        );
    }
}
