//! Standard base64 (RFC 4648, with padding) for the SRP numbers in the sign-in.
//! A dozen lines instead of another dependency.

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encodes bytes.
pub fn encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3).saturating_mul(4));
    for chunk in data.chunks(3) {
        let b = [
            chunk.first().copied().unwrap_or(0),
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                let index = usize::try_from((n >> (18 - 6 * i)) & 63).unwrap_or(0);
                out.push(char::from(ALPHABET.get(index).copied().unwrap_or(b'A')));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Decodes text; `None` when it is not valid base64.
pub fn decode(text: &str) -> Option<Vec<u8>> {
    let text = text.trim();
    if !text.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let bytes = text.as_bytes();
    let chunks = bytes.chunks(4);
    let count = chunks.len();
    for (index, chunk) in chunks.enumerate() {
        let mut n: u32 = 0;
        let mut pad = 0;
        for &c in chunk {
            let value = match c {
                b'=' => {
                    pad += 1;
                    0
                }
                _ if pad > 0 => return None,
                _ => u32::try_from(ALPHABET.iter().position(|&a| a == c)?).ok()?,
            };
            n = (n << 6) | value;
        }
        if pad > 2 || (pad > 0 && index + 1 != count) {
            return None;
        }
        let decoded = n.to_be_bytes();
        out.extend(decoded.get(1..4 - pad)?);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc4648_vectors() {
        for (plain, coded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(encode(plain.as_bytes()), coded);
            assert_eq!(decode(coded).as_deref(), Some(plain.as_bytes()));
        }
    }

    #[test]
    fn all_bytes_round_trip() {
        let data: Vec<u8> = (0..=255).collect();
        assert_eq!(decode(&encode(&data)), Some(data));
    }

    #[test]
    fn invalid_is_refused() {
        for bad in ["Zg=", "Z===", "Zg==Zg==", "Zm9!", "Z=g="] {
            assert_eq!(decode(bad), None, "{bad}");
        }
    }
}
