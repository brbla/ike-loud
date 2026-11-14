//! Tabulkové testy posouzení plánu: vstup → očekávaný výstup.
//!
//! Prvních pět řádků je příklad ze zadání (kapitola Testování).

use std::time::Duration;

use ikeloud_safety::nazvy::ChybaNazvu;
use ikeloud_safety::{
    Akce, Cesta, DuvodJenCteni, DuvodOdlozeni, Etag, Kontext, Rezim, Rozhodnuti, StavLokalniSlozky,
    Zamitnuti, posoudit,
};

// --- pomocníci --------------------------------------------------------------

fn c(s: &str) -> Cesta {
    Cesta::new(s)
}

fn e(s: &str) -> Etag {
    Etag::new(s)
}

fn nahrat(cesta: &str, etag: Option<&str>) -> Akce {
    Akce::Nahrat {
        cesta: c(cesta),
        ocekavany_etag: etag.map(e),
        velikost: 10,
        stabilni_po: Duration::from_secs(60),
    }
}

fn nahrat_stabilni(cesta: &str, stabilni_po: Duration) -> Akce {
    Akce::Nahrat {
        cesta: c(cesta),
        ocekavany_etag: None,
        velikost: 10,
        stabilni_po,
    }
}

fn do_kose(cesta: &str, etag: &str) -> Akce {
    Akce::PresunoutNaServeruDoKose {
        cesta: c(cesta),
        ocekavany_etag: e(etag),
    }
}

fn stahnout(cesta: &str, prepise_lokalni: bool) -> Akce {
    Akce::Stahnout {
        cesta: c(cesta),
        velikost: 10,
        prepise_lokalni,
    }
}

/// Kontext se `n` synchronizovanými soubory `f0`…`f{n-1}` s etagem `v1`.
fn kontext(n: u64) -> Kontext {
    let mut k = Kontext::new(n);
    for i in 0..n {
        k.vzdalene_etagy.insert(c(&format!("f{i}")), e("v1"));
    }
    k
}

/// Smazání prvních `pocet` souborů z kontextu.
fn smazat(pocet: u64) -> Vec<Akce> {
    (0..pocet)
        .map(|i| do_kose(&format!("f{i}"), "v1"))
        .collect()
}

/// Zjednodušený výsledek, aby tabulky zůstaly čitelné.
#[derive(Debug, PartialEq, Eq)]
enum Vysledek {
    NicKProvedeni,
    Schvaleno {
        provest: usize,
        odlozit: Vec<DuvodOdlozeni>,
    },
    Zamitnuto(Zamitnuti),
}

fn posud(plan: Vec<Akce>, k: &Kontext) -> Vysledek {
    match posoudit(plan, k) {
        Ok(Rozhodnuti::NicKProvedeni) => Vysledek::NicKProvedeni,
        Ok(Rozhodnuti::Schvaleno(p)) => Vysledek::Schvaleno {
            provest: p.akce.len(),
            odlozit: p.odlozene.into_iter().map(|o| o.duvod).collect(),
        },
        Err(z) => Vysledek::Zamitnuto(z),
    }
}

fn ok(provest: usize) -> Vysledek {
    Vysledek::Schvaleno {
        provest,
        odlozit: vec![],
    }
}

fn odlozit(provest: usize, duvody: Vec<DuvodOdlozeni>) -> Vysledek {
    Vysledek::Schvaleno {
        provest,
        odlozit: duvody,
    }
}

fn hromadna(vysledek: &Vysledek) -> Option<(u64, u64)> {
    match vysledek {
        Vysledek::Zamitnuto(Zamitnuti::HromadnaZmenaCekaNaPotvrzeni(s)) => {
            Some((s.pocet, s.celkem))
        }
        _ => None,
    }
}

// --- příklad ze zadání ------------------------------------------------------

