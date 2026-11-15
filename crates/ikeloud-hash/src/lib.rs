//! Hash functions of ike loud, the single wrapper of `sha2` and `pbkdf2`.
//!
//! The sign-in (SRP) needs SHA-256 and PBKDF2, the local scanner hashes file
//! contents. The single-wrapper rule (dependency-policy.toml) puts both
//! libraries here, behind plain functions on bytes.

use std::io::{self, Read};

use sha2::{Digest, Sha256};

/// SHA-256 of the concatenation of `parts`.
pub fn sha256(parts: &[&[u8]]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part);
    }
    hasher.finalize().into()
}

/// SHA-256 of everything `reader` yields, read in blocks (a file of any
/// size takes constant memory).
pub fn sha256_reader(reader: &mut impl Read) -> io::Result<[u8; 32]> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => return Ok(hasher.finalize().into()),
            Ok(n) => hasher.update(buffer.get(..n).unwrap_or_default()),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
}

/// PBKDF2-HMAC-SHA256 with a 32-byte output.
pub fn pbkdf2_sha256(password: &[u8], salt: &[u8], iterations: u32) -> [u8; 32] {
    let mut output = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<Sha256>(password, salt, iterations, &mut output);
    output
}

/// Lower-case hex form of bytes.
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}
