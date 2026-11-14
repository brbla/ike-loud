//! Posouzení plánu akcí: pustit, odložit, nebo zastavit a zeptat se.

use std::collections::{BTreeMap, BTreeSet};

use crate::akce::{Akce, Cesta, Etag};
use crate::kontext::{DuvodJenCteni, Kontext, Rezim, StavLokalniSlozky};
use crate::nazvy::{self, ChybaNazvu};

/// Akce, kterou bezpečnostní vrstva schválila.
///
/// Vytvořit ji umí jen tento crate a nejde zkopírovat. Zápisové funkce
/// `ikeloud-api` ji berou hodnotou, takže každé schválení platí na jeden zápis.
#[derive(Debug, PartialEq, Eq)]
pub struct SchvalenaAkce {
    akce: Akce,
}

impl SchvalenaAkce {
    /// Schválená akce.
    pub fn akce(&self) -> &Akce {
        &self.akce
    }
}

/// Proč se akce teď neprovede. Engine ji zkusí znovu v dalším běhu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DuvodOdlozeni {
    /// Aplikace je v režimu jen pro čtení.
    JenCteni(DuvodJenCteni),
    /// Soubor se ještě zapisuje (nezměněný kratší dobu než doba ustálení).
    SouborSeZapisuje,
    /// Název nejde nahrát na server.
    NepovolenyNazev(ChybaNazvu),
    /// Dočasný nebo zamykací soubor; nesynchronizuje se.
    IgnorovanySoubor,
    /// Na serveru už je položka se stejným názvem, lišícím se jen velikostí písmen.
    KolizeVelikostiPismen {
        /// Kolidující cesta.
        s: Cesta,
    },
}

/// Odložená akce a důvod.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OdlozenaAkce {
    /// Akce z plánu.
    pub akce: Akce,
    /// Proč se neprovede.
    pub duvod: DuvodOdlozeni,
}

/// Plán po schválení.
#[derive(Debug, PartialEq, Eq)]
pub struct SchvalenyPlan {
    /// Akce, které se smí provést.
    pub akce: Vec<SchvalenaAkce>,
    /// Akce, které se teď neprovedou.
    pub odlozene: Vec<OdlozenaAkce>,
}

/// Výsledek posouzení, když se smí pokračovat.
#[derive(Debug, PartialEq, Eq)]
pub enum Rozhodnuti {
    /// Plán je prázdný.
    NicKProvedeni,
    /// Plán je schválený (případně s odloženými akcemi).
    Schvaleno(SchvalenyPlan),
}

/// Přehled hromadné změny pro dotaz na uživatele.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SouhrnHromadneZmeny {
    /// Kolik souborů by se smazalo nebo přepsalo.
    pub pocet: u64,
    /// Kolik souborů je synchronizováno.
    pub celkem: u64,
    otisk: u64,
}

impl SouhrnHromadneZmeny {
    /// Uživatel změnu potvrdil. Potvrzení platí jen pro přesně tentýž plán.
    pub fn potvrdit(&self) -> PotvrzeniHromadneZmeny {
        PotvrzeniHromadneZmeny { otisk: self.otisk }
    }
}

/// Potvrzení hromadné změny, vázané na konkrétní plán.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PotvrzeniHromadneZmeny {
    otisk: u64,
}

/// Proč se plán neprovede vůbec. Synchronizace stojí.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Zamitnuti {
    /// Lokální složka chybí nebo je najednou prázdná.
    LokalniSlozkaNedostupna,
    /// Server vrátil prázdný výpis, i když má být co synchronizovat.
    PodezrelyVypisServeru,
    /// Položky se na serveru změnily od chvíle, kdy engine sestavil plán.
    Konflikt(Vec<Cesta>),
    /// Plán by smazal nebo přepsal moc souborů; čeká na potvrzení.
    HromadnaZmenaCekaNaPotvrzeni(SouhrnHromadneZmeny),
    /// Na iCloudu není dost místa.
    MaloMistaNaICloudu {
        /// Kolik bajtů je potřeba.
        potreba: u64,
        /// Kolik bajtů je volných.
        volno: u64,
    },
    /// Na disku není dost místa.
    MaloMistaNaDisku {
        /// Kolik bajtů je potřeba.
        potreba: u64,
        /// Kolik bajtů je volných.
        volno: u64,
    },
}

