//! Testy přihlášení SRP proti vektorům z pysrp a z varianty rclone.

#![allow(clippy::expect_used, clippy::panic)] // testovací kód smí spadnout

#[path = "data/srp_vektory.rs"]
mod vektory;

use ikeloud_api::srp::{ChybaSrp, Protokol, SrpKlient, Zarovnani, odvod_heslo};
use vektory::{OKRAJE, OKRAJE_HESLO_PBKDF2, OKRAJE_SALT, OKRAJE_UCET, VEKTORY};

fn z_hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2).expect("sudá délka"), 16).expect("hex"))
        .collect()
}

#[test]
fn odvozeni_hesla() {
    for v in VEKTORY {
        let protokol = Protokol::z_textu(v.protokol).expect("známý protokol");
        let odvozene = odvod_heslo(v.heslo, &z_hex(v.salt), v.iterace, protokol).expect("odvození");
        assert_eq!(
            odvozene.to_vec(),
            z_hex(v.heslo_pbkdf2),
            "{} {}",
            v.ucet,
            v.protokol
        );
    }
}

#[test]
fn s2k_a_s2k_fo_se_lisi() {
    let salt = [1u8; 16];
    let s2k = odvod_heslo("heslo", &salt, 10, Protokol::S2k);
    let fo = odvod_heslo("heslo", &salt, 10, Protokol::S2kFo);
    assert_ne!(s2k, fo);
}

#[test]
fn cela_vymena_odpovida_pysrp() {
    // U běžných vektorů jsou A, B i S plné délky, obě varianty musí dát totéž.
    for zarovnani in [Zarovnani::NaDelkuSkupiny, Zarovnani::BezUvodnichNul] {
        for v in VEKTORY {
            let klient = SrpKlient::s_tajemstvim(&z_hex(v.a), zarovnani).expect("klient");
            assert_eq!(klient.verejne_a(), z_hex(v.verejne_a), "A {}", v.protokol);
            let d = klient
                .zpracuj_vyzvu(v.ucet, &z_hex(v.heslo_pbkdf2), &z_hex(v.salt), &z_hex(v.b))
                .expect("výměna");
            let popis = format!("{zarovnani:?} {} {}", v.ucet, v.protokol);
            assert_eq!(d.k.to_vec(), z_hex(v.k), "K {popis}");
            assert_eq!(d.m1.to_vec(), z_hex(v.m1), "M1 {popis}");
            assert_eq!(d.m2.to_vec(), z_hex(v.m2), "M2 {popis}");
        }
    }
}

#[test]
fn okrajove_pripady_obou_variant() {
    for o in OKRAJE {
        for (zarovnani, ocekavano) in [
            (Zarovnani::BezUvodnichNul, &o.pysrp),
            (Zarovnani::NaDelkuSkupiny, &o.rclone),
        ] {
            let klient = SrpKlient::s_tajemstvim(&z_hex(o.a), zarovnani).expect("klient");
            assert_eq!(
                klient.verejne_a(),
                z_hex(ocekavano.verejne_a),
                "A {} {zarovnani:?}",
                o.popis
            );
            let d = klient
                .zpracuj_vyzvu(
                    OKRAJE_UCET,
                    &z_hex(OKRAJE_HESLO_PBKDF2),
                    &z_hex(OKRAJE_SALT),
                    &z_hex(o.b),
                )
                .expect("výměna");
            assert_eq!(
                d.k.to_vec(),
                z_hex(ocekavano.k),
                "K {} {zarovnani:?}",
                o.popis
            );
            assert_eq!(
                d.m1.to_vec(),
                z_hex(ocekavano.m1),
                "M1 {} {zarovnani:?}",
                o.popis
            );
            assert_eq!(
                d.m2.to_vec(),
                z_hex(ocekavano.m2),
                "M2 {} {zarovnani:?}",
                o.popis
            );
        }
        // Varianty se v okrajových případech opravdu liší.
        assert_ne!(o.pysrp.m1, o.rclone.m1, "{}", o.popis);
    }
}

#[test]
fn chybne_vstupy() {
    let klient = SrpKlient::s_tajemstvim(&[7u8; 32], Zarovnani::default()).expect("klient");
    let n = z_hex(concat!(
        "AC6BDB41324A9A9BF166DE5E1389582FAF72B6651987EE07FC3192943DB56050",
        "A37329CBB4A099ED8193E0757767A13DD52312AB4B03310DCD7F48A9DA04FD50",
        "E8083969EDB767B0CF6095179A163AB3661A05FBD5FAAAE82918A9962F0B93B8",
        "55F97993EC975EEAA80D740ADBF4FF747359D041D5C33EA71D281E446B14773B",
        "CA97B43A23FB801676BD207A436C6481F1D2B9078717461A5B9D32E688F87748",
        "544523B524B0D57D5EA77A2775D2ECFA032CFBDBF52FB3786160279004E57AE6",
        "AF874E7303CE53299CCC041C7BC308D82A5698F3A8D0C38271AE35F8E9DBFBB6",
        "94B5C803D89F7AE435DE236D525F54759B65E372FCD68EF20FA7111F9E4AFF73",
    ));
    let mut nad_n = n.clone();
    if let Some(posledni) = nad_n.last_mut() {
        *posledni += 1;
    }
    let pripady: Vec<(&str, Vec<u8>, ChybaSrp)> = vec![
        ("B = 0", vec![0], ChybaSrp::NeplatneB),
        ("prázdné B", vec![], ChybaSrp::NeplatneB),
        ("B = N", n.clone(), ChybaSrp::NeplatneB),
        ("B > N", nad_n, ChybaSrp::NeplatneB),
    ];
    for (popis, b, chyba) in pripady {
        assert_eq!(
            klient.zpracuj_vyzvu("u", b"p", b"s", &b),
            Err(chyba),
            "{popis}"
        );
    }
    assert_eq!(
        SrpKlient::s_tajemstvim(&[0u8; 32], Zarovnani::default()).map(|_| ()),
        Err(ChybaSrp::NeplatneTajemstvi)
    );
    assert_eq!(
        odvod_heslo("p", b"s", 0, Protokol::S2k),
        Err(ChybaSrp::NulovyPocetIteraci)
    );
    assert_eq!(
        Protokol::z_textu("s2k_xyz"),
        Err(ChybaSrp::NeznamyProtokol("s2k_xyz".into()))
    );
    for p in [Protokol::S2k, Protokol::S2kFo] {
        assert_eq!(Protokol::z_textu(p.jako_text()), Ok(p));
    }
}

#[test]
fn nahodny_klient() {
    let a = SrpKlient::new(Zarovnani::default()).expect("náhoda");
    let b = SrpKlient::new(Zarovnani::default()).expect("náhoda");
    assert_eq!(a.verejne_a().len(), 256);
    assert_ne!(a.verejne_a(), b.verejne_a());
    assert!(!format!("{a:?}").contains("a:"), "tajemství se nevypisuje");
}
