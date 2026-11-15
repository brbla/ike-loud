//! Sync engine of ike loud.
//!
//! Compares three states (local, remote, last common) and builds a plan of
//! actions following the decision table of the spec. It executes nothing
//! itself: the plan goes to `ikeloud-safety` for approval, the approved
//! actions to [`execute`], and the result to [`next_base`]. The algorithm
//! matches the TLA+ models `tla/Sync.tla` and `tla/SyncMoves.tla`.

pub mod base;
pub mod execute;
pub mod names;
pub mod plan;
pub mod state;

pub use base::{Transfer, next_base};
pub use execute::{EffectError, Effects, Report, StopReason, execute};
pub use plan::{Collision, Plan, Side, build_plan};
pub use state::{BaseEntry, ContentHash, DocId, LocalFile, RemoteFile, States};
