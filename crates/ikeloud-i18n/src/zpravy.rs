//! Převod typů z ostatních crates na zprávy s klíči.
//!
//! Každá varianta má vlastní klíč; test `vsechny_zpravy_maji_preklad`
//! hlídá, že klíč existuje ve všech zabudovaných jazycích.

use ikeloud_safety::nazvy::ChybaNazvu;
use ikeloud_safety::{DuvodJenCteni, DuvodOdlozeni, Zamitnuti};

use crate::{Hodnota, Zprava};

/// Typ, který umí říct, jakou zprávou se ukáže uživateli.
pub trait DoZpravy {
    /// Zpráva pro uživatele.
    fn zprava(&self) -> Zprava;
}

impl DoZpravy for Zamitnuti {
    fn zprava(&self) -> Zprava {
        match self {
            Self::LokalniSlozkaNedostupna => Zprava::new("zamitnuti-lokalni-slozka-nedostupna"),
            Self::PodezrelyVypisServeru => Zprava::new("zamitnuti-podezrely-vypis-serveru"),
            Self::Konflikt(cesty) => {
                Zprava::new("zamitnuti-konflikt").s("pocet", Hodnota::Cislo(cesty.len() as u64))
            }
            Self::HromadnaZmenaCekaNaPotvrzeni(souhrn) => Zprava::new("zamitnuti-hromadna-zmena")
                .s("pocet", Hodnota::Cislo(souhrn.pocet))
                .s("celkem", Hodnota::Cislo(souhrn.celkem)),
            Self::MaloMistaNaICloudu { potreba, volno } => {
                Zprava::new("zamitnuti-malo-mista-icloud")
                    .s("potreba", Hodnota::Velikost(*potreba))
                    .s("volno", Hodnota::Velikost(*volno))
            }
            Self::MaloMistaNaDisku { potreba, volno } => Zprava::new("zamitnuti-malo-mista-disk")
                .s("potreba", Hodnota::Velikost(*potreba))
                .s("volno", Hodnota::Velikost(*volno)),
        }
    }
}

impl DoZpravy for DuvodOdlozeni {
    fn zprava(&self) -> Zprava {
        match self {
            Self::JenCteni(duvod) => Zprava::new("odlozeno-jen-cteni")
                .s("duvod", Hodnota::Zprava(Box::new(duvod.zprava()))),
            Self::SouborSeZapisuje => Zprava::new("odlozeno-soubor-se-zapisuje"),
            Self::NepovolenyNazev(chyba) => chyba.zprava(),
            Self::IgnorovanySoubor => Zprava::new("odlozeno-ignorovany-soubor"),
            Self::KolizeVelikostiPismen { s } => Zprava::new("odlozeno-kolize-velikosti")
                .s("s", Hodnota::Text(s.nazev().to_string())),
        }
    }
}

impl DoZpravy for ChybaNazvu {
    fn zprava(&self) -> Zprava {
        match self {
            Self::PrazdnaCast => Zprava::new("odlozeno-nazev-prazdna-cast"),
            Self::TeckaNeboDveTecky => Zprava::new("odlozeno-nazev-tecka"),
            Self::NepovolenyZnak(_) => Zprava::new("odlozeno-nazev-znak"),
            Self::PrilisDlouhy => Zprava::new("odlozeno-nazev-prilis-dlouhy"),
        }
    }
}

impl DoZpravy for DuvodJenCteni {
    fn zprava(&self) -> Zprava {
        Zprava::new(match self {
            Self::Nastaveni => "jen-cteni-nastaveni",
            Self::PrvniStazeni => "jen-cteni-prvni-stazeni",
            Self::NeznamaOdpovedApi => "jen-cteni-neznama-odpoved-api",
            Self::VzdalenaPojistka => "jen-cteni-vzdalena-pojistka",
        })
    }
}
