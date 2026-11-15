//! Comparing names: after Unicode normalization (NFC) and regardless of
//! letter case. "Příloha" saved from a Mac (decomposed, NFD) and from Linux
//! (NFC) is the same file; "Photo.jpg" and "photo.jpg" are the same name for
//! iCloud.
//!
//! Single wrapper of the `unicode-normalization` crate.

use unicode_normalization::UnicodeNormalization;

/// Text in NFC normal form.
pub fn nfc(text: &str) -> String {
    text.nfc().collect()
}

/// Key that pairs files across the three states.
pub fn key(path: &str) -> String {
    nfc(&nfc(path).to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::{key, nfc};

    #[test]
    fn normalization() {
        let cases: &[(&str, &str, &str)] = &[
            ("ASCII unchanged", "abc", "abc"),
            ("NFD from a Mac → NFC", "Pr\u{30c}i\u{301}loha", "Příloha"),
            ("NFC stays", "Příloha", "Příloha"),
            ("Ångström: unit sign → letter", "\u{212b}", "\u{c5}"),
            ("empty", "", ""),
        ];
        for (name, input, output) in cases {
            assert_eq!(nfc(input), *output, "{name}");
        }
    }

    #[test]
    fn keys() {
        let same: &[(&str, &str, &str)] = &[
            ("letter case", "Photo.JPG", "photo.jpg"),
            ("NFD and NFC", "Pr\u{30c}i\u{301}loha.pdf", "Příloha.pdf"),
            (
                "NFD and upper case",
                "PR\u{30c}I\u{301}LOHA.pdf",
                "příloha.pdf",
            ),
            (
                "folders",
                "Documents/Contract.odt",
                "documents/contract.odt",
            ),
        ];
        for (name, a, b) in same {
            assert_eq!(key(a), key(b), "{name}");
        }
        let different: &[(&str, &str, &str)] = &[
            ("diacritics are not case", "Prilóha", "Príloha"),
            ("another folder", "a/x", "b/x"),
            ("extra space", "a.txt", "a .txt"),
        ];
        for (name, a, b) in different {
            assert_ne!(key(a), key(b), "{name}");
        }
    }
}