#[test]
fn priklad_ze_zadani() {
    let mut chybi = kontext(200);
    chybi.lokalni_slozka = StavLokalniSlozky::Chybi;

    let pripady: Vec<(&str, Vec<Akce>, Kontext, Vysledek)> = vec![
        (
            "nahrát 1 soubor, etag na serveru sedí",
            vec![nahrat("f0", Some("v1"))],
            kontext(1),
            ok(1),
        ),
        (
            "nahrát 1 soubor, etag na serveru se změnil",
            vec![nahrat("f0", Some("v0"))],
            kontext(1),
            Vysledek::Zamitnuto(Zamitnuti::Konflikt(vec![c("f0")])),
        ),
        (
            "smazat vše, lokální složka chybí",
            smazat(200),
            chybi,
            Vysledek::Zamitnuto(Zamitnuti::LokalniSlozkaNedostupna),
        ),
        ("prázdný plán", vec![], kontext(5), Vysledek::NicKProvedeni),
    ];
    for (popis, plan, k, ocekavano) in pripady {
        assert_eq!(posud(plan, &k), ocekavano, "{popis}");
    }

    // „smazat 120 z 200“: souhrn nese otisk, proto se porovnává zvlášť.
    let v = posud(smazat(120), &kontext(200));
    assert_eq!(hromadna(&v), Some((120, 200)), "smazat 120 z 200");
}

// --- okolí ------------------------------------------------------------------

#[test]
fn okoli() {
    let prazdna = |synchronizovano: u64| {
        let mut k = kontext(synchronizovano);
        k.lokalni_slozka = StavLokalniSlozky::Dostupna { pocet_souboru: 0 };
        k
    };
    let prazdny_vypis = |synchronizovano: u64| {
        let mut k = kontext(synchronizovano);
        k.pocet_vzdalenych_polozek = 0;
        k
    };
    let mut chybi_prazdny_plan = kontext(3);
    chybi_prazdny_plan.lokalni_slozka = StavLokalniSlozky::Chybi;

    let pripady: Vec<(&str, Vec<Akce>, Kontext, Vysledek)> = vec![
        (
            "lokální složka najednou prázdná",
            smazat(1),
            prazdna(3),
            Vysledek::Zamitnuto(Zamitnuti::LokalniSlozkaNedostupna),
        ),
        (
            "prázdná složka při prvním nastavení je v pořádku",
            vec![stahnout("a", false)],
            prazdna(0),
            ok(1),
        ),
        (
            "chybějící složka zastaví i prázdný plán",
            vec![],
            chybi_prazdny_plan,
            Vysledek::Zamitnuto(Zamitnuti::LokalniSlozkaNedostupna),
        ),
        (
            "server vrátil prázdný výpis",
            vec![Akce::PresunoutLokalneDoKose { cesta: c("f0") }],
            prazdny_vypis(3),
            Vysledek::Zamitnuto(Zamitnuti::PodezrelyVypisServeru),
        ),
        (
            "prázdný výpis u prázdného účtu je v pořádku",
            vec![nahrat("a", None)],
            prazdny_vypis(0),
            ok(1),
        ),
    ];
    for (popis, plan, k, ocekavano) in pripady {
        assert_eq!(posud(plan, &k), ocekavano, "{popis}");
    }
}

// --- konflikty verzí --------------------------------------------------------

