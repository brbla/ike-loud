//! SRP sign-in tests against vectors from pysrp and the rclone variant.

#![allow(clippy::expect_used, clippy::panic)] // test code may panic

#[path = "data/srp_vectors.rs"]
mod vectors;

use ikeloud_api::srp::{Padding, Protocol, SrpClient, SrpError, derive_password};
use vectors::{EDGE_ACCOUNT, EDGE_DERIVED, EDGE_SALT, EDGES, VECTORS};

fn from_hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2).expect("even length"), 16).expect("hex"))
        .collect()
}

#[test]
fn password_derivation() {
    for v in VECTORS {
        let protocol = Protocol::from_name(v.protocol).expect("known protocol");
        let derived =
            derive_password(v.password, &from_hex(v.salt), v.iterations, protocol).expect("derive");
        assert_eq!(
            derived.to_vec(),
            from_hex(v.derived),
            "{} {}",
            v.account,
            v.protocol
        );
    }
}

#[test]
fn s2k_and_s2k_fo_differ() {
    let salt = [1u8; 16];
    let s2k = derive_password("password", &salt, 10, Protocol::S2k);
    let fo = derive_password("password", &salt, 10, Protocol::S2kFo);
    assert_ne!(s2k, fo);
}

#[test]
fn whole_exchange_matches_pysrp() {
    // In the regular vectors A, B and S are full length; both variants must agree.
    for padding in [Padding::GroupLength, Padding::NoLeadingZeros] {
        for v in VECTORS {
            let client = SrpClient::with_secret(&from_hex(v.a), padding).expect("client");
            assert_eq!(client.public_a(), from_hex(v.public_a), "A {}", v.protocol);
            let proof = client
                .process_challenge(
                    v.account,
                    &from_hex(v.derived),
                    &from_hex(v.salt),
                    &from_hex(v.b),
                )
                .expect("exchange");
            let name = format!("{padding:?} {} {}", v.account, v.protocol);
            assert_eq!(proof.k.to_vec(), from_hex(v.k), "K {name}");
            assert_eq!(proof.m1.to_vec(), from_hex(v.m1), "M1 {name}");
            assert_eq!(proof.m2.to_vec(), from_hex(v.m2), "M2 {name}");
        }
    }
}

#[test]
fn edge_cases_of_both_variants() {
    for edge in EDGES {
        for (padding, expected) in [
            (Padding::NoLeadingZeros, &edge.pysrp),
            (Padding::GroupLength, &edge.rclone),
        ] {
            let client = SrpClient::with_secret(&from_hex(edge.a), padding).expect("client");
            assert_eq!(
                client.public_a(),
                from_hex(expected.public_a),
                "A {} {padding:?}",
                edge.name
            );
            let proof = client
                .process_challenge(
                    EDGE_ACCOUNT,
                    &from_hex(EDGE_DERIVED),
                    &from_hex(EDGE_SALT),
                    &from_hex(edge.b),
                )
                .expect("exchange");
            assert_eq!(
                proof.k.to_vec(),
                from_hex(expected.k),
                "K {} {padding:?}",
                edge.name
            );
            assert_eq!(
                proof.m1.to_vec(),
                from_hex(expected.m1),
                "M1 {} {padding:?}",
                edge.name
            );
            assert_eq!(
                proof.m2.to_vec(),
                from_hex(expected.m2),
                "M2 {} {padding:?}",
                edge.name
            );
        }
        // The variants really differ in the edge cases.
        assert_ne!(edge.pysrp.m1, edge.rclone.m1, "{}", edge.name);
    }
}

#[test]
fn invalid_inputs() {
    let client = SrpClient::with_secret(&[7u8; 32], Padding::default()).expect("client");
    let n = from_hex(concat!(
        "AC6BDB41324A9A9BF166DE5E1389582FAF72B6651987EE07FC3192943DB56050",
        "A37329CBB4A099ED8193E0757767A13DD52312AB4B03310DCD7F48A9DA04FD50",
        "E8083969EDB767B0CF6095179A163AB3661A05FBD5FAAAE82918A9962F0B93B8",
        "55F97993EC975EEAA80D740ADBF4FF747359D041D5C33EA71D281E446B14773B",
        "CA97B43A23FB801676BD207A436C6481F1D2B9078717461A5B9D32E688F87748",
        "544523B524B0D57D5EA77A2775D2ECFA032CFBDBF52FB3786160279004E57AE6",
        "AF874E7303CE53299CCC041C7BC308D82A5698F3A8D0C38271AE35F8E9DBFBB6",
        "94B5C803D89F7AE435DE236D525F54759B65E372FCD68EF20FA7111F9E4AFF73",
    ));
    let mut above_n = n.clone();
    if let Some(last) = above_n.last_mut() {
        *last += 1;
    }
    let cases: Vec<(&str, Vec<u8>, SrpError)> = vec![
        ("B = 0", vec![0], SrpError::InvalidB),
        ("empty B", vec![], SrpError::InvalidB),
        ("B = N", n.clone(), SrpError::InvalidB),
        ("B > N", above_n, SrpError::InvalidB),
    ];
    for (name, b, error) in cases {
        assert_eq!(
            client.process_challenge("u", b"p", b"s", &b),
            Err(error),
            "{name}"
        );
    }
    assert_eq!(
        SrpClient::with_secret(&[0u8; 32], Padding::default()).map(|_| ()),
        Err(SrpError::InvalidSecret)
    );
    assert_eq!(
        derive_password("p", b"s", 0, Protocol::S2k),
        Err(SrpError::ZeroIterations)
    );
    assert_eq!(
        Protocol::from_name("s2k_xyz"),
        Err(SrpError::UnknownProtocol("s2k_xyz".into()))
    );
    for p in [Protocol::S2k, Protocol::S2kFo] {
        assert_eq!(Protocol::from_name(p.name()), Ok(p));
    }
}

#[test]
fn random_client() {
    let a = SrpClient::new(Padding::default()).expect("randomness");
    let b = SrpClient::new(Padding::default()).expect("randomness");
    assert_eq!(a.public_a().len(), 256);
    assert_ne!(a.public_a(), b.public_a());
    assert!(
        !format!("{a:?}").contains(" a:"),
        "the secret is not printed"
    );
}
