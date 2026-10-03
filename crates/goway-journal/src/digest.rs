//! Content digests used to identify installed files.

use sha2::{Digest, Sha256};

/// Lowercase hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let hash = Sha256::digest(bytes);
    hash.iter().fold(String::with_capacity(64), |mut s, b| {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
        s
    })
}
