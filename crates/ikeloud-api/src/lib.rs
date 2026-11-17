//! iCloud web services client.
//!
//! Sign-in (SRP-6a, 2FA, trust token, ADP), folder listing, download,
//! upload and moving to the trash. No sync logic. Write operations will
//! require an approval from `ikeloud-safety`.

mod b64;
pub mod changes;
pub mod client;
pub mod cookies;
pub mod drive;
pub mod http;
pub mod srp;
pub mod write;

pub use changes::{RecordKind, ZoneChanges, ZoneRecord};
pub use client::{Account, ApiError, Client, SavedSession, SignIn, TrustedPhone};
pub use drive::{Folder, Item, Kind, Opened, ROOT};
pub use http::HttpsTransport;
pub use write::{CreatedFolder, UploadTarget, Uploaded};
