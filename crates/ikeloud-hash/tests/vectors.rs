//! Known-answer tests (FIPS 180-2 for SHA-256, RFC 7914 section 11 for
//! PBKDF2-HMAC-SHA256).

#![allow(clippy::expect_used)] // test code may panic

use ikeloud_hash::{hex, pbkdf2_sha256, sha256, sha256_reader};

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