#[test]
fn konflikty() {
    let presun = |z: &str, na: &str, etag: &str| Akce::PresunoutNaServeru {
        z: c(z),
        na: c(na),
        ocekavany_etag: e(etag),
    };
    let konflikt = |cesty: &[&str]| {
        Vysledek::Zamitnuto(Zamitnuti::Konflikt(cesty.iter().map(|s| c(s)).collect()))
    };

    let pripady: Vec<(&str, Vec<Akce>, Vysledek)> = vec![
        (
            "nový soubor na volné místo",
            vec![nahrat("novy", None)],
            ok(1),
        ),
        (
            "nový soubor, na serveru už je",
            vec![nahrat("f0", None)],
            konflikt(&["f0"]),
        ),
        (
            "přepsání, na serveru mezitím smazán",
            vec![nahrat("pryc", Some("v1"))],
            konflikt(&["pryc"]),
        ),
        ("koš, etag sedí", vec![do_kose("f0", "v1")], ok(1)),
        (
            "koš, etag se změnil",
            vec![do_kose("f0", "v2")],
            konflikt(&["f0"]),
        ),
        (
            "koš, položka už na serveru není",
            vec![do_kose("pryc", "v1")],
            konflikt(&["pryc"]),
        ),
        (
            "přesun, etag sedí a cíl je volný",
            vec![presun("f0", "g", "v1")],
            ok(1),
        ),
        (
            "přesun na obsazený cíl",
            vec![presun("f0", "f1", "v1")],
            konflikt(&["f1"]),
        ),
        (
            "přesun, etag se změnil",
            vec![presun("f0", "g", "v9")],
            konflikt(&["g"]),
        ),
        (
            "konfliktní kopie na obsazený název",
            vec![Akce::NahratKonfliktniKopii {
                cesta: c("f0"),
                velikost: 1,
            }],
            konflikt(&["f0"]),
        ),
        (
            "složka, která už existuje",
            vec![Akce::VytvoritSlozkuNaServeru { cesta: c("f0") }],
            konflikt(&["f0"]),
        ),
        (
            "víc konfliktů najednou, seřazené",
            vec![
                do_kose("f2", "x"),
                nahrat("f1", Some("x")),
                nahrat("ok", None),
            ],
            konflikt(&["f1", "f2"]),
        ),
        (
            "stažení konflikt nehlídá",
            vec![stahnout("f0", true)],
            ok(1),
        ),
    ];
    for (popis, plan, ocekavano) in pripady {
        assert_eq!(posud(plan, &kontext(3)), ocekavano, "{popis}");
    }
}

// --- brzda hromadných změn --------------------------------------------------

#[test]
fn brzda_prahy() {
    // (popis, počet smazání, synchronizováno, má zabrzdit)
    let pripady: Vec<(&str, u64, u64, bool)> = vec![
        ("50 z 1000: přesně na absolutním limitu", 50, 1000, false),
        ("51 z 1000: nad absolutním limitem", 51, 1000, true),
        ("10 z 20: 50 %, ale ne víc než 10 souborů", 10, 20, false),
        ("11 z 100: 11 %", 11, 100, true),
        ("11 z 110: přesně 10 %", 11, 110, false),
        ("11 z 109: těsně přes 10 %", 11, 109, true),
        ("11 z 111: pod 10 %", 11, 111, false),
        ("1 z 1: malá složka nebrzdí", 1, 1, false),
    ];
    for (popis, pocet, celkem, ma_zabrzdit) in pripady {
        let v = posud(smazat(pocet), &kontext(celkem));
        assert_eq!(hromadna(&v).is_some(), ma_zabrzdit, "{popis}: {v:?}");
    }
}

#[test]
fn brzda_co_se_pocita() {
    let presuny: Vec<Akce> = (0..100)
        .map(|i| Akce::PresunoutLokalne {
            z: c(&format!("f{i}")),
            na: c(&format!("g{i}")),
        })
        .collect();
    let prepisy: Vec<Akce> = (0..60)
        .map(|i| nahrat(&format!("f{i}"), Some("v1")))
        .collect();
    let nove: Vec<Akce> = (0..60).map(|i| nahrat(&format!("n{i}"), None)).collect();
    let stazeni_prepis: Vec<Akce> = (0..60).map(|i| stahnout(&format!("f{i}"), true)).collect();
    let stazeni_nova: Vec<Akce> = (0..60).map(|i| stahnout(&format!("n{i}"), false)).collect();
    let lokalni_kos: Vec<Akce> = (0..60)
        .map(|i| Akce::PresunoutLokalneDoKose {
            cesta: c(&format!("f{i}")),
        })
        .collect();

    let pripady: Vec<(&str, Vec<Akce>, bool)> = vec![
        ("přesuny se nepočítají", presuny, false),
        ("přepsání na serveru se počítá", prepisy, true),
        ("nové soubory se nepočítají", nove, false),
        (
            "stažení přes lokální soubor se počítá",
            stazeni_prepis,
            true,
        ),
        ("stažení nového souboru se nepočítá", stazeni_nova, false),
        ("lokální koš se počítá", lokalni_kos, true),
    ];
    for (popis, plan, ma_zabrzdit) in pripady {
        let v = posud(plan, &kontext(200));
        assert_eq!(hromadna(&v).is_some(), ma_zabrzdit, "{popis}: {v:?}");
    }
}

