//! ike loud daemon: background sync.
//!
//! [`sync_once`] is one sync run: scan the local folder, list the server,
//! plan, let the safety layer assess the plan, carry out the approved
//! actions, and store the new common state and the journal in one
//! transaction. The server is behind [`Remote`]: the real iCloud client, or
//! a fake one in tests.

mod effects;
mod remote;
mod run;

pub use remote::{Download, Listing, Remote, RemoteError, Written};
pub use run::{RunOutcome, RunReport, SyncSetup, sync_once};
