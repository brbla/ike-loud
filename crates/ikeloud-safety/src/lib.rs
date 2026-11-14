//! Bezpečnostní vrstva ike loud.
//!
//! Jediné místo, které smí schválit zápis na server. Posuzuje plán akcí
//! z `ikeloud-sync` podle pravidel z kapitoly „Ochrana dat“ v zadání.
//! Schválení bude typ, který umí vyrobit jen tento crate; zápisové funkce
//! v `ikeloud-api` ho budou vyžadovat.
//!
//! Crate záměrně nepoužívá nic kromě standardní knihovny, aby ho šlo
//! číst řádek po řádku.
