//! Random tokens sourced only from the operating system CSPRNG.

use std::fs::File;
use std::io::Read;

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};

/// Generate `bytes` cryptographically random bytes and encode them as unpadded base64url.
///
/// # Errors
/// The operating system random source cannot be read.
pub fn token(bytes: usize) -> std::io::Result<String> {
    let mut value = vec![0_u8; bytes];
    File::open("/dev/urandom")?.read_exact(&mut value)?;
    Ok(URL_SAFE_NO_PAD.encode(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tokens_are_url_safe_and_unpredictably_distinct() {
        let a = token(32).unwrap();
        let b = token(32).unwrap();
        assert_ne!(a, b);
        assert!(
            a.bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        );
    }
}
