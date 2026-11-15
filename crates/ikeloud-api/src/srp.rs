//! Client side of SRP-6a in the variant Apple's sign-in uses.
//!
//! Ported from the `iclouddrive` backend of rclone (`api/srp.go`, MIT) and
//! verified against the pysrp library used by pyicloud (MIT). Compared to
//! RFC 5054:
//!
//! - the password is derived first: PBKDF2-HMAC-SHA256(SHA256(password)) for
//!   protocol `s2k`; `s2k_fo` uses the hex form of the hash instead;
//! - `x = H(salt | H(":" | derived password))`, without the account name;
//! - `M1 = H(H(N) xor H(g) | H(account) | salt | A | B | K)`, `M2 = H(A | M1 | K)`.
//!
//! The implementations differ in one thing: rclone pads A, B and S with
//! zeros to the group length (256 bytes), pysrp does not. It only matters in
//! ~1 % of sign-ins, when one of the numbers comes out shorter. Which variant
//! the server wants is found by an experiment in milestone 0; hence
//! [`Padding`].
//!
//! Single wrapper of `num-bigint` and `getrandom`; hashes come from
//! `ikeloud-hash`.

use num_bigint::BigUint;

/// Length of group numbers in bytes (2048 bits).
const GROUP_LEN: usize = 256;

/// Prime N of the 2048-bit group from RFC 5054, appendix A, as bytes (big
/// endian), so that building it cannot fail. The test `rfc5054_group`
/// compares it with the hex text of the RFC.
const N_BYTES: [u8; GROUP_LEN] = [
    0xac, 0x6b, 0xdb, 0x41, 0x32, 0x4a, 0x9a, 0x9b, 0xf1, 0x66, 0xde, 0x5e, 0x13, 0x89, 0x58, 0x2f,
    0xaf, 0x72, 0xb6, 0x65, 0x19, 0x87, 0xee, 0x07, 0xfc, 0x31, 0x92, 0x94, 0x3d, 0xb5, 0x60, 0x50,
    0xa3, 0x73, 0x29, 0xcb, 0xb4, 0xa0, 0x99, 0xed, 0x81, 0x93, 0xe0, 0x75, 0x77, 0x67, 0xa1, 0x3d,
    0xd5, 0x23, 0x12, 0xab, 0x4b, 0x03, 0x31, 0x0d, 0xcd, 0x7f, 0x48, 0xa9, 0xda, 0x04, 0xfd, 0x50,
    0xe8, 0x08, 0x39, 0x69, 0xed, 0xb7, 0x67, 0xb0, 0xcf, 0x60, 0x95, 0x17, 0x9a, 0x16, 0x3a, 0xb3,
    0x66, 0x1a, 0x05, 0xfb, 0xd5, 0xfa, 0xaa, 0xe8, 0x29, 0x18, 0xa9, 0x96, 0x2f, 0x0b, 0x93, 0xb8,
    0x55, 0xf9, 0x79, 0x93, 0xec, 0x97, 0x5e, 0xea, 0xa8, 0x0d, 0x74, 0x0a, 0xdb, 0xf4, 0xff, 0x74,
    0x73, 0x59, 0xd0, 0x41, 0xd5, 0xc3, 0x3e, 0xa7, 0x1d, 0x28, 0x1e, 0x44, 0x6b, 0x14, 0x77, 0x3b,
    0xca, 0x97, 0xb4, 0x3a, 0x23, 0xfb, 0x80, 0x16, 0x76, 0xbd, 0x20, 0x7a, 0x43, 0x6c, 0x64, 0x81,
    0xf1, 0xd2, 0xb9, 0x07, 0x87, 0x17, 0x46, 0x1a, 0x5b, 0x9d, 0x32, 0xe6, 0x88, 0xf8, 0x77, 0x48,
    0x54, 0x45, 0x23, 0xb5, 0x24, 0xb0, 0xd5, 0x7d, 0x5e, 0xa7, 0x7a, 0x27, 0x75, 0xd2, 0xec, 0xfa,
    0x03, 0x2c, 0xfb, 0xdb, 0xf5, 0x2f, 0xb3, 0x78, 0x61, 0x60, 0x27, 0x90, 0x04, 0xe5, 0x7a, 0xe6,
    0xaf, 0x87, 0x4e, 0x73, 0x03, 0xce, 0x53, 0x29, 0x9c, 0xcc, 0x04, 0x1c, 0x7b, 0xc3, 0x08, 0xd8,
    0x2a, 0x56, 0x98, 0xf3, 0xa8, 0xd0, 0xc3, 0x82, 0x71, 0xae, 0x35, 0xf8, 0xe9, 0xdb, 0xfb, 0xb6,
    0x94, 0xb5, 0xc8, 0x03, 0xd8, 0x9f, 0x7a, 0xe4, 0x35, 0xde, 0x23, 0x6d, 0x52, 0x5f, 0x54, 0x75,
    0x9b, 0x65, 0xe3, 0x72, 0xfc, 0xd6, 0x8e, 0xf2, 0x0f, 0xa7, 0x11, 0x1f, 0x9e, 0x4a, 0xff, 0x73,
];

