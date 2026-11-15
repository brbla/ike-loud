//! Překlady ike loud.
//!
//! Kód nikde nedrží text pro uživatele, jen [`Zprava`]: klíč a argumenty.
//! Texty jsou v souborech Fluent `i18n/<jazyk>/ikeloud.ftl`. Čeština
//! a angličtina jsou zabudované v programu; další jazyk stačí přidat jako
//! složku se souborem (viz [`Preklady::s_adresarem`]), bez nového překladu
//! programu. Chybějící text se vezme z angličtiny.
//!
//! Tento crate je jediný obal knihoven Fluent.

mod jazyk;
mod kopie;
mod zpravy;

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use fluent_bundle::concurrent::FluentBundle;
use fluent_bundle::{FluentArgs, FluentResource, FluentValue};
use unic_langid::LanguageIdentifier;

pub use jazyk::{jazyky_z_promennych, jazyky_z_prostredi};
pub use zpravy::DoZpravy;

/// Název souboru s překlady v každé jazykové složce.
pub const SOUBOR_PREKLADU: &str = "ikeloud.ftl";

/// Záložní jazyk, ze kterého se bere chybějící text.
pub const ZALOZNI_JAZYK: &str = "en";

/// Překlady zabudované v programu: (kód jazyka, obsah souboru).
pub const ZABUDOVANE: &[(&str, &str)] = &[
    ("cs", include_str!("../../../i18n/cs/ikeloud.ftl")),
    ("en", include_str!("../../../i18n/en/ikeloud.ftl")),
];

/// Hodnota argumentu zprávy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hodnota {
    /// Celé číslo; řídí i tvar množného čísla.
    Cislo(u64),
    /// Text, který se nepřekládá (název souboru).
    Text(String),
    /// Velikost v bajtech; zformátuje se podle jazyka („1,5 GB“).
    Velikost(u64),
    /// Vnořená zpráva, přeložená stejným jazykem.
    Zprava(Box<Zprava>),
}

/// Zpráva pro uživatele: klíč z překladového souboru a argumenty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Zprava {
    /// Klíč zprávy v souboru `.ftl`.
    pub klic: &'static str,
    /// Pojmenované argumenty (`{ $nazev }` v textu).
    pub argumenty: Vec<(&'static str, Hodnota)>,
}

impl Zprava {
    /// Zpráva bez argumentů.
    pub fn new(klic: &'static str) -> Self {
        Self {
            klic,
            argumenty: Vec::new(),
        }
    }

    /// Přidá argument.
    #[must_use]
    pub fn s(mut self, nazev: &'static str, hodnota: Hodnota) -> Self {
        self.argumenty.push((nazev, hodnota));
        self
    }
}

/// Chyba při načítání překladů.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChybaPrekladu {
    /// Kód jazyka není platný (např. název složky).
    NeplatnyKodJazyka(String),
    /// Soubor s překlady nejde přečíst.
    Cteni {
        /// Který soubor.
        cesta: PathBuf,
        /// Popis chyby od systému.
        chyba: String,
    },
    /// Soubor s překlady má chybu v zápisu.
    Syntaxe {
        /// Kód jazyka.
        jazyk: String,
        /// Popisy chyb od parseru.
        chyby: Vec<String>,
    },
}

impl fmt::Display for ChybaPrekladu {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NeplatnyKodJazyka(kod) => write!(f, "neplatný kód jazyka: {kod}"),
            Self::Cteni { cesta, chyba } => write!(f, "nejde přečíst {}: {chyba}", cesta.display()),
            Self::Syntaxe { jazyk, chyby } => {
                write!(f, "chyba v překladu {jazyk}: {}", chyby.join("; "))
            }
        }
    }
}

impl std::error::Error for ChybaPrekladu {}

/// Chyba při přeložení konkrétní zprávy v konkrétním jazyce.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChybaFormatovani {
    /// Klíč v překladu chybí.
    ChybiKlic(&'static str),
    /// Text se nepodařilo sestavit (chybějící argument, chybný odkaz).
    Formatovani {
        /// Klíč zprávy.
        klic: &'static str,
        /// Popisy chyb od Fluentu.
        chyby: Vec<String>,
    },
}

/// Načtené překlady pro zvolené pořadí jazyků.
pub struct Preklady {
    /// Bundly v pořadí přednosti; poslední je vždy záložní jazyk.
    bundly: Vec<FluentBundle<FluentResource>>,
}

impl fmt::Debug for Preklady {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Preklady")
            .field("jazyky", &self.jazyky())
            .finish()
    }
}

