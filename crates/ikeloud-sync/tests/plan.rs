//! Tabulkové testy plánovače: tři stavy → akce.

#![allow(clippy::expect_used, clippy::panic)] // testovací kód smí spadnout

use std::time::Duration;

use ikeloud_safety::{Akce, Cesta, Etag};
use ikeloud_sync::{
    Kolize, LokalniSoubor, OtiskObsahu, Plan, SpolecnyStav, Stavy, Strana, VzdalenySoubor,
    sestavit_plan,
};

// --- pomocníci --------------------------------------------------------------

fn c(s: &str) -> Cesta {
    Cesta::new(s)
}

fn lok(cesta: &str, otisk: &str) -> LokalniSoubor {
    LokalniSoubor {
        cesta: c(cesta),
        otisk: OtiskObsahu::new(otisk),
        velikost: 7,
        stabilni_po: Duration::from_secs(60),
    }
}

fn vzd(cesta: &str, etag: &str) -> VzdalenySoubor {
    VzdalenySoubor {
        cesta: c(cesta),
        etag: Etag::new(etag),
        velikost: 9,
    }
}

fn spol(cesta: &str, otisk: &str, etag: &str) -> SpolecnyStav {
    SpolecnyStav {
        cesta: c(cesta),
        otisk: OtiskObsahu::new(otisk),
        etag: Etag::new(etag),
    }
}

/// Název kopie podle vzoru ze zadání, s pevným datem.
fn kopie(cesta: &Cesta, n: u32) -> Cesta {
    let (zaklad, pripona) = match cesta.as_str().rsplit_once('.') {
        Some((z, p)) if !z.ends_with('/') && !z.is_empty() => (z, format!(".{p}")),
        _ => (cesta.as_str(), String::new()),
    };
    let cislo = if n == 1 {
        String::new()
    } else {
        format!(" {n}")
    };
    Cesta::new(format!(
        "{zaklad} (konflikt Linux 2026-11-15{cislo}){pripona}"
    ))
}

fn planuj(stavy: &Stavy) -> Plan {
    sestavit_plan(stavy, &kopie)
}

fn nahrat(cesta: &str, etag: Option<&str>) -> Akce {
    Akce::Nahrat {
        cesta: c(cesta),
        ocekavany_etag: etag.map(Etag::new),
        velikost: 7,
        stabilni_po: Duration::from_secs(60),
    }
}

fn stahnout(cesta: &str, prepise: bool) -> Akce {
    Akce::Stahnout {
        cesta: c(cesta),
        velikost: 9,
        prepise_lokalni: prepise,
    }
}

fn konflikt(cesta: &str, kopie: &str, stahnout_take: bool) -> Vec<Akce> {
    let mut a = vec![
        Akce::PresunoutLokalne {
            z: c(cesta),
            na: c(kopie),
        },
        Akce::NahratKonfliktniKopii {
            cesta: c(kopie),
            velikost: 7,
        },
    ];
    if stahnout_take {
        a.push(stahnout(cesta, false));
    }
    a
}

// --- rozhodovací tabulka ----------------------------------------------------

#[test]
fn rozhodovaci_tabulka() {
    // Společný stav: soubor „a“ s obsahem o1 a etagem e1.
    // l/r: None = soubor na té straně není; o1/e1 = beze změny; o2/e2 = změna.
    type Pripad = (
        &'static str,
        Option<&'static str>,
        Option<&'static str>,
        bool,
        Vec<Akce>,
    );
    let kopie_a = "a (konflikt Linux 2026-11-15)";
    let pripady: Vec<Pripad> = vec![
        ("nic se nezměnilo", Some("o1"), Some("e1"), true, vec![]),
        (
            "změna jen na Linuxu",
            Some("o2"),
            Some("e1"),
            true,
            vec![nahrat("a", Some("e1"))],
        ),
        (
            "změna jen na iCloudu",
            Some("o1"),
            Some("e2"),
            true,
            vec![stahnout("a", true)],
        ),
        (
            "změna na obou",
            Some("o2"),
            Some("e2"),
            true,
            konflikt("a", kopie_a, true),
        ),
        (
            "smazáno na Linuxu",
            None,
            Some("e1"),
            true,
            vec![Akce::PresunoutNaServeruDoKose {
                cesta: c("a"),
                ocekavany_etag: Etag::new("e1"),
            }],
        ),
        (
            "smazáno na iCloudu",
            Some("o1"),
            None,
            true,
            vec![Akce::PresunoutLokalneDoKose { cesta: c("a") }],
        ),
        ("smazáno na obou", None, None, true, vec![]),
        (
            "smazáno na Linuxu, změna na iCloudu",
            None,
            Some("e2"),
            true,
            vec![stahnout("a", false)],
        ),
        (
            "změna na Linuxu, smazáno na iCloudu",
            Some("o2"),
            None,
            true,
            konflikt("a", kopie_a, false),
        ),
        // Bez společného stavu (soubor je nový).
        (
            "nový jen na Linuxu",
            Some("o1"),
            None,
            false,
            vec![nahrat("a", None)],
        ),
        (
            "nový jen na iCloudu",
            None,
            Some("e1"),
            false,
            vec![stahnout("a", false)],
        ),
        (
            "nový na obou",
            Some("o1"),
            Some("e1"),
            false,
            konflikt("a", kopie_a, true),
        ),
    ];
    for (popis, l, r, se_spolecnym, ocekavano) in pripady {
        let stavy = Stavy {
            lokalni: l.map(|o| lok("a", o)).into_iter().collect(),
            vzdalene: r.map(|e| vzd("a", e)).into_iter().collect(),
            spolecne: if se_spolecnym {
                vec![spol("a", "o1", "e1")]
            } else {
                vec![]
            },
            ..Stavy::default()
        };
        assert_eq!(planuj(&stavy).akce, ocekavano, "{popis}");
    }
}

