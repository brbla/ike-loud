//! Testy překladů: úplnost jazyků, tvary, náhradní jazyk, nové jazyky.

#![allow(clippy::expect_used, clippy::panic)] // testovací kód smí spadnout

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use ikeloud_i18n::{
    ChybaFormatovani, ChybaPrekladu, DoZpravy, Hodnota, Preklady, ZABUDOVANE, Zprava,
};
use ikeloud_safety::nazvy::ChybaNazvu;
use ikeloud_safety::{Akce, Cesta, DuvodJenCteni, DuvodOdlozeni, Kontext, Zamitnuti, posoudit};

fn jazyk(kod: &str) -> Preklady {
    Preklady::new(&[kod.to_string()]).expect("zabudované překlady jdou načíst")
}

/// Klíče zpráv ze souboru .ftl (řádky `klic = …` na začátku řádku).
fn klice(text: &str) -> BTreeSet<String> {
    text.lines()
        .filter_map(|r| r.split_once('='))
        .map(|(klic, _)| klic.trim_end())
        .filter(|k| {
            k.chars().next().is_some_and(|z| z.is_ascii_lowercase())
                && k.chars()
                    .all(|z| z.is_ascii_lowercase() || z.is_ascii_digit() || z == '-')
        })
        .map(ToString::to_string)
        .collect()
}

/// Jedna zpráva od každé varianty, kterou může uživatel vidět.
fn vsechny_zpravy() -> Vec<Zprava> {
    let k = Kontext::new(200);
    let Err(hromadna) = posoudit(
        (0..120)
            .map(|i| Akce::PresunoutLokalneDoKose {
                cesta: Cesta::new(format!("f{i}")),
            })
            .collect(),
        &k,
    ) else {
        panic!("čekala se brzda");
    };

    let jen_cteni = [
        DuvodJenCteni::Nastaveni,
        DuvodJenCteni::PrvniStazeni,
        DuvodJenCteni::NeznamaOdpovedApi,
        DuvodJenCteni::VzdalenaPojistka,
    ];
    let mut zpravy: Vec<Zprava> = vec![
        Zprava::new("aplikace-nazev"),
        Zprava::new("aplikace-popis"),
        Zamitnuti::LokalniSlozkaNedostupna.zprava(),
        Zamitnuti::PodezrelyVypisServeru.zprava(),
        Zamitnuti::Konflikt(vec![Cesta::new("a")]).zprava(),
        hromadna.zprava(),
        Zamitnuti::MaloMistaNaICloudu {
            potreba: 2_000_000,
            volno: 1_000,
        }
        .zprava(),
        Zamitnuti::MaloMistaNaDisku {
            potreba: 2_000_000,
            volno: 1_000,
        }
        .zprava(),
        DuvodOdlozeni::SouborSeZapisuje.zprava(),
        DuvodOdlozeni::IgnorovanySoubor.zprava(),
        DuvodOdlozeni::KolizeVelikostiPismen {
            s: Cesta::new("a/Foto.jpg"),
        }
        .zprava(),
    ];
    for chyba in [
        ChybaNazvu::PrazdnaCast,
        ChybaNazvu::TeckaNeboDveTecky,
        ChybaNazvu::NepovolenyZnak('\n'),
        ChybaNazvu::PrilisDlouhy,
    ] {
        zpravy.push(DuvodOdlozeni::NepovolenyNazev(chyba).zprava());
    }
    for d in jen_cteni {
        zpravy.push(d.zprava());
        zpravy.push(DuvodOdlozeni::JenCteni(d).zprava());
    }
    for klic in [
        "velikost-bajty",
        "velikost-kb",
        "velikost-mb",
        "velikost-gb",
        "velikost-tb",
    ] {
        zpravy.push(Zprava::new(klic).s("hodnota", Hodnota::Text("1".into())));
    }
    zpravy.push(Zprava::new("oddelovac-desetin"));
    zpravy
}

#[test]
fn vsechny_jazyky_maji_stejne_klice() {
    let zaklad = klice(
        ZABUDOVANE
            .iter()
            .find(|(k, _)| *k == "en")
            .map(|(_, t)| *t)
            .expect("angličtina je zabudovaná"),
    );
    assert!(!zaklad.is_empty());
    for (kod, text) in ZABUDOVANE {
        let k = klice(text);
        let chybi: Vec<_> = zaklad.difference(&k).collect();
        let navic: Vec<_> = k.difference(&zaklad).collect();
        assert!(
            chybi.is_empty() && navic.is_empty(),
            "{kod}: chybí {chybi:?}, navíc {navic:?}"
        );
    }
}

