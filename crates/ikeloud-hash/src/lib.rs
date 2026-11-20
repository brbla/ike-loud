//! Hash functions of ike loud, the single wrapper of `sha2`, `sha1` and
//! `pbkdf2`.
//!
//! The sign-in (SRP) needs SHA-256 and PBKDF2, the local scanner hashes file
//! contents, the check run computes iCloud's content checksum. The
//! single-wrapper rule (dependency-policy.toml) puts the libraries here,
//! behind plain functions on bytes.

use std::io::{self, Read};

use sha1::Sha1;
use sha2::{Digest, Sha256};

/// What iCloud hashes before the content (decision 0028, found by trying
/// on 1 466 files of the test account, up to 31.7 MB).
const ICLOUD_SALT: &[u8] = b"com.apple.XattrObjectSalt\0com.apple.DataObjectSalt\0";

/// Both sums of everything `reader` yields, read once in blocks: the
/// SHA-256 (the content hash of the common state) and iCloud's checksum in
/// the form iCloud lists it (`fileChecksum`: base64 of the byte 1 and the
/// SHA-1 of a fixed salt and the content).
pub fn content_sums(reader: &mut impl Read) -> io::Result<([u8; 32], String)> {
    let mut sha256 = Sha256::new();
    let mut sha1 = Sha1::new();
    sha1.update(ICLOUD_SALT);
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => {
                let part = buffer.get(..n).unwrap_or_default();
                sha256.update(part);
                sha1.update(part);
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    let mut checksum = vec![1u8];
    checksum.extend_from_slice(&sha1.finalize());
    Ok((sha256.finalize().into(), base64(&checksum)))
}

/// Standard base64 (RFC 4648, with padding).
pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let sign = |i: u32| {
        let index = usize::try_from(i & 63).unwrap_or_default();
        char::from(ALPHABET.get(index).copied().unwrap_or(b'='))
    };
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = |i: usize| u32::from(chunk.get(i).copied().unwrap_or(0));
        let n = (b(0) << 16) | (b(1) << 8) | b(2);
        out.push(sign(n >> 18));
        out.push(sign(n >> 12));
        out.push(if chunk.len() > 1 { sign(n >> 6) } else { '=' });
        out.push(if chunk.len() > 2 { sign(n) } else { '=' });
    }
    out
}

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

/// A reader that computes the SHA-256 of everything read through it: an
/// upload hashes exactly the bytes it sends.
#[derive(Debug)]
pub struct HashingReader<R> {
    inner: R,
    hasher: Sha256,
}

impl<R: Read> HashingReader<R> {
    /// Wraps `inner`.
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            hasher: Sha256::new(),
        }
    }

    /// The SHA-256 of what was read so far.
    pub fn finish(self) -> [u8; 32] {
        self.hasher.finalize().into()
    }
}

impl<R: Read> Read for HashingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.hasher.update(buf.get(..n).unwrap_or_default());
        Ok(n)
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
