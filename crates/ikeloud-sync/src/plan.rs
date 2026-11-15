//! Sestavení plánu akcí ze tří stavů.
//!
//! Rozhodovací tabulka (zadání, kapitola Základní principy; TLA+ `Plan`):
//!
//! | Změna od poslední synchronizace | Akce                                        |
//! |---------------------------------|---------------------------------------------|
//! | jen na Linuxu                   | nahrát (smazání → koš na iCloudu)           |
//! | jen na iCloudu                  | stáhnout (smazání → koš systému)            |
//! | na obou                         | iCloud vyhrává, lokální → konfliktní kopie  |
//! | nikde                           | nic                                         |
//!
//! Změnu na Linuxu pozná podle otisku obsahu, změnu na iCloudu podle etagu,
//! nikdy podle času. Soubory se párují podle klíče z [`crate::nazvy::klic`].

use std::collections::{BTreeMap, BTreeSet};

use ikeloud_safety::{Akce, Cesta, nazvy as safety_nazvy};

use crate::nazvy::{klic, nfc};
use crate::stav::{LokalniSoubor, SpolecnyStav, Stavy, VzdalenySoubor};

/// Na které straně kolize vznikla.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strana {
    /// Na Linuxu (rozlišuje velikost písmen, iCloud ne).
    Lokalni,
    /// Na iCloudu (nemělo by nastat; bere se jako podezřelý výpis).
    Vzdalena,
}

/// Soubory, které mají po normalizaci stejný název. Synchronizace je
/// vynechá, dokud je uživatel nepřejmenuje.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Kolize {
    /// Strana, kde soubory jsou.
    pub strana: Strana,
    /// Kolidující cesty, seřazené.
    pub cesty: Vec<Cesta>,
}

/// Výsledek plánovače.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    /// Akce v pořadí, ve kterém se mají provést.
    pub akce: Vec<Akce>,
    /// Položky společného stavu, které už nejsou na žádné straně.
    pub zapomenout: Vec<Cesta>,
    /// Kolize názvů, které synchronizace vynechala.
    pub kolize: Vec<Kolize>,
}

/// Sestaví plán ze tří stavů.
///
/// `nazev_kopie(cesta, n)` vrátí název konfliktní kopie souboru `cesta`;
/// `n` je pořadí pokusu od 1 a zvyšuje se, dokud název není volný. Název
/// dodává volající, protože ho uživatel vidí a je přeložený.
pub fn sestavit_plan(stavy: &Stavy, nazev_kopie: &dyn Fn(&Cesta, u32) -> Cesta) -> Plan {
    let mut plan = Plan::default();
    let lokalni = indexuj(
        &stavy.lokalni,
        |s| &s.cesta,
        Strana::Lokalni,
        &mut plan.kolize,
    );
    let vzdalene = indexuj(
        &stavy.vzdalene,
        |s| &s.cesta,
        Strana::Vzdalena,
        &mut plan.kolize,
    );
    let spolecne: BTreeMap<String, &SpolecnyStav> = stavy
        .spolecne
        .iter()
        .map(|s| (klic(s.cesta.as_str()), s))
        .collect();
    let blokovane: BTreeSet<String> = plan
        .kolize
        .iter()
        .flat_map(|k| k.cesty.iter().map(|c| klic(c.as_str())))
        .collect();

    let mut obsazene: BTreeSet<String> = lokalni.keys().chain(vzdalene.keys()).cloned().collect();
    obsazene.extend(blokovane.iter().cloned());

    let klice: BTreeSet<&String> = lokalni
        .keys()
        .chain(vzdalene.keys())
        .chain(spolecne.keys())
        .collect();
    let mut akce = Vec::new();
    for k in klice {
        if blokovane.contains(k) {
            continue;
        }
        let l = lokalni.get(k).copied();
        let r = vzdalene.get(k).copied();
        let b = spolecne.get(k).copied();
        rozhodni(l, r, b, &mut akce, &mut plan.zapomenout, &mut |cesta| {
            volny_nazev_kopie(cesta, nazev_kopie, &mut obsazene)
        });
    }

    plan.akce = chybejici_slozky(&akce, stavy);
    plan.akce.extend(akce);
    plan
}

