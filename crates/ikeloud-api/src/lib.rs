//! iCloud web services client.
//!
//! Sign-in (SRP-6a, 2FA, trust token, ADP), folder listing, download,
//! upload and moving to the trash. No sync logic. Write operations will
//! require an approval from `ikeloud-safety`.

pub mod srp;