/// Group generator.
const G: u32 = 2;

/// Password derivation the server returns in `signin/init`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    /// PBKDF2 of the SHA-256 of the password.
    S2k,
    /// PBKDF2 of the hex form of the SHA-256 of the password.
    S2kFo,
}

impl Protocol {
    /// Protocol from the server text (`"s2k"`, `"s2k_fo"`).
    pub fn from_name(text: &str) -> Result<Self, SrpError> {
        match text {
            "s2k" => Ok(Self::S2k),
            "s2k_fo" => Ok(Self::S2kFo),
            other => Err(SrpError::UnknownProtocol(other.to_string())),
        }
    }

    /// Text sent to the server in the list of supported protocols.
    pub fn name(self) -> &'static str {
        match self {
            Self::S2k => "s2k",
            Self::S2kFo => "s2k_fo",
        }
    }
}

/// How numbers become bytes in M1, M2 and K.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Padding {
    /// A, B and S padded with zeros to 256 bytes (rclone).
    #[default]
    GroupLength,
    /// Without leading zeros (pysrp).
    NoLeadingZeros,
}

/// SRP computation error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SrpError {
    /// The server sent a protocol the client does not know.
    UnknownProtocol(String),
    /// The server sent zero PBKDF2 iterations.
    ZeroIterations,
    /// The server sent B outside 1..N-1 (or a multiple of N).
    InvalidB,
    /// u = 0; SRP-6a requires aborting the sign-in.
    ZeroU,
    /// The client secret is zero.
    InvalidSecret,
    /// The system did not provide random numbers.
    RandomUnavailable,
}

/// Result of the exchange sent in `signin/complete`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proof {
    /// Client proof M1.
    pub m1: [u8; 32],
    /// Expected server proof M2.
    pub m2: [u8; 32],
    /// Shared session key K.
    pub k: [u8; 32],
}

/// Derives the SRP password from the protocol, salt and iteration count from the server.
pub fn derive_password(
    password: &str,
    salt: &[u8],
    iterations: u32,
    protocol: Protocol,
) -> Result<[u8; 32], SrpError> {
    if iterations == 0 {
        return Err(SrpError::ZeroIterations);
    }
    let hash = h(&[password.as_bytes()]);
    let input: Vec<u8> = match protocol {
        Protocol::S2k => hash.to_vec(),
        Protocol::S2kFo => ikeloud_hash::hex(&hash).into_bytes(),
    };
    Ok(ikeloud_hash::pbkdf2_sha256(&input, salt, iterations))
}

/// SRP client for one sign-in.
///
/// The secret `a` is not zeroed when dropped: `BigUint` keeps it in a
/// `Vec<u64>` that safe code cannot wipe, and wiping only some copies would
/// pretend a protection that is not there. The client lives for one sign-in;
/// the daemon keeps secrets out of core dumps instead (milestone 1).
pub struct SrpClient {
    a: BigUint,
    public_a: BigUint,
    padding: Padding,
}

impl std::fmt::Debug for SrpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The secret is never printed.
        f.debug_struct("SrpClient")
            .field("padding", &self.padding)
            .finish_non_exhaustive()
    }
}

impl SrpClient {
    /// New client with a random secret (32 bytes like rclone and pysrp).
    pub fn new(padding: Padding) -> Result<Self, SrpError> {
        let mut secret = [0u8; 32];
        getrandom::fill(&mut secret).map_err(|_| SrpError::RandomUnavailable)?;
        Self::with_secret(&secret, padding)
    }

    /// Client with a given secret. For tests and for the milestone 0 experiment.
    pub fn with_secret(secret: &[u8], padding: Padding) -> Result<Self, SrpError> {
        let a = BigUint::from_bytes_be(secret);
        if a == BigUint::ZERO {
            return Err(SrpError::InvalidSecret);
        }
        let public_a = BigUint::from(G).modpow(&a, &n());
        Ok(Self {
            a,
            public_a,
            padding,
        })
    }

