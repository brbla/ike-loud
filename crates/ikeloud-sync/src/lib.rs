//! Synchronizační engine ike loud.
//!
//! Porovná tři stavy (lokální, vzdálený, poslední společný) a sestaví plán
//! akcí podle rozhodovací tabulky ze zadání. Sám nic neprovádí: plán předá
//! ke schválení `ikeloud-safety`. Algoritmus odpovídá TLA+ modelu
//! `tla/Sync.tla`.

pub mod nazvy;
pub mod plan;
pub mod stav;

pub use plan::{Kolize, Plan, Strana, sestavit_plan};
pub use stav::{LokalniSoubor, OtiskObsahu, SpolecnyStav, Stavy, VzdalenySoubor};