impl Preklady {
    /// Překlady jen ze zabudovaných jazyků.
    ///
    /// `pozadovane` je pořadí jazyků podle uživatele (např. z
    /// [`jazyky_z_prostredi`]); neznámé jazyky se přeskočí.
    pub fn new(pozadovane: &[String]) -> Result<Self, ChybaPrekladu> {
        Self::ze_zdroju(pozadovane, zabudovane_zdroje())
    }

    /// Překlady ze zabudovaných jazyků a ze složky `adresar`.
    ///
    /// Každá podsložka `adresar/<kód jazyka>/ikeloud.ftl` přidá jazyk, nebo
    /// nahradí zabudovaný překlad stejného jazyka.
    pub fn s_adresarem(pozadovane: &[String], adresar: &Path) -> Result<Self, ChybaPrekladu> {
        let mut zdroje = zabudovane_zdroje();
        zdroje.extend(nacti_adresar(adresar)?);
        Self::ze_zdroju(pozadovane, zdroje)
    }

    fn ze_zdroju(
        pozadovane: &[String],
        zdroje: BTreeMap<String, String>,
    ) -> Result<Self, ChybaPrekladu> {
        let dostupne: Vec<&str> = zdroje.keys().map(String::as_str).collect();
        let poradi = jazyk::vyber(pozadovane, &dostupne);
        let mut bundly = Vec::new();
        for kod in poradi {
            let Some(text) = zdroje.get(&kod) else {
                continue;
            };
            bundly.push(sestav_bundle(&kod, text.clone())?);
        }
        Ok(Self { bundly })
    }

    /// Jazyky v pořadí, ve kterém se hledá text.
    pub fn jazyky(&self) -> Vec<String> {
        self.bundly
            .iter()
            .filter_map(|b| b.locales.first().map(ToString::to_string))
            .collect()
    }

    /// Přeloží zprávu. Když text v prvním jazyce chybí nebo je chybný,
    /// vezme další jazyk; když chybí všude, vrátí klíč.
    pub fn text(&self, zprava: &Zprava) -> String {
        self.bundly
            .iter()
            .find_map(|b| self.formatuj(b, zprava).ok())
            .unwrap_or_else(|| zprava.klic.to_string())
    }

    /// Přeloží zprávu jen prvním jazykem, bez náhrady. Pro testy a kontrolu
    /// úplnosti překladů.
    pub fn text_presne(&self, zprava: &Zprava) -> Result<String, ChybaFormatovani> {
        match self.bundly.first() {
            Some(b) => self.formatuj(b, zprava),
            None => Err(ChybaFormatovani::ChybiKlic(zprava.klic)),
        }
    }

    fn formatuj(
        &self,
        bundle: &FluentBundle<FluentResource>,
        zprava: &Zprava,
    ) -> Result<String, ChybaFormatovani> {
        let vzor = bundle
            .get_message(zprava.klic)
            .and_then(|m| m.value())
            .ok_or(ChybaFormatovani::ChybiKlic(zprava.klic))?;
        let mut argumenty = FluentArgs::new();
        for (nazev, hodnota) in &zprava.argumenty {
            let hodnota: FluentValue<'_> = match hodnota {
                Hodnota::Cislo(n) => FluentValue::from(*n),
                Hodnota::Text(t) => FluentValue::from(t.clone()),
                Hodnota::Velikost(b) => FluentValue::from(self.velikost(bundle, *b)?),
                Hodnota::Zprava(z) => FluentValue::from(self.formatuj(bundle, z)?),
            };
            argumenty.set(*nazev, hodnota);
        }
        let mut chyby = Vec::new();
        let text = bundle.format_pattern(vzor, Some(&argumenty), &mut chyby);
        if chyby.is_empty() {
            Ok(text.into_owned())
        } else {
            Err(ChybaFormatovani::Formatovani {
                klic: zprava.klic,
                chyby: chyby.iter().map(ToString::to_string).collect(),
            })
        }
    }

    /// Velikost v jednotkách SI (1 kB = 1000 B), s jedním desetinným místem
    /// pod 100 jednotek.
    fn velikost(
        &self,
        bundle: &FluentBundle<FluentResource>,
        bajty: u64,
    ) -> Result<String, ChybaFormatovani> {
        let (klic, cislo) = velikost_v_jednotkach(bajty);
        let oddelovac = self.formatuj(bundle, &Zprava::new("oddelovac-desetin"))?;
        let cislo = cislo.replace('.', &oddelovac);
        self.formatuj(
            bundle,
            &Zprava::new(klic).s("hodnota", Hodnota::Text(cislo)),
        )
    }
}

