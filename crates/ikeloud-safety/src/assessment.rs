//! Assessing a plan of actions: allow, defer, or stop and ask.

use std::collections::{BTreeMap, BTreeSet};

use crate::action::{Action, Etag, RelPath};
use crate::context::{Context, LocalFolderState, Mode, ReadOnlyReason};
use crate::names::{self, NameError};

/// An action the safety layer approved.
///
/// Only this crate can create it and it cannot be copied. Write functions in
/// `ikeloud-api` take it by value, so each approval is good for one write.
#[derive(Debug, PartialEq, Eq)]
pub struct ApprovedAction {
    action: Action,
}

impl ApprovedAction {
    /// The approved action.
    pub fn action(&self) -> &Action {
        &self.action
    }
}

/// Why an action is not carried out now. The engine retries in the next run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeferReason {
    /// The app is in read-only mode.
    ReadOnly(ReadOnlyReason),
    /// The file is still being written (unchanged for less than the settle time).
    StillBeingWritten,
    /// The name cannot be uploaded to the server.
    InvalidName(NameError),
    /// Temporary or lock file; never synced.
    Ignored,
    /// The server already has an item whose name differs only in letter case.
    CaseCollision {
        /// The colliding path.
        with: RelPath,
    },
}

/// A deferred action and the reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeferredAction {
    /// The action from the plan.
    pub action: Action,
    /// Why it is not carried out.
    pub reason: DeferReason,
}

/// The plan after approval.
#[derive(Debug, PartialEq, Eq)]
pub struct ApprovedPlan {
    /// Actions that may be carried out.
    pub actions: Vec<ApprovedAction>,
    /// Actions that are not carried out now.
    pub deferred: Vec<DeferredAction>,
}

/// Result of the assessment when sync may continue.
#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    /// The plan is empty.
    NothingToDo,
    /// The plan is approved (possibly with deferred actions).
    Approved(ApprovedPlan),
}

/// Overview of a mass change for asking the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MassChangeSummary {
    /// How many files would be deleted or overwritten.
    pub count: u64,
    /// How many files are synced.
    pub total: u64,
    fingerprint: u64,
}

impl MassChangeSummary {
    /// The user confirmed the change. The confirmation is valid only for
    /// exactly the same plan.
    pub fn confirm(&self) -> MassChangeConfirmation {
        MassChangeConfirmation {
            fingerprint: self.fingerprint,
        }
    }
}

/// Confirmation of a mass change, bound to one specific plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MassChangeConfirmation {
    fingerprint: u64,
}

/// Why the plan is not carried out at all. Sync stops.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejection {
    /// The local folder is missing or suddenly empty.
    LocalFolderUnavailable,
    /// The server returned an empty listing although there should be content.
    SuspiciousRemoteListing,
    /// Items changed on the server since the engine built the plan.
    Conflict(Vec<RelPath>),
    /// The plan would delete or overwrite too many files; waits for confirmation.
    MassChangeNeedsConfirmation(MassChangeSummary),
    /// Not enough space on iCloud.
    NotEnoughICloudSpace {
        /// Bytes needed.
        needed: u64,
        /// Bytes free.
        free: u64,
    },
    /// Not enough space on the disk.
    NotEnoughDiskSpace {
        /// Bytes needed.
        needed: u64,
        /// Bytes free.
        free: u64,
    },
}

/// Assesses a plan of actions.
///
/// Order of checks: surroundings (local folder, server listing), deferring
/// individual actions, version conflicts, mass-change brake, free space.
/// `context.remote_etags` must contain the fresh state of every path in the
/// plan and of every item in folders where the plan adds new names.
pub fn assess(plan: Vec<Action>, context: &Context) -> Result<Decision, Rejection> {
    check_surroundings(context)?;
    if plan.is_empty() {
        return Ok(Decision::NothingToDo);
    }

    let mut to_run = Vec::new();
    let mut deferred = Vec::new();
    let collisions = case_collisions(&plan, &context.remote_etags);
    for action in plan {
        match defer_reason(&action, context, &collisions) {
            Some(reason) => deferred.push(DeferredAction { action, reason }),
            None => to_run.push(action),
        }
    }

    check_conflicts(&to_run, &context.remote_etags)?;
    check_mass_change(&to_run, context)?;
    check_space(&to_run, context)?;

    Ok(Decision::Approved(ApprovedPlan {
        actions: to_run
            .into_iter()
            .map(|action| ApprovedAction { action })
            .collect(),
        deferred,
    }))
}

fn check_surroundings(context: &Context) -> Result<(), Rejection> {
    let should_have_content = context.synced_count > 0;
    match context.local_folder {
        LocalFolderState::Missing => return Err(Rejection::LocalFolderUnavailable),
        LocalFolderState::Available { file_count: 0 } if should_have_content => {
            return Err(Rejection::LocalFolderUnavailable);
        }
        LocalFolderState::Available { .. } => {}
    }
    if should_have_content && context.remote_item_count == 0 {
        return Err(Rejection::SuspiciousRemoteListing);
    }
    Ok(())
}