/// Rozhodovací tabulka pro jeden soubor.
fn rozhodni(
    l: Option<&LokalniSoubor>,
    r: Option<&VzdalenySoubor>,
    b: Option<&SpolecnyStav>,
    akce: &mut Vec<Akce>,
    zapomenout: &mut Vec<Cesta>,
    kopie: &mut dyn FnMut(&Cesta) -> Cesta,
) {
    let zmena_lokalne = match (l, b) {
        (None, None) => false,
        (Some(l), Some(b)) => l.otisk != b.otisk,
        _ => true,
    };
    let zmena_vzdalene = match (r, b) {
        (None, None) => false,
        (Some(r), Some(b)) => r.etag != b.etag,
        _ => true,
    };

    match (zmena_lokalne, zmena_vzdalene, l, r) {
        (false, false, _, _) => {}
        // Smazáno na obou stranách: jen zapomenout společný stav.
        (_, _, None, None) => {
            if let Some(b) = b {
                zapomenout.push(b.cesta.clone());
            }
        }
        // Jen na Linuxu.
        (true, false, Some(l), r) => akce.push(Akce::Nahrat {
            cesta: r.map_or_else(|| Cesta::new(nfc(l.cesta.as_str())), |r| r.cesta.clone()),
            ocekavany_etag: r.map(|r| r.etag.clone()),
            velikost: l.velikost,
            stabilni_po: l.stabilni_po,
        }),
        (true, false, None, Some(r)) => akce.push(Akce::PresunoutNaServeruDoKose {
            cesta: r.cesta.clone(),
            ocekavany_etag: r.etag.clone(),
        }),
        // Jen na iCloudu.
        (false, true, l, Some(r)) => akce.push(Akce::Stahnout {
            cesta: l.map_or_else(|| r.cesta.clone(), |l| l.cesta.clone()),
            velikost: r.velikost,
            prepise_lokalni: l.is_some(),
        }),
        (false, true, Some(l), None) => {
            akce.push(Akce::PresunoutLokalneDoKose {
                cesta: l.cesta.clone(),
            });
        }
        // Na obou: iCloud vyhrává.
        (true, true, None, Some(r)) => akce.push(Akce::Stahnout {
            cesta: r.cesta.clone(),
            velikost: r.velikost,
            prepise_lokalni: false,
        }),
        (true, true, Some(l), r) => {
            let nazev = kopie(&l.cesta);
            akce.push(Akce::PresunoutLokalne {
                z: l.cesta.clone(),
                na: nazev.clone(),
            });
            akce.push(Akce::NahratKonfliktniKopii {
                cesta: nazev,
                velikost: l.velikost,
            });
            if let Some(r) = r {
                akce.push(Akce::Stahnout {
                    cesta: r.cesta.clone(),
                    velikost: r.velikost,
                    prepise_lokalni: false,
                });
            }
        }
    }
}

/// Seřadí soubory podle klíče; soubory se stejným klíčem ohlásí jako kolizi.
fn indexuj<'a, T>(
    soubory: &'a [T],
    cesta: impl Fn(&T) -> &Cesta,
    strana: Strana,
    kolize: &mut Vec<Kolize>,
) -> BTreeMap<String, &'a T> {
    let mut skupiny: BTreeMap<String, Vec<&'a T>> = BTreeMap::new();
    for s in soubory {
        if safety_nazvy::je_ignorovany(cesta(s).nazev()) {
            continue;
        }
        skupiny.entry(klic(cesta(s).as_str())).or_default().push(s);
    }
    let mut index = BTreeMap::new();
    for (k, skupina) in skupiny {
        match skupina.as_slice() {
            [jediny] => {
                index.insert(k, *jediny);
            }
            vice => {
                let mut cesty: Vec<Cesta> = vice.iter().map(|s| cesta(s).clone()).collect();
                cesty.sort();
                kolize.push(Kolize { strana, cesty });
            }
        }
    }
    index
}

fn volny_nazev_kopie(
    cesta: &Cesta,
    nazev_kopie: &dyn Fn(&Cesta, u32) -> Cesta,
    obsazene: &mut BTreeSet<String>,
) -> Cesta {
    let mut n = 1;
    loop {
        let kandidat = nazev_kopie(cesta, n);
        if obsazene.insert(klic(kandidat.as_str())) || n == u32::MAX {
            return kandidat;
        }
        n += 1;
    }
}

/// Složky na serveru, které je potřeba vytvořit před nahráním nových souborů,
/// mělčí dřív než hlubší.
fn chybejici_slozky(akce: &[Akce], stavy: &Stavy) -> Vec<Akce> {
    let mut existujici: BTreeSet<String> = stavy
        .vzdalene_slozky
        .iter()
        .map(|c| klic(c.as_str()))
        .collect();
    for s in &stavy.vzdalene {
        existujici.extend(predci(s.cesta.as_str()).map(klic));
    }
    let mut nove: Vec<(usize, Cesta)> = Vec::new();
    for a in akce {
        let nova = match a {
            Akce::Nahrat {
                cesta,
                ocekavany_etag: None,
                ..
            }
            | Akce::NahratKonfliktniKopii { cesta, .. } => cesta,
            _ => continue,
        };
        for predek in predci(nova.as_str()) {
            if existujici.insert(klic(predek)) {
                nove.push((predek.matches('/').count(), Cesta::new(nfc(predek))));
            }
        }
    }
    nove.sort();
    nove.into_iter()
        .map(|(_, cesta)| Akce::VytvoritSlozkuNaServeru { cesta })
        .collect()
}

/// Nadřazené složky cesty, od kořene: `a/b/c.txt` → `a`, `a/b`.
fn predci(cesta: &str) -> impl Iterator<Item = &str> {
    cesta
        .match_indices('/')
        .filter_map(move |(i, _)| cesta.get(..i))
}
