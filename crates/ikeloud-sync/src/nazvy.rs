//! Porovnávání názvů: po Unicode normalizaci (NFC) a bez ohledu na velikost
//! písmen. „Příloha“ uložená z Macu (rozložené znaky, NFD) a z Linuxu (NFC)
//! je tentýž soubor; „Foto.jpg“ a „foto.jpg“ jsou pro iCloud tentýž název.
//!
//! Jediný obal knihovny `unicode-normalization`.

use unicode_normalization::UnicodeNormalization;

/// Text v normalizované podobě NFC.
pub fn nfc(text: &str) -> String {
    text.nfc().collect()
}

/// Klíč, podle kterého se párují soubory ze tří stavů.
pub fn klic(cesta: &str) -> String {
    nfc(&nfc(cesta).to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::{klic, nfc};

    #[test]
    fn normalizace() {
        let pripady: &[(&str, &str, &str)] = &[
            ("ASCII beze změny", "abc", "abc"),
            ("NFD z Macu → NFC", "Pr\u{30c}i\u{301}loha", "Příloha"),
            ("NFC zůstane", "Příloha", "Příloha"),
            ("Ångström: znak jednotky → písmeno", "\u{212b}", "\u{c5}"),
            ("prázdný", "", ""),
        ];
        for (popis, vstup, vystup) in pripady {
            assert_eq!(nfc(vstup), *vystup, "{popis}");
        }
    }

    #[test]
    fn klice() {
        let stejne: &[(&str, &str, &str)] = &[
            ("velikost písmen", "Foto.JPG", "foto.jpg"),
            ("NFD a NFC", "Pr\u{30c}i\u{301}loha.pdf", "Příloha.pdf"),
            (
                "NFD a velká písmena",
                "PR\u{30c}I\u{301}LOHA.pdf",
                "příloha.pdf",
            ),
            ("složky", "Dokumenty/Smlouva.odt", "dokumenty/smlouva.odt"),
        ];
        for (popis, a, b) in stejne {
            assert_eq!(klic(a), klic(b), "{popis}");
        }
        let ruzne: &[(&str, &str, &str)] = &[
            ("diakritika není velikost", "Prilóha", "Príloha"),
            ("jiná složka", "a/x", "b/x"),
            ("mezera navíc", "a.txt", "a .txt"),
        ];
        for (popis, a, b) in ruzne {
            assert_ne!(klic(a), klic(b), "{popis}");
        }
    }
}
