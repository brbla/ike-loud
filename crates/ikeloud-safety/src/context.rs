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

/// Thresholds of the mass-change brake.
///
/// Spec: more than 50 files, or more than 10 files and at the same time more
/// than 10 % of the synced content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MassChangeLimits {
    /// Above this count the brake always stops.
    pub absolute: u64,
    /// Above this count the brake stops when it is also more than `percent`.
    pub minimum: u64,
    /// Share of the synced content in percent.
    pub percent: u64,
}

impl Default for MassChangeLimits {
    fn default() -> Self {
        Self {
            absolute: 50,
            minimum: 10,
            percent: 10,
        }
    }
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
    /// The user's confirmation of a mass change, if any.
    pub confirmation: Option<MassChangeConfirmation>,
    /// Free space on iCloud in bytes, if known.
    pub icloud_free: Option<u64>,
    /// Free space on the local disk in bytes, if known.
    pub disk_free: Option<u64>,
    /// How long a file must be unchanged before upload.
    pub settle_time: Duration,
}

impl Context {
    /// Context for a folder and a server with the same number of files, full
    /// mode, default limits, unknown free space.
    pub fn new(synced_count: u64) -> Self {
        Self {
            local_folder: LocalFolderState::Available {
                file_count: synced_count,
            },
            remote_item_count: synced_count,
            synced_count,
            remote_etags: BTreeMap::new(),
            mode: Mode::Full,
            limits: MassChangeLimits::default(),
            confirmation: None,
            icloud_free: None,
            disk_free: None,
            settle_time: DEFAULT_SETTLE_TIME,
        }
    }
}
