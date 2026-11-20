//! Known-answer tests (FIPS 180-2 for SHA-256, RFC 7914 section 11 for
//! PBKDF2-HMAC-SHA256, RFC 4648 section 10 for base64, iCloud's listing
//! of the test account for its checksum).

#![allow(clippy::expect_used)] // test code may panic

use std::io::Read;

use ikeloud_hash::{
    HashingReader, base64, content_sums, hex, pbkdf2_sha256, sha256, sha256_reader,
};

#[test]
fn sha256_vectors() {
    let cases: &[(&[&[u8]], &str)] = &[
        (
            &[b""],
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ),
        (
            &[b"abc"],
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        ),
        (
            &[b"a", b"b", b"c"],
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        ),
    ];
    for (parts, expected) in cases {
        assert_eq!(hex(&sha256(parts)), *expected);
    }
}

#[test]
fn a_reader_gives_the_same_hash_in_blocks() {
    // Larger than one block of the reader.
    let data: Vec<u8> = (0..200_000u32)
        .map(|i| u8::try_from(i % 251).expect("below 251"))
        .collect();
    let mut reader = data.as_slice();
    assert_eq!(sha256_reader(&mut reader).expect("read"), sha256(&[&data]));
    assert_eq!(
        hex(&sha256_reader(&mut &b"abc"[..]).expect("read")),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn pbkdf2_vector() {
    // RFC 7914, section 11: P = "passwd", S = "salt", c = 1, first 32 bytes.
    assert_eq!(
        hex(&pbkdf2_sha256(b"passwd", b"salt", 1)),
        "55ac046e56e3089fec1691c22544b605f94185216dde0465e68b9d57c20dacbc"
    );
}

#[test]
fn a_hashing_reader_hashes_what_passes_through() {
    let data: Vec<u8> = (0..100_000u32)
        .map(|i| u8::try_from(i % 7).expect("small"))
        .collect();
    let mut reader = HashingReader::new(data.as_slice());
    let mut copy = Vec::new();
    reader.read_to_end(&mut copy).expect("read");
    assert_eq!(copy, data);
    assert_eq!(reader.finish(), sha256(&[&data]));
}

#[test]
fn base64_vectors() {
    let cases: &[(&[u8], &str)] = &[
        (b"", ""),
        (b"f", "Zg=="),
        (b"fo", "Zm8="),
        (b"foo", "Zm9v"),
        (b"foob", "Zm9vYg=="),
        (b"fooba", "Zm9vYmE="),
        (b"foobar", "Zm9vYmFy"),
        (&[0xfb, 0xff, 0xbf], "+/+/"),
    ];
    for (input, expected) in cases {
        assert_eq!(base64(input), *expected, "{input:?}");
    }
}

/// Files of the test account and the checksum iCloud listed for them
/// (`fileContent.value.fileChecksum`, 2026-11-19).
#[test]
fn icloud_checksum_vectors() {
    let cases: &[(&[u8], &str)] = &[
        (b"", "AW2cWT2PxzWixMKuuNMTHJ8CJ3sH"),
        (
            ".skrytý soubor\n".as_bytes(),
            "AQHakDkhfQrDAuUw+UN+i9NLP6MP",
        ),
        (
            "tečka.na.konci.\n".as_bytes(),
            "AfNrVTIsg0mQl0hufLRQzgrpnjBS",
        ),
    ];
    for (content, expected) in cases {
        let (sha256_sum, checksum) = content_sums(&mut &content[..]).expect("read");
        assert_eq!(checksum, *expected, "{content:?}");
        assert_eq!(sha256_sum, sha256(&[content]));
    }
}

#[test]
fn content_sums_read_in_blocks() {
    // Larger than one block of the reader; the same sums as in one piece.
    let data: Vec<u8> = (0..200_000u32)
        .map(|i| u8::try_from(i % 251).expect("below 251"))
        .collect();
    let (whole, checksum) = content_sums(&mut data.as_slice()).expect("read");
    assert_eq!(whole, sha256(&[&data]));
    let mut slow = OneByte(data.as_slice());
    assert_eq!(content_sums(&mut slow).expect("read"), (whole, checksum));
}

/// Yields one byte per read and an interruption before each.
struct OneByte<'a>(&'a [u8]);

impl Read for OneByte<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        static FLIP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !FLIP.fetch_xor(true, std::sync::atomic::Ordering::Relaxed) {
            return Err(std::io::ErrorKind::Interrupted.into());
        }
        match (self.0.split_first(), buf.first_mut()) {
            (Some((first, rest)), Some(slot)) => {
                *slot = *first;
                self.0 = rest;
                Ok(1)
            }
            _ => Ok(0),
        }
    }
}

#[test]
fn content_sums_pass_read_errors_on() {
    struct Broken;
    impl Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::PermissionDenied.into())
        }
    }
    let error = content_sums(&mut Broken).expect_err("must fail");
    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
}