/// Posoudí plán akcí.
///
/// Pořadí kontrol: okolí (lokální složka, výpis serveru), odložení
/// jednotlivých akcí, konflikty verzí, brzda hromadných změn, volné místo.
/// `kontext.vzdalene_etagy` musí obsahovat čerstvý stav všech cest z plánu
/// a všech položek ve složkách, kam plán přidává nové názvy.
pub fn posoudit(plan: Vec<Akce>, kontext: &Kontext) -> Result<Rozhodnuti, Zamitnuti> {
    over_okoli(kontext)?;
    if plan.is_empty() {
        return Ok(Rozhodnuti::NicKProvedeni);
    }

    let mut k_provedeni = Vec::new();
    let mut odlozene = Vec::new();
    let kolize = kolize_velikosti_pismen(&plan, &kontext.vzdalene_etagy);
    for akce in plan {
        match duvod_odlozeni(&akce, kontext, &kolize) {
            Some(duvod) => odlozene.push(OdlozenaAkce { akce, duvod }),
            None => k_provedeni.push(akce),
        }
    }

    over_konflikty(&k_provedeni, &kontext.vzdalene_etagy)?;
    over_hromadnou_zmenu(&k_provedeni, kontext)?;
    over_misto(&k_provedeni, kontext)?;

    Ok(Rozhodnuti::Schvaleno(SchvalenyPlan {
        akce: k_provedeni
            .into_iter()
            .map(|akce| SchvalenaAkce { akce })
            .collect(),
        odlozene,
    }))
}

fn over_okoli(kontext: &Kontext) -> Result<(), Zamitnuti> {
    let ma_byt_obsah = kontext.pocet_synchronizovanych > 0;
    match kontext.lokalni_slozka {
        StavLokalniSlozky::Chybi => return Err(Zamitnuti::LokalniSlozkaNedostupna),
        StavLokalniSlozky::Dostupna { pocet_souboru: 0 } if ma_byt_obsah => {
            return Err(Zamitnuti::LokalniSlozkaNedostupna);
        }
        StavLokalniSlozky::Dostupna { .. } => {}
    }
    if ma_byt_obsah && kontext.pocet_vzdalenych_polozek == 0 {
        return Err(Zamitnuti::PodezrelyVypisServeru);
    }
    Ok(())
}

fn duvod_odlozeni(
    akce: &Akce,
    kontext: &Kontext,
    kolize: &BTreeMap<Cesta, Cesta>,
) -> Option<DuvodOdlozeni> {
    if nazvy::je_ignorovany(akce.cesta().nazev()) {
        return Some(DuvodOdlozeni::IgnorovanySoubor);
    }
    if !akce.zapisuje_na_server() {
        return None;
    }
    if let Rezim::JenCteni(duvod) = kontext.rezim {
        return Some(DuvodOdlozeni::JenCteni(duvod));
    }
    if let Err(chyba) = nazvy::over_cestu(akce.cesta().as_str()) {
        return Some(DuvodOdlozeni::NepovolenyNazev(chyba));
    }
    if let Some(s) = kolize.get(akce.cesta()) {
        return Some(DuvodOdlozeni::KolizeVelikostiPismen { s: s.clone() });
    }
    if let Akce::Nahrat { stabilni_po, .. } = akce
        && *stabilni_po < kontext.doba_ustaleni
    {
        return Some(DuvodOdlozeni::SouborSeZapisuje);
    }
    None
}

/// Nové názvy na serveru, které se od existujících nebo od jiných nových
/// liší jen velikostí písmen. Vrací novou cestu a cestu, se kterou koliduje.
fn kolize_velikosti_pismen(
    plan: &[Akce],
    vzdalene: &BTreeMap<Cesta, Etag>,
) -> BTreeMap<Cesta, Cesta> {
    let mut obsazene: BTreeMap<String, Cesta> = vzdalene
        .keys()
        .map(|c| (nazvy::klic_bez_velikosti(c.as_str()), c.clone()))
        .collect();
    let mut kolize = BTreeMap::new();
    for akce in plan {
        let Some(nova) = nova_cesta_na_serveru(akce) else {
            continue;
        };
        let klic = nazvy::klic_bez_velikosti(nova.as_str());
        match obsazene.get(&klic) {
            Some(existujici) if existujici != nova => {
                kolize.insert(nova.clone(), existujici.clone());
            }
            Some(_) => {}
            None => {
                obsazene.insert(klic, nova.clone());
            }
        }
    }
    kolize
}

