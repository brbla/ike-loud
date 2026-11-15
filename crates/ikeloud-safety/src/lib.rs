//! Safety layer of ike loud.
//!
//! The only place allowed to approve a write to the server. It assesses the
//! plan of actions from `ikeloud-sync` against the rules in the "Data
//! protection" chapter of the spec: it allows an action, defers it, or stops
//! the whole plan and waits for the user. An approved action is the type
//! [`ApprovedAction`], which only this crate can create; write functions in
//! `ikeloud-api` require it.
//!
//! The crate deliberately uses nothing but the standard library so that it
//! can be read line by line.

pub mod action;
pub mod assessment;
pub mod context;
pub mod names;

pub use action::{
    Action, ContentHash, Destruction, DestructionKind, Etag, LocalId, LocalVersion, RelPath,
};
pub use assessment::{
    ApprovedAction, ApprovedPlan, Decision, DeferReason, DeferredAction, MassChangeConfirmation,
    MassChangeSummary, Rejection, assess,
};
pub use context::{
    Context, DEFAULT_SETTLE_TIME, LocalFolderState, MassChangeLimits, Mode, ReadOnlyReason,
};
