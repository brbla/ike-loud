//! Bezpečnostní vrstva ike loud.
//!
//! Jediné místo, které smí schválit zápis na server. Posuzuje plán akcí
//! z `ikeloud-sync` podle pravidel z kapitoly „Ochrana dat“ v zadání:
//! akci pustí, odloží, nebo celý plán zastaví a čeká na uživatele.
//! Schválená akce je typ [`SchvalenaAkce`], který umí vyrobit jen tento
//! crate; zápisové funkce v `ikeloud-api` ho vyžadují.
//!
//! Crate záměrně nepoužívá nic kromě standardní knihovny, aby ho šlo
//! číst řádek po řádku.

pub mod akce;
pub mod kontext;
pub mod nazvy;
pub mod posouzeni;

pub use akce::{Akce, Cesta, Etag};
pub use kontext::{
    DuvodJenCteni, Kontext, LimityHromadneZmeny, Rezim, StavLokalniSlozky, VYCHOZI_DOBA_USTALENI,
};
pub use posouzeni::{
    DuvodOdlozeni, OdlozenaAkce, PotvrzeniHromadneZmeny, Rozhodnuti, SchvalenaAkce, SchvalenyPlan,
    SouhrnHromadneZmeny, Zamitnuti, posoudit,
};