fn nova_cesta_na_serveru(akce: &Akce) -> Option<&Cesta> {
    match akce {
        Akce::Nahrat {
            cesta,
            ocekavany_etag: None,
            ..
        }
        | Akce::NahratKonfliktniKopii { cesta, .. }
        | Akce::VytvoritSlozkuNaServeru { cesta } => Some(cesta),
        Akce::PresunoutNaServeru { na, .. } => Some(na),
        _ => None,
    }
}

fn over_konflikty(akce: &[Akce], vzdalene: &BTreeMap<Cesta, Etag>) -> Result<(), Zamitnuti> {
    let mut konflikty = BTreeSet::new();
    for a in akce {
        let sedi = match a {
            Akce::Nahrat {
                cesta,
                ocekavany_etag,
                ..
            } => vzdalene.get(cesta) == ocekavany_etag.as_ref(),
            Akce::NahratKonfliktniKopii { cesta, .. } | Akce::VytvoritSlozkuNaServeru { cesta } => {
                !vzdalene.contains_key(cesta)
            }
            Akce::PresunoutNaServeruDoKose {
                cesta,
                ocekavany_etag,
            } => vzdalene.get(cesta) == Some(ocekavany_etag),
            Akce::PresunoutNaServeru {
                z,
                na,
                ocekavany_etag,
            } => vzdalene.get(z) == Some(ocekavany_etag) && !vzdalene.contains_key(na),
            Akce::Stahnout { .. }
            | Akce::PresunoutLokalneDoKose { .. }
            | Akce::PresunoutLokalne { .. } => true,
        };
        if !sedi {
            konflikty.insert(a.cesta().clone());
        }
    }
    if konflikty.is_empty() {
        Ok(())
    } else {
        Err(Zamitnuti::Konflikt(konflikty.into_iter().collect()))
    }
}

fn over_hromadnou_zmenu(akce: &[Akce], kontext: &Kontext) -> Result<(), Zamitnuti> {
    let pocet = akce.iter().filter(|a| a.maze_nebo_prepisuje()).count() as u64;
    let limity = kontext.limity;
    let celkem = kontext.pocet_synchronizovanych;
    let nad_absolutnim = pocet > limity.absolutni;
    let nad_podilem = pocet > limity.minimalni
        && u128::from(pocet) * 100 > u128::from(limity.procent) * u128::from(celkem);
    if !(nad_absolutnim || nad_podilem) {
        return Ok(());
    }
    let otisk = otisk_planu(akce);
    if kontext.potvrzeni.as_ref().is_some_and(|p| p.otisk == otisk) {
        return Ok(());
    }
    Err(Zamitnuti::HromadnaZmenaCekaNaPotvrzeni(
        SouhrnHromadneZmeny {
            pocet,
            celkem,
            otisk,
        },
    ))
}

fn over_misto(akce: &[Akce], kontext: &Kontext) -> Result<(), Zamitnuti> {
    let na_server = akce
        .iter()
        .map(Akce::bajtu_na_server)
        .fold(0u64, u64::saturating_add);
    if let Some(volno) = kontext.volne_misto_icloud
        && na_server > volno
    {
        return Err(Zamitnuti::MaloMistaNaICloudu {
            potreba: na_server,
            volno,
        });
    }
    let na_disk = akce
        .iter()
        .map(Akce::bajtu_na_disk)
        .fold(0u64, u64::saturating_add);
    if let Some(volno) = kontext.volne_misto_disk
        && na_disk > volno
    {
        return Err(Zamitnuti::MaloMistaNaDisku {
            potreba: na_disk,
            volno,
        });
    }
    Ok(())
}

/// Otisk plánu (FNV-1a 64 nad textovou podobou akcí). Slouží jen k tomu, aby
/// potvrzení platilo pro přesně ten plán, který uživatel viděl.
fn otisk_planu(akce: &[Akce]) -> u64 {
    const ZACATEK: u64 = 0xcbf2_9ce4_8422_2325;
    const PRVOCISLO: u64 = 0x0000_0100_0000_01b3;
    let text = format!("{akce:?}");
    text.bytes()
        .fold(ZACATEK, |h, b| (h ^ u64::from(b)).wrapping_mul(PRVOCISLO))
}