fn defer_reason(
    action: &Action,
    context: &Context,
    collisions: &BTreeMap<RelPath, RelPath>,
) -> Option<DeferReason> {
    if names::is_ignored(action.path().name()) {
        return Some(DeferReason::Ignored);
    }
    if !action.writes_remote() {
        return None;
    }
    if let Mode::ReadOnly(reason) = context.mode {
        return Some(DeferReason::ReadOnly(reason));
    }
    if let Err(error) = names::validate_path(action.path().as_str()) {
        return Some(DeferReason::InvalidName(error));
    }
    if let Some(with) = collisions.get(action.path()) {
        return Some(DeferReason::CaseCollision { with: with.clone() });
    }
    if let Action::Upload { stable_for, .. } = action
        && *stable_for < context.settle_time
    {
        return Some(DeferReason::StillBeingWritten);
    }
    None
}

/// New names on the server that differ from existing or other new names only
/// in letter case. Returns the new path and the path it collides with.
fn case_collisions(
    plan: &[Action],
    remote: &BTreeMap<RelPath, Etag>,
) -> BTreeMap<RelPath, RelPath> {
    let mut taken: BTreeMap<String, RelPath> = remote
        .keys()
        .map(|p| (names::case_insensitive_key(p.as_str()), p.clone()))
        .collect();
    let mut collisions = BTreeMap::new();
    for action in plan {
        let Some(new) = new_remote_path(action) else {
            continue;
        };
        let key = names::case_insensitive_key(new.as_str());
        match taken.get(&key) {
            Some(existing) if existing != new => {
                collisions.insert(new.clone(), existing.clone());
            }
            Some(_) => {}
            None => {
                taken.insert(key, new.clone());
            }
        }
    }
    collisions
}

fn new_remote_path(action: &Action) -> Option<&RelPath> {
    match action {
        Action::Upload {
            path,
            expected_etag: None,
            ..
        }
        | Action::UploadConflictCopy { path, .. }
        | Action::CreateRemoteFolder { path } => Some(path),
        Action::MoveRemote { to, .. } => Some(to),
        _ => None,
    }
}

fn check_conflicts(actions: &[Action], remote: &BTreeMap<RelPath, Etag>) -> Result<(), Rejection> {
    let mut conflicts = BTreeSet::new();
    for a in actions {
        let matches = match a {
            Action::Upload {
                path,
                expected_etag,
                ..
            } => remote.get(path) == expected_etag.as_ref(),
            Action::UploadConflictCopy { path, .. } | Action::CreateRemoteFolder { path } => {
                !remote.contains_key(path)
            }
            Action::TrashRemote {
                path,
                expected_etag,
            } => remote.get(path) == Some(expected_etag),
            Action::MoveRemote {
                from,
                to,
                expected_etag,
            } => remote.get(from) == Some(expected_etag) && !remote.contains_key(to),
            Action::Download { .. } | Action::TrashLocal { .. } | Action::MoveLocal { .. } => true,
        };
        if !matches {
            conflicts.insert(a.path().clone());
        }
    }
    if conflicts.is_empty() {
        Ok(())
    } else {
        Err(Rejection::Conflict(conflicts.into_iter().collect()))
    }
}

fn check_mass_change(actions: &[Action], context: &Context) -> Result<(), Rejection> {
    let count = actions.iter().filter(|a| a.deletes_or_overwrites()).count() as u64;
    let limits = context.limits;
    let total = context.synced_count;
    let above_absolute = count > limits.absolute;
    let above_share = count > limits.minimum
        && u128::from(count) * 100 > u128::from(limits.percent) * u128::from(total);
    if !(above_absolute || above_share) {
        return Ok(());
    }
    let fingerprint = plan_fingerprint(actions);
    if context
        .confirmation
        .as_ref()
        .is_some_and(|c| c.fingerprint == fingerprint)
    {
        return Ok(());
    }
    Err(Rejection::MassChangeNeedsConfirmation(MassChangeSummary {
        count,
        total,
        fingerprint,
    }))
}

fn check_space(actions: &[Action], context: &Context) -> Result<(), Rejection> {
    let to_remote = actions
        .iter()
        .map(Action::bytes_to_remote)
        .fold(0u64, u64::saturating_add);
    if let Some(free) = context.icloud_free
        && to_remote > free
    {
        return Err(Rejection::NotEnoughICloudSpace {
            needed: to_remote,
            free,
        });
    }
    let to_disk = actions
        .iter()
        .map(Action::bytes_to_disk)
        .fold(0u64, u64::saturating_add);
    if let Some(free) = context.disk_free
        && to_disk > free
    {
        return Err(Rejection::NotEnoughDiskSpace {
            needed: to_disk,
            free,
        });
    }
    Ok(())
}

/// Plan fingerprint (FNV-1a 64 over the debug form of the actions). It only
/// makes a confirmation valid for exactly the plan the user saw.
fn plan_fingerprint(actions: &[Action]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let text = format!("{actions:?}");
    text.bytes()
        .fold(OFFSET, |h, b| (h ^ u64::from(b)).wrapping_mul(PRIME))
}