#[test]
fn brzda_potvrzeni_plati_jen_pro_stejny_plan() {
    let k = kontext(200);
    let Err(Zamitnuti::HromadnaZmenaCekaNaPotvrzeni(souhrn)) = posoudit(smazat(120), &k) else {
        panic!("čekala se brzda");
    };

    let mut potvrzeno = k.clone();
    potvrzeno.potvrzeni = Some(souhrn.potvrdit());
    assert_eq!(
        posud(smazat(120), &potvrzeno),
        ok(120),
        "stejný plán po potvrzení"
    );
    assert!(
        hromadna(&posud(smazat(121), &potvrzeno)).is_some(),
        "jiný plán potvrzení nepřebírá"
    );
}

#[test]
fn brzda_nepocita_odlozene_akce() {
    let mut k = kontext(200);
    k.rezim = Rezim::JenCteni(DuvodJenCteni::Nastaveni);
    let v = posud(smazat(120), &k);
    assert_eq!(
        v,
        odlozit(
            0,
            vec![DuvodOdlozeni::JenCteni(DuvodJenCteni::Nastaveni); 120]
        )
    );
}

// --- odložené akce ----------------------------------------------------------

#[test]
fn rezim_jen_cteni() {
    for duvod in [
        DuvodJenCteni::Nastaveni,
        DuvodJenCteni::PrvniStazeni,
        DuvodJenCteni::NeznamaOdpovedApi,
        DuvodJenCteni::VzdalenaPojistka,
    ] {
        let mut k = kontext(3);
        k.rezim = Rezim::JenCteni(duvod);
        let plan = vec![
            nahrat("f0", Some("v1")),
            do_kose("f1", "v1"),
            Akce::VytvoritSlozkuNaServeru { cesta: c("s") },
            stahnout("f2", true),
            Akce::PresunoutLokalne {
                z: c("f2"),
                na: c("x"),
            },
        ];
        assert_eq!(
            posud(plan, &k),
            odlozit(2, vec![DuvodOdlozeni::JenCteni(duvod); 3]),
            "{duvod:?}: zápis na server čeká, lokální akce projdou"
        );
    }
}

#[test]
fn rezim_jen_cteni_neohlasuje_konflikt() {
    let mut k = kontext(1);
    k.rezim = Rezim::JenCteni(DuvodJenCteni::Nastaveni);
    assert_eq!(
        posud(vec![nahrat("f0", Some("stary"))], &k),
        odlozit(0, vec![DuvodOdlozeni::JenCteni(DuvodJenCteni::Nastaveni)])
    );
}

#[test]
fn ustaleni_souboru() {
    let pripady: Vec<(&str, Duration, Vysledek)> = vec![
        (
            "právě se zapisuje",
            Duration::ZERO,
            odlozit(0, vec![DuvodOdlozeni::SouborSeZapisuje]),
        ),
        (
            "4,999 s",
            Duration::from_millis(4999),
            odlozit(0, vec![DuvodOdlozeni::SouborSeZapisuje]),
        ),
        ("přesně 5 s", Duration::from_secs(5), ok(1)),
        ("hodinu beze změny", Duration::from_secs(3600), ok(1)),
    ];
    for (popis, stabilni_po, ocekavano) in pripady {
        assert_eq!(
            posud(vec![nahrat_stabilni("a", stabilni_po)], &kontext(0)),
            ocekavano,
            "{popis}"
        );
    }
}