#[test]
fn vsechny_zpravy_maji_preklad() {
    let zpravy = vsechny_zpravy();
    let pouzite: BTreeSet<String> = zpravy.iter().map(|z| z.klic.to_string()).collect();
    for (kod, text) in ZABUDOVANE {
        let p = jazyk(kod);
        assert_eq!(p.jazyky().first().map(String::as_str), Some(*kod));
        for z in &zpravy {
            let t = p.text_presne(z);
            assert!(
                t.as_ref()
                    .is_ok_and(|t| !t.trim().is_empty() && !t.contains('{')),
                "{kod}: {}: {t:?}",
                z.klic
            );
        }
        // Žádný klíč v souboru není mrtvý.
        let nepouzite: Vec<_> = klice(text).difference(&pouzite).cloned().collect();
        assert!(nepouzite.is_empty(), "{kod}: nepoužité klíče {nepouzite:?}");
    }
}

#[test]
fn ceske_tvary_mnozneho_cisla() {
    let cs = jazyk("cs");
    let konflikt = |n: usize| {
        let cesty = (0..n).map(|i| Cesta::new(format!("f{i}"))).collect();
        cs.text(&Zamitnuti::Konflikt(cesty).zprava())
    };
    let pripady: &[(usize, &str)] = &[
        (1, "Jeden soubor se mezitím změnil"),
        (2, "2 soubory se mezitím změnily"),
        (4, "4 soubory se mezitím změnily"),
        (5, "5 souborů se mezitím změnilo"),
        (22, "22 souborů se mezitím změnilo"),
    ];
    for (n, zacatek) in pripady {
        let t = konflikt(*n);
        assert!(t.starts_with(zacatek), "{n}: {t}");
    }
}

#[test]
fn anglicke_tvary_mnozneho_cisla() {
    let en = jazyk("en");
    let konflikt = |n: usize| {
        let cesty = (0..n).map(|i| Cesta::new(format!("f{i}"))).collect();
        en.text(&Zamitnuti::Konflikt(cesty).zprava())
    };
    assert!(konflikt(1).starts_with("One file was"), "{}", konflikt(1));
    assert!(konflikt(3).starts_with("3 files were"), "{}", konflikt(3));
}

#[test]
fn velikosti_podle_jazyka() {
    let zprava = Zamitnuti::MaloMistaNaICloudu {
        potreba: 1_500_000_000,
        volno: 250_000,
    }
    .zprava();
    let pripady: &[(&str, &str)] = &[
        ("cs", "potřeba 1,5 GB, volno 250 kB"),
        ("en", "1.5 GB needed, 250 kB free"),
    ];
    for (kod, ocekavano) in pripady {
        let t = jazyk(kod).text(&zprava);
        assert!(t.contains(ocekavano), "{kod}: {t}");
    }
}

#[test]
fn vnorena_zprava_ve_stejnem_jazyce() {
    let z = DuvodOdlozeni::JenCteni(DuvodJenCteni::PrvniStazeni).zprava();
    assert_eq!(
        jazyk("cs").text(&z),
        "Zatím se jen stahuje. Zápis na iCloud se zapne, až se dokončí a zkontroluje první stažení."
    );
    assert_eq!(
        jazyk("en").text(&z),
        "Only downloading for now. Writing to iCloud turns on once the first download has finished and been checked."
    );
}

#[test]
fn kolize_ukazuje_jen_nazev() {
    let z = DuvodOdlozeni::KolizeVelikostiPismen {
        s: Cesta::new("Dokumenty/Příloha.pdf"),
    }
    .zprava();
    assert!(
        jazyk("cs")
            .text(&z)
            .starts_with("Na iCloudu už je „Příloha.pdf“.")
    );
}

#[test]
fn vyber_jazyka_a_zaloha() {
    let p = |pozadovane: &[&str]| {
        Preklady::new(
            &pozadovane
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
        )
        .expect("načtení")
        .jazyky()
    };
    assert_eq!(p(&["cs-CZ"]), ["cs", "en"]);
    assert_eq!(p(&["ja"]), ["en"]);
    assert_eq!(p(&[]), ["en"]);
}

