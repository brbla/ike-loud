//! Akce, které synchronizace navrhuje a bezpečnostní vrstva posuzuje.

use std::fmt;
use std::time::Duration;

/// Relativní cesta v synchronizované složce, části oddělené `/`.
///
/// Platnost názvů posuzuje [`crate::nazvy`]; tady se jen nese.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Cesta(String);

impl Cesta {
    /// Vytvoří cestu z textu tak, jak ho poslal engine.
    pub fn new(cesta: impl Into<String>) -> Self {
        Self(cesta.into())
    }

    /// Cesta jako text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Nadřazená složka (prázdný text pro kořen).
    pub fn rodic(&self) -> &str {
        self.0.rsplit_once('/').map_or("", |(rodic, _)| rodic)
    }

    /// Poslední část cesty.
    pub fn nazev(&self) -> &str {
        self.0
            .rsplit_once('/')
            .map_or(self.0.as_str(), |(_, nazev)| nazev)
    }
}

impl fmt::Display for Cesta {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Verze položky na serveru.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Etag(String);

impl Etag {
    /// Vytvoří etag z hodnoty, kterou vrátil server.
    pub fn new(etag: impl Into<String>) -> Self {
        Self(etag.into())
    }

    /// Etag jako text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Jedna akce plánu.
///
/// Trvalé smazání na serveru tu záměrně není: mazání jde vždy do koše.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Akce {
    /// Nahrát lokální soubor na server.
    Nahrat {
        /// Kam na serveru.
        cesta: Cesta,
        /// Verze na serveru, kterou engine zná; `None` = soubor na serveru není.
        ocekavany_etag: Option<Etag>,
        /// Velikost v bajtech.
        velikost: u64,
        /// Jak dlouho se lokální soubor nezměnil (velikost ani čas změny).
        stabilni_po: Duration,
    },
    /// Uložit lokální verzi jako konfliktní kopii a nahrát ji jako nový soubor.
    NahratKonfliktniKopii {
        /// Název konfliktní kopie na serveru.
        cesta: Cesta,
        /// Velikost v bajtech.
        velikost: u64,
    },
    /// Vytvořit složku na serveru.
    VytvoritSlozkuNaServeru {
        /// Cesta nové složky.
        cesta: Cesta,
    },
    /// Přesunout položku na serveru do „Nedávno smazané“.
    PresunoutNaServeruDoKose {
        /// Co smazat.
        cesta: Cesta,
        /// Verze, kterou engine zná.
        ocekavany_etag: Etag,
    },
    /// Přejmenovat nebo přesunout položku na serveru (podle ID dokumentu).
    PresunoutNaServeru {
        /// Odkud.
        z: Cesta,
        /// Kam.
        na: Cesta,
        /// Verze, kterou engine zná.
        ocekavany_etag: Etag,
    },
    /// Stáhnout soubor ze serveru.
    Stahnout {
        /// Co stáhnout.
        cesta: Cesta,
        /// Velikost v bajtech.
        velikost: u64,
        /// Přepíše existující lokální soubor (předchozí verze jde do historie).
        prepise_lokalni: bool,
    },
    /// Přesunout lokální soubor do koše systému.
    PresunoutLokalneDoKose {
        /// Co smazat.
        cesta: Cesta,
    },
    /// Přejmenovat nebo přesunout lokální soubor.
    PresunoutLokalne {
        /// Odkud.
        z: Cesta,
        /// Kam.
        na: Cesta,
    },
}

impl Akce {
    /// Cesta, které se akce týká (u přesunu cíl).
    pub fn cesta(&self) -> &Cesta {
        match self {
            Self::Nahrat { cesta, .. }
            | Self::NahratKonfliktniKopii { cesta, .. }
            | Self::VytvoritSlozkuNaServeru { cesta }
            | Self::PresunoutNaServeruDoKose { cesta, .. }
            | Self::Stahnout { cesta, .. }
            | Self::PresunoutLokalneDoKose { cesta } => cesta,
            Self::PresunoutNaServeru { na, .. } | Self::PresunoutLokalne { na, .. } => na,
        }
    }

    /// Mění akce něco na serveru?
    pub fn zapisuje_na_server(&self) -> bool {
        matches!(
            self,
            Self::Nahrat { .. }
                | Self::NahratKonfliktniKopii { .. }
                | Self::VytvoritSlozkuNaServeru { .. }
                | Self::PresunoutNaServeruDoKose { .. }
                | Self::PresunoutNaServeru { .. }
        )
    }

    /// Maže nebo přepisuje existující obsah? To počítá brzda hromadných změn.
    pub fn maze_nebo_prepisuje(&self) -> bool {
        match self {
            Self::Nahrat { ocekavany_etag, .. } => ocekavany_etag.is_some(),
            Self::Stahnout {
                prepise_lokalni, ..
            } => *prepise_lokalni,
            Self::PresunoutNaServeruDoKose { .. } | Self::PresunoutLokalneDoKose { .. } => true,
            Self::NahratKonfliktniKopii { .. }
            | Self::VytvoritSlozkuNaServeru { .. }
            | Self::PresunoutNaServeru { .. }
            | Self::PresunoutLokalne { .. } => false,
        }
    }

    /// Kolik bajtů akce přidá na server.
    pub fn bajtu_na_server(&self) -> u64 {
        match self {
            Self::Nahrat { velikost, .. } | Self::NahratKonfliktniKopii { velikost, .. } => {
                *velikost
            }
            _ => 0,
        }
    }

    /// Kolik bajtů akce přidá na lokální disk.
    pub fn bajtu_na_disk(&self) -> u64 {
        match self {
            Self::Stahnout { velikost, .. } => *velikost,
            _ => 0,
        }
    }
}
