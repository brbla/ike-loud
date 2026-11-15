//! Klientská strana SRP-6a ve variantě, kterou používá přihlášení Applu.
//!
//! Převzato z backendu `iclouddrive` v rclone (`api/srp.go`, MIT) a ověřeno
//! proti knihovně pysrp, kterou používá pyicloud (MIT). Oproti RFC 5054:
//!
//! - heslo se nejdřív odvodí: PBKDF2-HMAC-SHA256(SHA256(heslo)) pro
//!   protokol `s2k`, u `s2k_fo` se místo hashe použije jeho hex zápis;
//! - `x = H(salt | H(":" | odvozené heslo))`, bez jména účtu;
//! - `M1 = H(H(N) xor H(g) | H(účet) | salt | A | B | K)`, `M2 = H(A | M1 | K)`.
//!
//! Implementace se liší v jedné věci: rclone doplňuje A, B a S nulami na
//! délku skupiny (256 bajtů), pysrp ne. Liší se to jen u ~1 % přihlášení,
//! kdy některé z čísel vyjde kratší. Kterou variantu chce server, zjistí
//! pokus v milníku 0; proto [`Zarovnani`].
//!
//! Jediný obal knihoven `num-bigint`, `sha2`, `pbkdf2` a `getrandom`.

use num_bigint::BigUint;
use sha2::{Digest, Sha256};

/// Délka čísel skupiny v bajtech (2048 bitů).
const DELKA_SKUPINY: usize = 256;

/// Prvočíslo N skupiny 2048 bitů z RFC 5054, příloha A.
const N_HEX: &[u8] = b"AC6BDB41324A9A9BF166DE5E1389582FAF72B6651987EE07FC3192943DB56050\
A37329CBB4A099ED8193E0757767A13DD52312AB4B03310DCD7F48A9DA04FD50\
E8083969EDB767B0CF6095179A163AB3661A05FBD5FAAAE82918A9962F0B93B8\
55F97993EC975EEAA80D740ADBF4FF747359D041D5C33EA71D281E446B14773B\
CA97B43A23FB801676BD207A436C6481F1D2B9078717461A5B9D32E688F87748\
544523B524B0D57D5EA77A2775D2ECFA032CFBDBF52FB3786160279004E57AE6\
AF874E7303CE53299CCC041C7BC308D82A5698F3A8D0C38271AE35F8E9DBFBB6\
94B5C803D89F7AE435DE236D525F54759B65E372FCD68EF20FA7111F9E4AFF73";

/// Generátor skupiny.
const G: u32 = 2;

/// Způsob odvození hesla, který vrátí server v `signin/init`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protokol {
    /// PBKDF2 ze SHA-256 hesla.
    S2k,
    /// PBKDF2 z hex zápisu SHA-256 hesla.
    S2kFo,
}

impl Protokol {
    /// Protokol podle textu ze serveru (`"s2k"`, `"s2k_fo"`).
    pub fn z_textu(text: &str) -> Result<Self, ChybaSrp> {
        match text {
            "s2k" => Ok(Self::S2k),
            "s2k_fo" => Ok(Self::S2kFo),
            jiny => Err(ChybaSrp::NeznamyProtokol(jiny.to_string())),
        }
    }

    /// Text, který se posílá serveru v seznamu podporovaných protokolů.
    pub fn jako_text(self) -> &'static str {
        match self {
            Self::S2k => "s2k",
            Self::S2kFo => "s2k_fo",
        }
    }
}

/// Jak převádět čísla na bajty v M1, M2 a K.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Zarovnani {
    /// A, B a S doplněné nulami na 256 bajtů (rclone).
    #[default]
    NaDelkuSkupiny,
    /// Bez úvodních nul (pysrp).
    BezUvodnichNul,
}

/// Chyba výpočtu SRP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChybaSrp {
    /// Server poslal protokol, který klient nezná.
    NeznamyProtokol(String),
    /// Server poslal nulový počet iterací PBKDF2.
    NulovyPocetIteraci,
    /// Server poslal B mimo rozsah 1..N-1 (nebo násobek N).
    NeplatneB,
    /// Vyšlo u = 0; podle SRP-6a se musí přihlášení přerušit.
    NuloveU,
    /// Tajné číslo klienta je nula.
    NeplatneTajemstvi,
    /// Systém nedal náhodná čísla.
    NahodnostNedostupna,
}

/// Výsledek výměny, který se posílá v `signin/complete`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dukaz {
    /// Důkaz klienta M1.
    pub m1: [u8; 32],
    /// Očekávaný důkaz serveru M2.
    pub m2: [u8; 32],
    /// Sdílený klíč relace K.
    pub k: [u8; 32],
}

/// Odvodí heslo pro SRP podle protokolu, saltu a počtu iterací ze serveru.
pub fn odvod_heslo(
    heslo: &str,
    salt: &[u8],
    iterace: u32,
    protokol: Protokol,
) -> Result<[u8; 32], ChybaSrp> {
    if iterace == 0 {
        return Err(ChybaSrp::NulovyPocetIteraci);
    }
    let hash = h(&[heslo.as_bytes()]);
    let vstup: Vec<u8> = match protokol {
        Protokol::S2k => hash.to_vec(),
        Protokol::S2kFo => hex(&hash).into_bytes(),
    };
    let mut vystup = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<Sha256>(&vstup, salt, iterace, &mut vystup);
    Ok(vystup)
}