#[test]
fn neznamy_klic_vrati_klic() {
    let z = Zprava::new("tento-klic-neexistuje");
    assert_eq!(jazyk("cs").text(&z), "tento-klic-neexistuje");
    assert_eq!(
        jazyk("cs").text_presne(&z),
        Err(ChybaFormatovani::ChybiKlic("tento-klic-neexistuje"))
    );
}

#[test]
fn chybejici_argument_je_chyba() {
    let z = Zprava::new("zamitnuti-konflikt");
    assert!(matches!(
        jazyk("cs").text_presne(&z),
        Err(ChybaFormatovani::Formatovani {
            klic: "zamitnuti-konflikt",
            ..
        })
    ));
}

// --- jazyky ze složky --------------------------------------------------------

struct DocasnaSlozka(PathBuf);

impl DocasnaSlozka {
    fn new(jmeno: &str) -> Self {
        let cesta =
            std::env::temp_dir().join(format!("ikeloud-i18n-{jmeno}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&cesta);
        std::fs::create_dir_all(&cesta).expect("dočasná složka");
        Self(cesta)
    }

    fn jazyk(&self, kod: &str, text: &str) -> &Self {
        let slozka = self.0.join(kod);
        std::fs::create_dir_all(&slozka).expect("složka jazyka");
        std::fs::write(slozka.join("ikeloud.ftl"), text).expect("soubor překladu");
        self
    }

    fn cesta(&self) -> &Path {
        &self.0
    }
}

impl Drop for DocasnaSlozka {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn novy_jazyk_ze_slozky_a_nahrada_z_anglictiny() {
    let d = DocasnaSlozka::new("novy");
    d.jazyk(
        "de",
        "aplikace-popis = Inoffizieller Client für iCloud Drive\n",
    );
    let p = Preklady::s_adresarem(&["de-AT".into()], d.cesta()).expect("načtení");
    assert_eq!(p.jazyky(), ["de", "en"]);
    assert_eq!(
        p.text(&Zprava::new("aplikace-popis")),
        "Inoffizieller Client für iCloud Drive"
    );
    // Chybějící text se vezme z angličtiny.
    assert_eq!(
        p.text(&DuvodOdlozeni::SouborSeZapisuje.zprava()),
        "The file is still being saved. It will be uploaded once it is done."
    );
}

#[test]
fn slozka_nahradi_zabudovany_preklad() {
    let d = DocasnaSlozka::new("nahrada");
    d.jazyk("cs", "aplikace-popis = Upravený popis\n");
    let p = Preklady::s_adresarem(&["cs".into()], d.cesta()).expect("načtení");
    assert_eq!(p.text(&Zprava::new("aplikace-popis")), "Upravený popis");
}

#[test]
fn chyby_ve_slozce() {
    let d = DocasnaSlozka::new("chyby");
    d.jazyk("de", "aplikace-popis = {\n");
    assert!(matches!(
        Preklady::s_adresarem(&["de".into()], d.cesta()),
        Err(ChybaPrekladu::Syntaxe { ref jazyk, .. }) if jazyk == "de"
    ));
    // Rozbitý jazyk, který nikdo nechce, aplikaci nezastaví.
    assert_eq!(
        Preklady::s_adresarem(&["cs".into()], d.cesta()).map(|p| p.jazyky()),
        Ok(vec!["cs".to_string(), "en".to_string()])
    );

    let d = DocasnaSlozka::new("kod");
    d.jazyk("neni_jazyk!", "a = b\n");
    assert_eq!(
        Preklady::s_adresarem(&[], d.cesta()).map(|p| p.jazyky()),
        Err(ChybaPrekladu::NeplatnyKodJazyka("neni_jazyk!".into()))
    );

    let neexistuje = std::env::temp_dir().join("ikeloud-i18n-neexistuje-xyz");
    assert!(matches!(
        Preklady::s_adresarem(&[], &neexistuje),
        Err(ChybaPrekladu::Cteni { .. })
    ));

    // Složka bez souboru překladu se přeskočí.
    let d = DocasnaSlozka::new("prazdna");
    std::fs::create_dir_all(d.cesta().join("fr")).expect("složka");
    assert_eq!(
        Preklady::s_adresarem(&["fr".into()], d.cesta()).map(|p| p.jazyky()),
        Ok(vec!["en".to_string()])
    );
}