/// Vybere jednotku a zformátuje číslo s tečkou jako oddělovačem desetin.
fn velikost_v_jednotkach(bajty: u64) -> (&'static str, String) {
    const JEDNOTKY: [&str; 4] = ["velikost-kb", "velikost-mb", "velikost-gb", "velikost-tb"];
    if bajty < 1000 {
        return ("velikost-bajty", bajty.to_string());
    }
    let mut klic = "velikost-kb";
    let mut delitel: u64 = 1000;
    for (i, jednotka) in JEDNOTKY.iter().enumerate() {
        klic = jednotka;
        delitel = 1000u64.pow(u32::try_from(i + 1).unwrap_or(1));
        if bajty / delitel < 1000 || i + 1 == JEDNOTKY.len() {
            break;
        }
    }
    // Desetiny zaokrouhlené dolů, ať 999 999 B není „1000 kB“.
    let desetiny = u128::from(bajty) * 10 / u128::from(delitel);
    let cela = desetiny / 10;
    let zbytek = desetiny % 10;
    let text = if cela >= 100 || zbytek == 0 {
        cela.to_string()
    } else {
        format!("{cela}.{zbytek}")
    };
    (klic, text)
}

fn zabudovane_zdroje() -> BTreeMap<String, String> {
    ZABUDOVANE
        .iter()
        .map(|(kod, text)| ((*kod).to_string(), (*text).to_string()))
        .collect()
}

fn nacti_adresar(adresar: &Path) -> Result<BTreeMap<String, String>, ChybaPrekladu> {
    let chyba_cteni = |cesta: &Path, e: std::io::Error| ChybaPrekladu::Cteni {
        cesta: cesta.to_path_buf(),
        chyba: e.to_string(),
    };
    let mut zdroje = BTreeMap::new();
    let polozky = std::fs::read_dir(adresar).map_err(|e| chyba_cteni(adresar, e))?;
    for polozka in polozky {
        let polozka = polozka.map_err(|e| chyba_cteni(adresar, e))?;
        let soubor = polozka.path().join(SOUBOR_PREKLADU);
        if !soubor.is_file() {
            continue;
        }
        let kod = polozka.file_name().to_string_lossy().into_owned();
        kod.parse::<LanguageIdentifier>()
            .map_err(|_| ChybaPrekladu::NeplatnyKodJazyka(kod.clone()))?;
        let text = std::fs::read_to_string(&soubor).map_err(|e| chyba_cteni(&soubor, e))?;
        zdroje.insert(kod, text);
    }
    Ok(zdroje)
}

fn sestav_bundle(kod: &str, text: String) -> Result<FluentBundle<FluentResource>, ChybaPrekladu> {
    let jazyk: LanguageIdentifier = kod
        .parse()
        .map_err(|_| ChybaPrekladu::NeplatnyKodJazyka(kod.to_string()))?;
    let zdroj = FluentResource::try_new(text).map_err(|(_, chyby)| ChybaPrekladu::Syntaxe {
        jazyk: kod.to_string(),
        chyby: chyby.iter().map(ToString::to_string).collect(),
    })?;
    let mut bundle = FluentBundle::new_concurrent(vec![jazyk]);
    // Bez neviditelných znaků pro obousměrný text: GTK si směr řeší samo
    // a v terminálu by ty znaky překážely.
    bundle.set_use_isolating(false);
    bundle
        .add_resource(zdroj)
        .map_err(|chyby| ChybaPrekladu::Syntaxe {
            jazyk: kod.to_string(),
            chyby: chyby.iter().map(ToString::to_string).collect(),
        })?;
    Ok(bundle)
}

#[cfg(test)]
mod tests {
    use super::velikost_v_jednotkach;

    #[test]
    fn velikosti() {
        let pripady: &[(u64, &str, &str)] = &[
            (0, "velikost-bajty", "0"),
            (999, "velikost-bajty", "999"),
            (1000, "velikost-kb", "1"),
            (1500, "velikost-kb", "1.5"),
            (1999, "velikost-kb", "1.9"),
            (99_950, "velikost-kb", "99.9"),
            (100_000, "velikost-kb", "100"),
            (999_999, "velikost-kb", "999"),
            (1_000_000, "velikost-mb", "1"),
            (123_456_789, "velikost-mb", "123"),
            (5_000_000_000, "velikost-gb", "5"),
            (2_500_000_000_000, "velikost-tb", "2.5"),
            (u64::MAX, "velikost-tb", "18446744"),
        ];
        for (bajty, klic, text) in pripady {
            assert_eq!(
                velikost_v_jednotkach(*bajty),
                (*klic, (*text).to_string()),
                "{bajty} B"
            );
        }
    }
}