#[test]
fn smazano_na_obou_se_zapomene() {
    let stavy = Stavy {
        spolecne: vec![spol("a", "o1", "e1")],
        ..Stavy::default()
    };
    let p = planuj(&stavy);
    assert_eq!(p.akce, vec![]);
    assert_eq!(p.zapomenout, vec![c("a")]);
}

#[test]
fn cas_zmeny_nerozhoduje() {
    // Jiná „stabilita“ (čas) při stejném otisku není změna.
    let mut l = lok("a", "o1");
    l.stabilni_po = Duration::ZERO;
    let stavy = Stavy {
        lokalni: vec![l],
        vzdalene: vec![vzd("a", "e1")],
        spolecne: vec![spol("a", "o1", "e1")],
        ..Stavy::default()
    };
    assert_eq!(planuj(&stavy).akce, vec![]);
}

// --- názvy ------------------------------------------------------------------

#[test]
fn parovani_po_normalizaci() {
    let nfd = "Pr\u{30c}i\u{301}loha.pdf";
    let pripady: Vec<(&str, Stavy, Vec<Akce>)> = vec![
        (
            "NFD na Linuxu a NFC na iCloudu je tentýž soubor",
            Stavy {
                lokalni: vec![lok(nfd, "o1")],
                vzdalene: vec![vzd("Příloha.pdf", "e1")],
                spolecne: vec![spol("Příloha.pdf", "o1", "e1")],
                ..Stavy::default()
            },
            vec![],
        ),
        (
            "jiná velikost písmen je tentýž soubor; nahrává se pod názvem ze serveru",
            Stavy {
                lokalni: vec![lok("foto.JPG", "o2")],
                vzdalene: vec![vzd("Foto.jpg", "e1")],
                spolecne: vec![spol("Foto.jpg", "o1", "e1")],
                ..Stavy::default()
            },
            vec![nahrat("Foto.jpg", Some("e1"))],
        ),
        (
            "nový soubor z NFD se nahraje jako NFC",
            Stavy {
                lokalni: vec![lok(nfd, "o1")],
                ..Stavy::default()
            },
            vec![nahrat("Příloha.pdf", None)],
        ),
        (
            "stažení přes lokální soubor jde na lokální název",
            Stavy {
                lokalni: vec![lok("foto.jpg", "o1")],
                vzdalene: vec![vzd("Foto.jpg", "e2")],
                spolecne: vec![spol("Foto.jpg", "o1", "e1")],
                ..Stavy::default()
            },
            vec![stahnout("foto.jpg", true)],
        ),
    ];
    for (popis, stavy, ocekavano) in pripady {
        assert_eq!(planuj(&stavy).akce, ocekavano, "{popis}");
    }
}

#[test]
fn kolize_na_linuxu_se_vynechaji() {
    let stavy = Stavy {
        lokalni: vec![lok("a.txt", "o1"), lok("A.txt", "o2"), lok("b.txt", "o3")],
        vzdalene: vec![vzd("a.txt", "e1")],
        spolecne: vec![spol("a.txt", "o1", "e1")],
        ..Stavy::default()
    };
    let p = planuj(&stavy);
    assert_eq!(
        p.akce,
        vec![nahrat("b.txt", None)],
        "kolidující soubory se nesynchronizují"
    );
    assert_eq!(
        p.kolize,
        vec![Kolize {
            strana: Strana::Lokalni,
            cesty: vec![c("A.txt"), c("a.txt")]
        }]
    );
}

#[test]
fn kolize_na_serveru_se_vynechaji() {
    let stavy = Stavy {
        vzdalene: vec![vzd("x", "e1"), vzd("X", "e2")],
        ..Stavy::default()
    };
    let p = planuj(&stavy);
    assert_eq!(p.akce, vec![]);
    assert_eq!(
        p.kolize,
        vec![Kolize {
            strana: Strana::Vzdalena,
            cesty: vec![c("X"), c("x")]
        }]
    );
}

#[test]
fn ignorovane_soubory_se_neplanuji() {
    let stavy = Stavy {
        lokalni: vec![
            lok(".~lock.zprava.odt#", "o"),
            lok("a.swp", "o"),
            lok("x.part", "o"),
        ],
        vzdalene: vec![vzd("fotky/.DS_Store", "e")],
        ..Stavy::default()
    };
    assert_eq!(planuj(&stavy), Plan::default());
}

