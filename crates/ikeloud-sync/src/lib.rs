//! Sync engine of ike loud.
//!
//! Compares three states (local, remote, last common) and builds a plan of
//! actions following the decision table of the spec. It executes nothing
//! itself: the plan goes to `ikeloud-safety` for approval. The algorithm
//! matches the TLA+ model `tla/Sync.tla`.

pub mod names;
pub mod plan;
pub mod state;

pub use plan::{Collision, Plan, Side, build_plan};
pub use state::{BaseEntry, ContentHash, LocalFile, RemoteFile, States};