#[test]
fn ignorovane_soubory() {
    let ign = || odlozit(0, vec![DuvodOdlozeni::IgnorovanySoubor]);
    let pripady: Vec<(&str, Akce, Vysledek)> = vec![
        (
            "zámek LibreOffice",
            nahrat(".~lock.zprava.odt#", None),
            ign(),
        ),
        ("zámek ve složce", nahrat("dok/.~lock.a.ods#", None), ign()),
        ("swap vimu", nahrat(".zprava.txt.swp", None), ign()),
        (
            "nedokončené stahování",
            nahrat("film.mkv.part", None),
            ign(),
        ),
        ("soubor z Macu", nahrat(".DS_Store", None), ign()),
        (
            "soubor z Macu ze serveru",
            stahnout("fotky/.DS_Store", false),
            ign(),
        ),
        (
            "podobný běžný název",
            nahrat(".~lock.bez-krizku", None),
            ok(1),
        ),
        ("swp uprostřed názvu", nahrat("a.swp.txt", None), ok(1)),
        ("DS_Store bez tečky", nahrat("DS_Store", None), ok(1)),
    ];
    for (popis, akce, ocekavano) in pripady {
        assert_eq!(posud(vec![akce], &kontext(0)), ocekavano, "{popis}");
    }
}

#[test]
fn nepovolene_nazvy() {
    let chyba = |ch| odlozit(0, vec![DuvodOdlozeni::NepovolenyNazev(ch)]);
    let dlouhy = "a".repeat(256);
    let presne_255 = "a".repeat(255);
    // 128 dvoubajtových znaků = 256 bajtů
    let dlouhy_cesky = "č".repeat(128);
    let pripady: Vec<(&str, String, Vysledek)> = vec![
        (
            "běžný název s diakritikou",
            "Příloha – smlouva.pdf".into(),
            ok(1),
        ),
        (
            "prázdná část",
            "a//b".into(),
            chyba(ChybaNazvu::PrazdnaCast),
        ),
        (
            "lomítko na začátku",
            "/a".into(),
            chyba(ChybaNazvu::PrazdnaCast),
        ),
        (
            "lomítko na konci",
            "a/".into(),
            chyba(ChybaNazvu::PrazdnaCast),
        ),
        (
            "tečka",
            "a/./b".into(),
            chyba(ChybaNazvu::TeckaNeboDveTecky),
        ),
        (
            "dvě tečky",
            "../b".into(),
            chyba(ChybaNazvu::TeckaNeboDveTecky),
        ),
        (
            "nový řádek",
            "a\nb".into(),
            chyba(ChybaNazvu::NepovolenyZnak('\n')),
        ),
        (
            "nulový znak",
            "a\0b".into(),
            chyba(ChybaNazvu::NepovolenyZnak('\0')),
        ),
        ("255 bajtů", presne_255, ok(1)),
        ("256 bajtů", dlouhy, chyba(ChybaNazvu::PrilisDlouhy)),
        (
            "256 bajtů v češtině",
            dlouhy_cesky,
            chyba(ChybaNazvu::PrilisDlouhy),
        ),
    ];
    for (popis, cesta, ocekavano) in pripady {
        assert_eq!(
            posud(vec![nahrat(&cesta, None)], &kontext(0)),
            ocekavano,
            "{popis}"
        );
    }
}

#[test]
fn prazdna_cesta() {
    assert_eq!(
        posud(vec![nahrat("", None)], &kontext(0)),
        odlozit(
            0,
            vec![DuvodOdlozeni::NepovolenyNazev(ChybaNazvu::PrazdnaCast)]
        )
    );
}