// --- konfliktní kopie -------------------------------------------------------

#[test]
fn konfliktni_kopie_dostane_volny_nazev() {
    let stavy = Stavy {
        lokalni: vec![
            lok("zprava.odt", "o2"),
            lok("zprava (konflikt Linux 2026-11-15).odt", "x"),
        ],
        vzdalene: vec![
            vzd("zprava.odt", "e2"),
            vzd("zprava (konflikt Linux 2026-11-15).odt", "y"),
        ],
        spolecne: vec![
            spol("zprava.odt", "o1", "e1"),
            spol("zprava (konflikt Linux 2026-11-15).odt", "x", "y"),
        ],
        ..Stavy::default()
    };
    assert_eq!(
        planuj(&stavy).akce,
        konflikt(
            "zprava.odt",
            "zprava (konflikt Linux 2026-11-15 2).odt",
            true
        )
    );
}

#[test]
fn dve_kopie_ve_stejnem_behu_se_nesrazi() {
    // „A“ a „a“ v různých složkách, ale kopie mají jiné názvy.
    let stavy = Stavy {
        lokalni: vec![lok("d/a.txt", "o2"), lok("e/a.txt", "o2")],
        vzdalene: vec![vzd("d/a.txt", "e2"), vzd("e/a.txt", "e2")],
        spolecne: vec![spol("d/a.txt", "o1", "e1"), spol("e/a.txt", "o1", "e1")],
        ..Stavy::default()
    };
    let p = planuj(&stavy);
    let kopie: Vec<&Cesta> = p
        .akce
        .iter()
        .filter_map(|a| match a {
            Akce::NahratKonfliktniKopii { cesta, .. } => Some(cesta),
            _ => None,
        })
        .collect();
    assert_eq!(
        kopie,
        vec![
            &c("d/a (konflikt Linux 2026-11-15).txt"),
            &c("e/a (konflikt Linux 2026-11-15).txt")
        ]
    );
}

// --- složky -----------------------------------------------------------------

#[test]
fn chybejici_slozky_se_vytvori_predem() {
    let slozka = |s: &str| Akce::VytvoritSlozkuNaServeru { cesta: c(s) };
    let pripady: Vec<(&str, Stavy, Vec<Akce>)> = vec![
        (
            "nový soubor v nové hluboké složce",
            Stavy {
                lokalni: vec![lok("a/b/c.txt", "o")],
                ..Stavy::default()
            },
            vec![slozka("a"), slozka("a/b"), nahrat("a/b/c.txt", None)],
        ),
        (
            "složka už na serveru je (jako složka)",
            Stavy {
                lokalni: vec![lok("a/b/c.txt", "o")],
                vzdalene_slozky: vec![c("a"), c("A/B")],
                ..Stavy::default()
            },
            vec![nahrat("a/b/c.txt", None)],
        ),
        (
            "složka na serveru je vidět ze souboru v ní",
            Stavy {
                lokalni: vec![lok("a/nove.txt", "o"), lok("a/stare.txt", "x")],
                vzdalene: vec![vzd("a/stare.txt", "e")],
                spolecne: vec![spol("a/stare.txt", "x", "e")],
                ..Stavy::default()
            },
            vec![nahrat("a/nove.txt", None)],
        ),
        (
            "dva soubory ve stejné nové složce, složka jednou",
            Stavy {
                lokalni: vec![lok("s/1", "o"), lok("s/2", "o")],
                ..Stavy::default()
            },
            vec![slozka("s"), nahrat("s/1", None), nahrat("s/2", None)],
        ),
        (
            "přepsání existujícího souboru složku nevytváří",
            Stavy {
                lokalni: vec![lok("a/b.txt", "o2")],
                vzdalene: vec![vzd("a/b.txt", "e1")],
                spolecne: vec![spol("a/b.txt", "o1", "e1")],
                ..Stavy::default()
            },
            vec![nahrat("a/b.txt", Some("e1"))],
        ),
    ];
    for (popis, stavy, ocekavano) in pripady {
        assert_eq!(planuj(&stavy).akce, ocekavano, "{popis}");
    }
}

// --- celý plán prochází bezpečnostní vrstvou -------------------------------

#[test]
fn plan_po_zmene_na_linuxu_schvali_safety() {
    use ikeloud_safety::{Kontext, Rozhodnuti, posoudit};

    let stavy = Stavy {
        lokalni: vec![lok("a", "o2"), lok("b", "o1")],
        vzdalene: vec![vzd("a", "e1"), vzd("b", "e1")],
        spolecne: vec![spol("a", "o1", "e1"), spol("b", "o1", "e1")],
        ..Stavy::default()
    };
    let p = planuj(&stavy);
    let mut k = Kontext::new(2);
    for v in &stavy.vzdalene {
        k.vzdalene_etagy.insert(v.cesta.clone(), v.etag.clone());
    }
    let Ok(Rozhodnuti::Schvaleno(s)) = posoudit(p.akce, &k) else {
        panic!("plán měl projít");
    };
    assert_eq!(s.akce.len(), 1);
    assert!(s.odlozene.is_empty());
}
