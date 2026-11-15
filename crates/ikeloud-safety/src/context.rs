//! What the safety layer knows about its surroundings when assessing a plan.

use std::collections::BTreeMap;
use std::time::Duration;

use crate::action::{Etag, RelPath};
use crate::assessment::MassChangeConfirmation;

/// State of the local synced folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalFolderState {
    /// The folder exists; number of files in it at the last scan.
    Available {
        /// Number of files.
        file_count: u64,
    },
    /// The folder is missing (unplugged drive, deleted directory).
    Missing,
}

/// Why the app only reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadOnlyReason {
    /// The user turned it on in settings.
    Settings,
    /// The first download is not finished and checked yet.
    FirstDownload,
    /// The API returned an unknown response.
    UnknownApiResponse,
    /// The remote kill switch put clients into read-only mode.
    RemoteKillSwitch,
}

/// Sync mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Two-way sync.
    Full,
    /// Nothing is written to the server.
    ReadOnly(ReadOnlyReason),
}

/// When the mass-change brake stops: above `absolute` changes, or above
/// `minimum` changes that are at the same time more than `percent` of the
/// synced files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Threshold {
    /// Above this count the brake always stops.
    pub absolute: u64,
    /// Above this count the brake stops when it is also more than `percent`.
    pub minimum: u64,
    /// Share of the synced files in percent.
    pub percent: u64,
}

impl Threshold {
    /// Is `count` changes out of `total` synced files above the threshold?
    pub fn exceeded_by(self, count: u64, total: u64) -> bool {
        count > self.absolute
            || (count > self.minimum
                && u128::from(count) * 100 > u128::from(self.percent) * u128::from(total))
    }
}

/// Thresholds of the mass-change brake, counted over the brake window
/// ([`BRAKE_WINDOW`]).
///
/// Spec: deleting or overwriting more than 50 files, or more than 10 files
/// and at the same time more than 10 % of the synced content. Moves lose
/// nothing and renaming a folder is routine, so they have their own, higher
/// threshold: it catches a runaway rename (a wrong pairing, a bug), not the
/// user's work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MassChangeLimits {
    /// Deleting and overwriting.
    pub destructive: Threshold,
    /// Moving and renaming.
    pub moves: Threshold,
}

impl Default for MassChangeLimits {
    fn default() -> Self {
        Self {
            destructive: Threshold {
                absolute: 50,
                minimum: 10,
                percent: 10,
            },
            moves: Threshold {
                absolute: 500,
                minimum: 50,
                percent: 50,
            },
        }
    }
}

/// Over how long the brake counts changes: a deletion spread over many runs
/// (a script deleting one file at a time) is as dangerous as one at once.
pub const BRAKE_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);

/// Changes already carried out in the brake window: the last
/// [`BRAKE_WINDOW`], but not before the user last confirmed a mass change
/// (the daemon counts them from the journal).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RecentChanges {
    /// Files deleted or overwritten.
    pub destructive: u64,
    /// Files moved or renamed.
    pub moves: u64,
}

/// How long a file must stay unchanged before it is uploaded.
pub const DEFAULT_SETTLE_TIME: Duration = Duration::from_secs(5);

/// Everything the assessment needs to know about the surroundings.
#[derive(Debug, Clone)]
pub struct Context {
    /// State of the local folder.
    pub local_folder: LocalFolderState,
    /// Number of items the last server listing returned.
    pub remote_item_count: u64,
    /// Number of files in the last common state.
    pub synced_count: u64,
    /// Current server versions from a fresh listing, by path.
    pub remote_etags: BTreeMap<RelPath, Etag>,
    /// Sync mode.
    pub mode: Mode,
    /// Brake thresholds.
    pub limits: MassChangeLimits,
    /// Changes already carried out in the brake window.
    pub recent: RecentChanges,
    /// The user's confirmation of a mass change, if any.
    pub confirmation: Option<MassChangeConfirmation>,
    /// Free space on iCloud in bytes, if known.
    pub icloud_free: Option<u64>,
    /// Free space on the local disk in bytes, if known.
    pub disk_free: Option<u64>,
    /// How long a file must be unchanged before upload.
    pub settle_time: Duration,
    /// When two names are the same name for iCloud: the engine's key (NFC
    /// and letter case, `ikeloud_sync::names::key`). iCloud does not tell
    /// upper and lower case apart, nor a name a Mac wrote decomposed (NFD)
    /// from the same name composed (NFC).
    pub name_key: fn(&str) -> String,
}

impl Context {
    /// Context for a folder and a server with the same number of files, full
    /// mode, default limits, unknown free space. `name_key` decides when two
    /// names are the same name (see [`Context::name_key`]).
    pub fn new(synced_count: u64, name_key: fn(&str) -> String) -> Self {
        Self {
            local_folder: LocalFolderState::Available {
                file_count: synced_count,
            },
            remote_item_count: synced_count,
            synced_count,
            remote_etags: BTreeMap::new(),
            mode: Mode::Full,
            limits: MassChangeLimits::default(),
            recent: RecentChanges::default(),
            confirmation: None,
            icloud_free: None,
            disk_free: None,
            settle_time: DEFAULT_SETTLE_TIME,
            name_key,
        }
    }
}

/// Proofs for every input, checked by Kani (`scripts/kani.sh`). Not a test
/// of the formula against itself: properties the brake must have whatever
/// the formula is.
#[cfg(kani)]
mod proofs {
    use super::{MassChangeLimits, Threshold};

    fn any_threshold() -> Threshold {
        Threshold {
            absolute: kani::any(),
            minimum: kani::any(),
            percent: kani::any(),
        }
    }

    /// One more change never releases the brake.
    #[kani::proof]
    fn more_changes_never_release_the_brake() {
        let t = any_threshold();
        let (count, total): (u64, u64) = (kani::any(), kani::any());
        kani::assume(count < u64::MAX);
        if t.exceeded_by(count, total) {
            assert!(t.exceeded_by(count + 1, total));
        }
    }

    /// The default limits: up to 10 deletions never stop, more than 50
    /// always do, whatever the number of synced files (spec).
    #[kani::proof]
    fn default_deletion_limits() {
        let t = MassChangeLimits::default().destructive;
        let (count, total): (u64, u64) = (kani::any(), kani::any());
        if count <= 10 {
            assert!(!t.exceeded_by(count, total));
        }
        if count > 50 {
            assert!(t.exceeded_by(count, total));
        }
    }
}
