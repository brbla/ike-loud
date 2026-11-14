//! Pravidla pro názvy souborů a pro soubory, které se nesynchronizují.
//!
//! Unicode normalizaci (NFC) dělá engine dřív, než plán pošle sem; tento
//! crate používá jen standardní knihovnu.

/// Proč název nejde nahrát na server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChybaNazvu {
    /// Prázdná cesta nebo prázdná část cesty (`a//b`, `/a`).
    PrazdnaCast,
    /// Část cesty je `.` nebo `..`.
    TeckaNeboDveTecky,
    /// Název obsahuje znak, který iCloud nepřijme (nulový znak, řídicí znak).
    NepovolenyZnak(char),
    /// Část cesty je delší než 255 bajtů.
    PrilisDlouhy,
}

/// Nejdelší povolená část cesty v bajtech (UTF-8).
pub const MAX_DELKA_NAZVU: usize = 255;

/// Ověří, že cestu lze nahrát na server.
///
/// Pravidla jsou záměrně přísná. Přesná pravidla serveru se zjistí
/// v milníku 0 a doplní se sem s testy.
pub fn over_cestu(cesta: &str) -> Result<(), ChybaNazvu> {
    if cesta.is_empty() {
        return Err(ChybaNazvu::PrazdnaCast);
    }
    for cast in cesta.split('/') {
        over_nazev(cast)?;
    }
    Ok(())
}

fn over_nazev(nazev: &str) -> Result<(), ChybaNazvu> {
    if nazev.is_empty() {
        return Err(ChybaNazvu::PrazdnaCast);
    }
    if nazev == "." || nazev == ".." {
        return Err(ChybaNazvu::TeckaNeboDveTecky);
    }
    if nazev.len() > MAX_DELKA_NAZVU {
        return Err(ChybaNazvu::PrilisDlouhy);
    }
    if let Some(znak) = nazev.chars().find(|z| z.is_control()) {
        return Err(ChybaNazvu::NepovolenyZnak(znak));
    }
    Ok(())
}

/// Je to dočasný nebo zamykací soubor, který se nikdy nesynchronizuje?
///
/// Zadání: `.~lock.*#`, `*.swp`, `*.part`, `.DS_Store`.
pub fn je_ignorovany(nazev: &str) -> bool {
    (nazev.starts_with(".~lock.") && nazev.ends_with('#'))
        || nazev.ends_with(".swp")
        || nazev.ends_with(".part")
        || nazev == ".DS_Store"
}

/// Klíč pro porovnání názvů bez ohledu na velikost písmen.
///
/// iCloud velká a malá písmena nerozlišuje, Linux ano.
pub fn klic_bez_velikosti(nazev: &str) -> String {
    nazev.to_lowercase()
}
