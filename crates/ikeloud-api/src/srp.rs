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
//! Single wrapper of `num-bigint`, `sha2`, `pbkdf2` and `getrandom`.

use num_bigint::BigUint;
use sha2::{Digest, Sha256};

/// Length of group numbers in bytes (2048 bits).
const GROUP_LEN: usize = 256;

/// Prime N of the 2048-bit group from RFC 5054, appendix A.
const N_HEX: &[u8] = b"AC6BDB41324A9A9BF166DE5E1389582FAF72B6651987EE07FC3192943DB56050\
A37329CBB4A099ED8193E0757767A13DD52312AB4B03310DCD7F48A9DA04FD50\
E8083969EDB767B0CF6095179A163AB3661A05FBD5FAAAE82918A9962F0B93B8\
55F97993EC975EEAA80D740ADBF4FF747359D041D5C33EA71D281E446B14773B\
CA97B43A23FB801676BD207A436C6481F1D2B9078717461A5B9D32E688F87748\
544523B524B0D57D5EA77A2775D2ECFA032CFBDBF52FB3786160279004E57AE6\
AF874E7303CE53299CCC041C7BC308D82A5698F3A8D0C38271AE35F8E9DBFBB6\
94B5C803D89F7AE435DE236D525F54759B65E372FCD68EF20FA7111F9E4AFF73";

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
        Protocol::S2kFo => hex(&hash).into_bytes(),
    };
    let mut output = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<Sha256>(&input, salt, iterations, &mut output);
    Ok(output)
}

/// SRP client for one sign-in.
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
        for (i, byte) in xor.iter_mut().enumerate() {
            *byte = h_n.get(i).copied().unwrap_or(0) ^ h_g.get(i).copied().unwrap_or(0);
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
    // The constant is valid hex; an empty result would only fail the tests.
    BigUint::parse_bytes(N_HEX, 16).unwrap_or_default()
}

fn h(parts: &[&[u8]]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part);
    }
    hasher.finalize().into()
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

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

#[cfg(test)]
mod tests {
    use super::{BigUint, n};

    #[test]
    fn rfc5054_group() {
        let n = n();
        assert_eq!(n.bits(), 2048);
        assert_eq!(n.to_bytes_be().first(), Some(&0xAC));
        assert_eq!(n.to_bytes_be().last(), Some(&0x73));
        assert!(n > BigUint::from(2u32));
    }
}
