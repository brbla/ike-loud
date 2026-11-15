//! Tři stavy, ze kterých se sestavuje plán.

use std::time::Duration;

use ikeloud_safety::{Cesta, Etag};

/// Otisk obsahu lokálního souboru (kontrolní součet).
///
/// O změně na Linuxu rozhoduje obsah, nikdy čas změny: hodiny počítače
/// mohou jít špatně.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OtiskObsahu(String);

impl OtiskObsahu {
    /// Otisk z textové podoby kontrolního součtu.
    pub fn new(otisk: impl Into<String>) -> Self {
        Self(otisk.into())
    }
}

/// Soubor v lokální složce při posledním průchodu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LokalniSoubor {
    /// Relativní cesta tak, jak je na disku.
    pub cesta: Cesta,
    /// Otisk obsahu.
    pub otisk: OtiskObsahu,
    /// Velikost v bajtech.
    pub velikost: u64,
    /// Jak dlouho se soubor nezměnil.
    pub stabilni_po: Duration,
}

/// Soubor na serveru podle posledního výpisu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VzdalenySoubor {
    /// Relativní cesta tak, jak ji vrátil server.
    pub cesta: Cesta,
    /// Verze na serveru.
    pub etag: Etag,
    /// Velikost v bajtech.
    pub velikost: u64,
}

/// Poslední společný stav souboru (uložený v SQLite).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpolecnyStav {
    /// Relativní cesta.
    pub cesta: Cesta,
    /// Otisk obsahu, který byl na obou stranách.
    pub otisk: OtiskObsahu,
    /// Etag na serveru po poslední synchronizaci.
    pub etag: Etag,
}

/// Všechny tři stavy pro jeden běh plánovače.
#[derive(Debug, Clone, Default)]
pub struct Stavy {
    /// Soubory na Linuxu.
    pub lokalni: Vec<LokalniSoubor>,
    /// Soubory na iCloudu.
    pub vzdalene: Vec<VzdalenySoubor>,
    /// Složky na iCloudu (kvůli vytváření chybějících složek).
    pub vzdalene_slozky: Vec<Cesta>,
    /// Poslední společný stav.
    pub spolecne: Vec<SpolecnyStav>,
}