#[test]
fn kolize_velikosti_pismen() {
    let mut k = kontext(0);
    k.vzdalene_etagy.insert(c("Foto.jpg"), e("v1"));
    k.vzdalene_etagy.insert(c("Dokumenty/Příloha.pdf"), e("v1"));
    k.pocet_synchronizovanych = 2;
    k.pocet_vzdalenych_polozek = 2;
    k.lokalni_slozka = StavLokalniSlozky::Dostupna { pocet_souboru: 2 };

    let kolize = |s: &str| odlozit(0, vec![DuvodOdlozeni::KolizeVelikostiPismen { s: c(s) }]);
    let pripady: Vec<(&str, Vec<Akce>, Vysledek)> = vec![
        (
            "jiná velikost než na serveru",
            vec![nahrat("foto.jpg", None)],
            kolize("Foto.jpg"),
        ),
        (
            "česká písmena",
            vec![nahrat("Dokumenty/PŘÍLOHA.pdf", None)],
            kolize("Dokumenty/Příloha.pdf"),
        ),
        (
            "dva nové názvy v plánu",
            vec![nahrat("a.txt", None), nahrat("A.txt", None)],
            odlozit(
                1,
                vec![DuvodOdlozeni::KolizeVelikostiPismen { s: c("a.txt") }],
            ),
        ),
        (
            "přesun na kolidující název",
            vec![Akce::PresunoutNaServeru {
                z: c("Foto.jpg"),
                na: c("FOTO.JPG"),
                ocekavany_etag: e("v1"),
            }],
            kolize("Foto.jpg"),
        ),
        (
            "stejný název v jiné složce",
            vec![nahrat("jinde/foto.jpg", None)],
            ok(1),
        ),
        (
            "přepsání existujícího",
            vec![nahrat("Foto.jpg", Some("v1"))],
            ok(1),
        ),
    ];
    for (popis, plan, ocekavano) in pripady {
        assert_eq!(posud(plan, &k), ocekavano, "{popis}");
    }
}

// --- volné místo ------------------------------------------------------------

#[test]
fn volne_misto() {
    let s_mistem = |icloud: Option<u64>, disk: Option<u64>| {
        let mut k = kontext(3);
        k.volne_misto_icloud = icloud;
        k.volne_misto_disk = disk;
        k
    };
    // každá akce z pomocníků má 10 bajtů
    let plan = || vec![nahrat("a", None), nahrat("b", None), stahnout("f0", true)];
    let pripady: Vec<(&str, Kontext, Vysledek)> = vec![
        ("místo neznámé", s_mistem(None, None), ok(3)),
        ("přesně dost", s_mistem(Some(20), Some(10)), ok(3)),
        (
            "málo na iCloudu",
            s_mistem(Some(19), Some(100)),
            Vysledek::Zamitnuto(Zamitnuti::MaloMistaNaICloudu {
                potreba: 20,
                volno: 19,
            }),
        ),
        (
            "málo na disku",
            s_mistem(Some(100), Some(9)),
            Vysledek::Zamitnuto(Zamitnuti::MaloMistaNaDisku {
                potreba: 10,
                volno: 9,
            }),
        ),
        (
            "plný iCloud",
            s_mistem(Some(0), None),
            Vysledek::Zamitnuto(Zamitnuti::MaloMistaNaICloudu {
                potreba: 20,
                volno: 0,
            }),
        ),
    ];
    for (popis, k, ocekavano) in pripady {
        assert_eq!(posud(plan(), &k), ocekavano, "{popis}");
    }
}

#[test]
fn velke_soubory_nepretecou() {
    let mut k = kontext(0);
    k.volne_misto_icloud = Some(u64::MAX - 1);
    let obri = |cesta: &str| Akce::Nahrat {
        cesta: c(cesta),
        ocekavany_etag: None,
        velikost: u64::MAX,
        stabilni_po: Duration::from_secs(60),
    };
    assert_eq!(
        posud(vec![obri("a"), obri("b")], &k),
        Vysledek::Zamitnuto(Zamitnuti::MaloMistaNaICloudu {
            potreba: u64::MAX,
            volno: u64::MAX - 1
        })
    );
}

// --- schválená akce ---------------------------------------------------------

#[test]
fn schvalena_akce_nese_puvodni_akci() {
    let akce = nahrat("f0", Some("v1"));
    let Ok(Rozhodnuti::Schvaleno(plan)) = posoudit(vec![akce.clone()], &kontext(1)) else {
        panic!("čekalo se schválení");
    };
    let [schvalena] = plan.akce.as_slice() else {
        panic!("čekala se jedna akce");
    };
    assert_eq!(schvalena.akce(), &akce);
}