    /// Public value A sent in `signin/init`.
    pub fn public_a(&self) -> Vec<u8> {
        self.bytes(&self.public_a)
    }

    /// Computes the proof from the server response (`salt`, `b`).
    pub fn process_challenge(
        &self,
        account: &str,
        derived_password: &[u8],
        salt: &[u8],
        b: &[u8],
    ) -> Result<Proof, SrpError> {
        let n = n();
        let g = BigUint::from(G);
        let b = BigUint::from_bytes_be(b);
        if b == BigUint::ZERO || b >= n {
            return Err(SrpError::InvalidB);
        }

        let k = number(&h(&[&n.to_bytes_be(), &pad(&g)]));
        let u = number(&h(&[&pad(&self.public_a), &pad(&b)]));
        if u == BigUint::ZERO {
            return Err(SrpError::ZeroU);
        }
        let x = number(&h(&[salt, &h(&[b":", derived_password])]));

        // S = (B - k·g^x)^(a + u·x) mod N, subtraction in modular arithmetic.
        let kv = (k * g.modpow(&x, &n)) % &n;
        let base = (&b + &n - kv) % &n;
        let s = base.modpow(&(&self.a + &u * &x), &n);

        let key = h(&[&self.bytes(&s)]);
        let a_bytes = self.bytes(&self.public_a);
        let b_bytes = self.bytes(&b);

        let h_n = h(&[&n.to_bytes_be()]);
        let h_g = h(&[&pad(&g)]);
        let mut xor = [0u8; 32];
        for ((byte, n_byte), g_byte) in xor.iter_mut().zip(h_n).zip(h_g) {
            *byte = n_byte ^ g_byte;
        }
        let m1 = h(&[
            &xor,
            &h(&[account.as_bytes()]),
            salt,
            &a_bytes,
            &b_bytes,
            &key,
        ]);
        let m2 = h(&[&a_bytes, &m1, &key]);
        Ok(Proof { m1, m2, k: key })
    }

    fn bytes(&self, value: &BigUint) -> Vec<u8> {
        match self.padding {
            Padding::GroupLength => pad(value),
            Padding::NoLeadingZeros => value.to_bytes_be(),
        }
    }
}

fn n() -> BigUint {
    BigUint::from_bytes_be(&N_BYTES)
}

fn h(parts: &[&[u8]]) -> [u8; 32] {
    ikeloud_hash::sha256(parts)
}

fn number(bytes: &[u8]) -> BigUint {
    BigUint::from_bytes_be(bytes)
}

/// A number as 256 bytes with leading zeros.
fn pad(value: &BigUint) -> Vec<u8> {
    let bytes = value.to_bytes_be();
    let mut output = vec![0u8; GROUP_LEN.saturating_sub(bytes.len())];
    output.extend_from_slice(&bytes);
    output
}

#[cfg(test)]
mod tests {
    use super::{BigUint, n};

    /// RFC 5054, appendix A, 2048-bit group, as printed there.
    const N_RFC_HEX: &[u8] = b"AC6BDB41324A9A9BF166DE5E1389582FAF72B6651987EE07FC3192943DB56050\
A37329CBB4A099ED8193E0757767A13DD52312AB4B03310DCD7F48A9DA04FD50\
E8083969EDB767B0CF6095179A163AB3661A05FBD5FAAAE82918A9962F0B93B8\
55F97993EC975EEAA80D740ADBF4FF747359D041D5C33EA71D281E446B14773B\
CA97B43A23FB801676BD207A436C6481F1D2B9078717461A5B9D32E688F87748\
544523B524B0D57D5EA77A2775D2ECFA032CFBDBF52FB3786160279004E57AE6\
AF874E7303CE53299CCC041C7BC308D82A5698F3A8D0C38271AE35F8E9DBFBB6\
94B5C803D89F7AE435DE236D525F54759B65E372FCD68EF20FA7111F9E4AFF73";

    #[test]
    fn rfc5054_group() {
        let n = n();
        assert_eq!(Some(n.clone()), BigUint::parse_bytes(N_RFC_HEX, 16));
        assert_eq!(n.bits(), 2048);
        assert_eq!(n.to_bytes_be().first(), Some(&0xAC));
        assert_eq!(n.to_bytes_be().last(), Some(&0x73));
        assert!(n > BigUint::from(2u32));
    }
}