/// Klient SRP pro jedno přihlášení.
pub struct SrpKlient {
    a: BigUint,
    verejne_a: BigUint,
    zarovnani: Zarovnani,
}

impl std::fmt::Debug for SrpKlient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Tajné číslo se nikdy nevypisuje.
        f.debug_struct("SrpKlient")
            .field("zarovnani", &self.zarovnani)
            .finish_non_exhaustive()
    }
}

impl SrpKlient {
    /// Nový klient s náhodným tajným číslem (32 bajtů jako rclone a pysrp).
    pub fn new(zarovnani: Zarovnani) -> Result<Self, ChybaSrp> {
        let mut tajemstvi = [0u8; 32];
        getrandom::fill(&mut tajemstvi).map_err(|_| ChybaSrp::NahodnostNedostupna)?;
        Self::s_tajemstvim(&tajemstvi, zarovnani)
    }

    /// Klient s daným tajným číslem. Pro testy a pro pokus v milníku 0.
    pub fn s_tajemstvim(tajemstvi: &[u8], zarovnani: Zarovnani) -> Result<Self, ChybaSrp> {
        let a = BigUint::from_bytes_be(tajemstvi);
        if a == BigUint::ZERO {
            return Err(ChybaSrp::NeplatneTajemstvi);
        }
        let verejne_a = BigUint::from(G).modpow(&a, &n());
        Ok(Self {
            a,
            verejne_a,
            zarovnani,
        })
    }

    /// Veřejné číslo A, které se posílá v `signin/init`.
    pub fn verejne_a(&self) -> Vec<u8> {
        self.bajty(&self.verejne_a)
    }

    /// Spočítá důkaz z odpovědi serveru (`salt`, `b`).
    pub fn zpracuj_vyzvu(
        &self,
        ucet: &str,
        odvozene_heslo: &[u8],
        salt: &[u8],
        b: &[u8],
    ) -> Result<Dukaz, ChybaSrp> {
        let n = n();
        let g = BigUint::from(G);
        let b = BigUint::from_bytes_be(b);
        if b == BigUint::ZERO || b >= n {
            return Err(ChybaSrp::NeplatneB);
        }

        let k = cislo(&h(&[&n.to_bytes_be(), &doplnit(&g)]));
        let u = cislo(&h(&[&doplnit(&self.verejne_a), &doplnit(&b)]));
        if u == BigUint::ZERO {
            return Err(ChybaSrp::NuloveU);
        }
        let x = cislo(&h(&[salt, &h(&[b":", odvozene_heslo])]));

        // S = (B - k·g^x)^(a + u·x) mod N, odečítání v modulární aritmetice.
        let kv = (k * g.modpow(&x, &n)) % &n;
        let zaklad = (&b + &n - kv) % &n;
        let s = zaklad.modpow(&(&self.a + &u * &x), &n);

        let klic = h(&[&self.bajty(&s)]);
        let a_bajty = self.bajty(&self.verejne_a);
        let b_bajty = self.bajty(&b);

        let h_n = h(&[&n.to_bytes_be()]);
        let h_g = h(&[&doplnit(&g)]);
        let mut xor = [0u8; 32];
        for (i, bajt) in xor.iter_mut().enumerate() {
            *bajt = h_n.get(i).copied().unwrap_or(0) ^ h_g.get(i).copied().unwrap_or(0);
        }
        let m1 = h(&[
            &xor,
            &h(&[ucet.as_bytes()]),
            salt,
            &a_bajty,
            &b_bajty,
            &klic,
        ]);
        let m2 = h(&[&a_bajty, &m1, &klic]);
        Ok(Dukaz { m1, m2, k: klic })
    }

    fn bajty(&self, cislo: &BigUint) -> Vec<u8> {
        match self.zarovnani {
            Zarovnani::NaDelkuSkupiny => doplnit(cislo),
            Zarovnani::BezUvodnichNul => cislo.to_bytes_be(),
        }
    }
}

fn n() -> BigUint {
    // Konstanta je platný hex; prázdný výsledek by jen shodil testy.
    BigUint::parse_bytes(N_HEX, 16).unwrap_or_default()
}

fn h(casti: &[&[u8]]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for cast in casti {
        hasher.update(cast);
    }
    hasher.finalize().into()
}

fn cislo(bajty: &[u8]) -> BigUint {
    BigUint::from_bytes_be(bajty)
}

/// Číslo jako 256 bajtů s úvodními nulami.
fn doplnit(cislo: &BigUint) -> Vec<u8> {
    let bajty = cislo.to_bytes_be();
    let mut vystup = vec![0u8; DELKA_SKUPINY.saturating_sub(bajty.len())];
    vystup.extend_from_slice(&bajty);
    vystup
}

fn hex(bajty: &[u8]) -> String {
    use std::fmt::Write;
    bajty
        .iter()
        .fold(String::with_capacity(bajty.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

#[cfg(test)]
mod tests {
    use super::{BigUint, n};

    #[test]
    fn skupina_rfc5054() {
        let n = n();
        assert_eq!(n.bits(), 2048);
        assert_eq!(n.to_bytes_be().first(), Some(&0xAC));
        assert_eq!(n.to_bytes_be().last(), Some(&0x73));
        assert!(n > BigUint::from(2u32));
    }
}
