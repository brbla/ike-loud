//! Co bezpečnostní vrstva ví o okolí, když posuzuje plán.

use std::collections::BTreeMap;
use std::time::Duration;

use crate::akce::{Cesta, Etag};
use crate::posouzeni::PotvrzeniHromadneZmeny;

/// Stav lokální synchronizované složky.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StavLokalniSlozky {
    /// Složka existuje; počet souborů v ní při posledním průchodu.
    Dostupna {
        /// Počet souborů.
        pocet_souboru: u64,
    },
    /// Složka chybí (odpojený disk, smazaný adresář).
    Chybi,
}

/// Proč aplikace jen čte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DuvodJenCteni {
    /// Uživatel si to zapnul v nastavení.
    Nastaveni,
    /// První stažení ještě není dokončené a zkontrolované.
    PrvniStazeni,
    /// API vrátilo neznámou odpověď.
    NeznamaOdpovedApi,
    /// Vzdálená pojistka přepnula klienty do režimu jen pro čtení.
    VzdalenaPojistka,
}

/// Režim synchronizace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rezim {
    /// Obousměrná synchronizace.
    Plny,
    /// Na server se nezapisuje.
    JenCteni(DuvodJenCteni),
}

/// Prahy brzdy hromadných změn.
///
/// Zadání: víc než 50 souborů, nebo víc než 10 souborů a zároveň přes 10 %
/// synchronizovaného obsahu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LimityHromadneZmeny {
    /// Nad tento počet se zastaví vždy.
    pub absolutni: u64,
    /// Nad tento počet se zastaví, když jde zároveň o víc než `procent` obsahu.
    pub minimalni: u64,
    /// Podíl synchronizovaného obsahu v procentech.
    pub procent: u64,
}

impl Default for LimityHromadneZmeny {
    fn default() -> Self {
        Self {
            absolutni: 50,
            minimalni: 10,
            procent: 10,
        }
    }
}

/// Doba, po kterou se soubor nesmí měnit, než se nahraje.
pub const VYCHOZI_DOBA_USTALENI: Duration = Duration::from_secs(5);

/// Všechno, co posouzení plánu potřebuje vědět o okolí.
#[derive(Debug, Clone)]
pub struct Kontext {
    /// Stav lokální složky.
    pub lokalni_slozka: StavLokalniSlozky,
    /// Počet položek, které vrátil poslední výpis serveru.
    pub pocet_vzdalenych_polozek: u64,
    /// Počet souborů v posledním společném stavu.
    pub pocet_synchronizovanych: u64,
    /// Aktuální verze na serveru podle čerstvého výpisu, podle cesty.
    pub vzdalene_etagy: BTreeMap<Cesta, Etag>,
    /// Režim synchronizace.
    pub rezim: Rezim,
    /// Prahy brzdy.
    pub limity: LimityHromadneZmeny,
    /// Potvrzení hromadné změny od uživatele, pokud nějaké je.
    pub potvrzeni: Option<PotvrzeniHromadneZmeny>,
    /// Volné místo na iCloudu v bajtech, pokud je známé.
    pub volne_misto_icloud: Option<u64>,
    /// Volné místo na lokálním disku v bajtech, pokud je známé.
    pub volne_misto_disk: Option<u64>,
    /// Jak dlouho musí být soubor beze změny, než se nahraje.
    pub doba_ustaleni: Duration,
}

impl Kontext {
    /// Kontext pro složku a server se stejným počtem souborů, plný režim,
    /// výchozí limity, bez známého volného místa.
    pub fn new(pocet_synchronizovanych: u64) -> Self {
        Self {
            lokalni_slozka: StavLokalniSlozky::Dostupna {
                pocet_souboru: pocet_synchronizovanych,
            },
            pocet_vzdalenych_polozek: pocet_synchronizovanych,
            pocet_synchronizovanych,
            vzdalene_etagy: BTreeMap::new(),
            rezim: Rezim::Plny,
            limity: LimityHromadneZmeny::default(),
            potvrzeni: None,
            volne_misto_icloud: None,
            volne_misto_disk: None,
            doba_ustaleni: VYCHOZI_DOBA_USTALENI,
        }
    }
}
