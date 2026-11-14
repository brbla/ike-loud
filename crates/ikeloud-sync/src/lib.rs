//! Synchronizační engine ike loud.
//!
//! Porovná tři stavy (lokální, vzdálený, poslední společný) a sestaví plán
//! akcí. Sám nic neprovádí: plán předá ke schválení `ikeloud-safety`.
