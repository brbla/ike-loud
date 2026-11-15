//! Carrying out an approved plan.
//!
//! The executor runs the approved actions in plan order through
//! [`Effects`], which does the real work (local disk, server API) and checks
//! each action's preconditions right before acting. It never sees an action
//! the safety layer did not approve: it takes [`ApprovedAction`]s, the only
//! type the effects accept.
//!
//! An action whose preconditions no longer hold is skipped; the next scan
//! and plan pick it up. An error that makes further work pointless (network
//! down, signed out, disk full) stops the run. Either way the caller rescans
//! both sides and computes the new common state with
//! [`crate::base::next_base`] from the transfers in the report.

use ikeloud_safety::{Action, ApprovedAction};

use crate::base::Transfer;

/// Why an effect did not happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectError {
    /// A precondition no longer holds (the file changed, the target is
    /// taken, the etag differs). Nothing was changed; skip the action.
    Changed,
    /// Further actions make no sense now (network down, signed out, disk
    /// full). Nothing was changed by this action; stop the run.
    Stop(StopReason),
}

/// Why a run stopped early.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// No connection to the server.
    Offline,
    /// The sign-in expired.
    SignedOut,
    /// No space left on iCloud or on the disk.
    NoSpace,
    /// The server answered something unknown (switch to read-only mode).
    UnknownResponse,
}

/// The side effects of sync: implemented by the daemon over the local disk
/// and the server API, and by the simulation in tests.
///
/// Contract for every implementation:
/// - check the action's preconditions immediately before acting and return
///   [`EffectError::Changed`] without changing anything if they do not hold
///   (server writes are conditional on the expected etag; local writes check
///   the file is unchanged and the target free, and keep the previous
///   version in local history);
/// - never overwrite a file that is not explicitly expected to be replaced;
/// - for uploads and downloads, return the [`Transfer`] describing the
///   result, so the common state can be updated.
pub trait Effects {
    /// Performs one approved action.
    fn apply(&mut self, action: ApprovedAction) -> Result<Option<Transfer>, EffectError>;
}

/// What a run did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Actions carried out, in order.
    pub done: Vec<Action>,
    /// Actions skipped because a precondition no longer held.
    pub skipped: Vec<Action>,
    /// Actions not attempted because the run stopped.
    pub not_attempted: Vec<Action>,
    /// What the uploads and downloads produced.
    pub transfers: Vec<Transfer>,
    /// Why the run stopped early, if it did.
    pub stopped: Option<StopReason>,
}

/// Runs approved actions in order.
pub fn execute(actions: Vec<ApprovedAction>, effects: &mut dyn Effects) -> Report {
    let mut report = Report::default();
    let mut actions = actions.into_iter();
    while let Some(approved) = actions.next() {
        let action = approved.action().clone();
        match effects.apply(approved) {
            Ok(transfer) => {
                report.transfers.extend(transfer);
                report.done.push(action);
            }
            Err(EffectError::Changed) => report.skipped.push(action),
            Err(EffectError::Stop(reason)) => {
                report.skipped.push(action);
                report.not_attempted = actions.map(|a| a.action().clone()).collect();
                report.stopped = Some(reason);
                break;
            }
        }
    }
    report
}
