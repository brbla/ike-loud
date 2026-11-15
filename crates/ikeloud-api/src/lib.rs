//! Klient webových služeb iCloudu.
//!
//! Přihlášení (SRP-6a, 2FA, trust token, ADP), výpis složek, stahování,
//! nahrávání a přesun do koše. Žádná synchronizační logika. Zápisové
//! operace budou vyžadovat schválení z `ikeloud-safety`.

pub mod srp;
