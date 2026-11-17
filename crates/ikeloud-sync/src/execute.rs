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
    /// The local disk refused (read-only, permissions, another filesystem).
    LocalDisk,
}

/// What carrying out one action gave.
pub type ApplyResult = Result<Option<Transfer>, EffectError>;

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

    /// Performs independent actions (see [`independent`]), possibly several
    /// at once, and returns one entry per action, in their order: its result,
    /// or `None` when it was not attempted (the run stopped first). The
    /// contract of [`Effects::apply`] holds for each, and each must be
    /// recorded as soon as it is done, not after the whole group. Default:
    /// one after another.
    fn apply_group(&mut self, actions: Vec<ApprovedAction>) -> Vec<Option<ApplyResult>> {
        let mut results = Vec::with_capacity(actions.len());
        let mut stopped = false;
        for action in actions {
            if stopped {
                results.push(None);
                continue;
            }
            let result = self.apply(action);
            stopped = matches!(result, Err(EffectError::Stop(_)));
            results.push(Some(result));
        }
        results
    }
}

/// Whether actions may run at once: transfers and folder creations, which
/// depend on nothing but folders made before them. A group never has two
/// actions on the same path (compared as the engine compares names).
fn groupable(action: &Action) -> Option<(u8, usize)> {
    match action {
        Action::Upload { .. } | Action::UploadConflictCopy { .. } | Action::Download { .. } => {
            Some((0, 0))
        }
        // Folders of one depth at once; a deeper one waits for its parent.
        Action::CreateRemoteFolder { path } => Some((1, path.as_str().matches('/').count())),
        _ => None,
    }
}

/// Paths an action touches, for keeping a group free of clashes.
fn touched(action: &Action) -> Vec<String> {
    let paths: Vec<&ikeloud_safety::RelPath> = match action {
        Action::Upload { local, remote, .. } => vec![local, remote],
        Action::UploadConflictCopy { path, .. } | Action::CreateRemoteFolder { path } => vec![path],
        Action::Download { remote, local, .. } => vec![remote, local],
        Action::TrashRemote { path, .. } | Action::TrashLocal { path, .. } => vec![path],
        Action::MoveRemote { from, to, .. } | Action::MoveLocal { from, to, .. } => vec![from, to],
    };
    paths
        .into_iter()
        .map(|p| crate::names::key(p.as_str()))
        .collect()
}

/// Splits a plan into groups that keep its order: a run of consecutive
/// independent actions of one kind becomes one group, anything else is a
/// group of one.
pub fn independent(actions: &[ApprovedAction]) -> Vec<std::ops::Range<usize>> {
    let mut groups = Vec::new();
    let mut start = 0;
    while start < actions.len() {
        let mut end = start.saturating_add(1);
        let kind = actions.get(start).and_then(|a| groupable(a.action()));
        if kind.is_some() {
            let mut seen: std::collections::BTreeSet<String> = actions
                .get(start)
                .map(|a| touched(a.action()).into_iter().collect())
                .unwrap_or_default();
            while let Some(next) = actions.get(end) {
                let paths = touched(next.action());
                if groupable(next.action()) != kind || paths.iter().any(|p| seen.contains(p)) {
                    break;
                }
                seen.extend(paths);
                end = end.saturating_add(1);
            }
        }
        groups.push(start..end);
        start = end;
    }
    groups
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

/// Runs approved actions in order; independent ones may run at once (see
/// [`Effects::apply_group`]). An action that finished counts as done even
/// when another one of its group stopped the run.
pub fn execute(actions: Vec<ApprovedAction>, effects: &mut dyn Effects) -> Report {
    let mut report = Report::default();
    let groups = independent(&actions);
    let mut actions: std::collections::VecDeque<ApprovedAction> = actions.into();
    for range in groups {
        let group: Vec<ApprovedAction> = actions.drain(..range.len()).collect();
        let planned: Vec<Action> = group.iter().map(|a| a.action().clone()).collect();
        let results: Vec<Option<ApplyResult>> = if group.len() == 1 {
            group.into_iter().map(|a| Some(effects.apply(a))).collect()
        } else {
            effects.apply_group(group)
        };
        for (action, result) in planned.into_iter().zip(results) {
            match result {
                None => report.not_attempted.push(action),
                Some(Ok(transfer)) => {
                    report.transfers.extend(transfer);
                    report.done.push(action);
                }
                Some(Err(EffectError::Changed)) => report.skipped.push(action),
                Some(Err(EffectError::Stop(reason))) => {
                    report.skipped.push(action);
                    report.stopped.get_or_insert(reason);
                }
            }
        }
        if report.stopped.is_some() {
            report
                .not_attempted
                .extend(actions.iter().map(|a| a.action().clone()));
            break;
        }
    }
    report
}
