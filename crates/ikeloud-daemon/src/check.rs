//! The check run (decision 0028): does the content on the disk still match
//! what iCloud holds?
//!
//! For a file of the common state that neither side changed since (the
//! local file has the content hash of the common state, iCloud lists the
//! version of the common state), iCloud's checksum of the content must be
//! the checksum of the local file. A difference is something the sync
//! itself never sees: both sides look unchanged to it. Such a file is
//! reported once (journal, status); nothing is changed, on either side.
//!
//! The check reads files, so it goes in parts after quiet runs: new and
//! changed versions first, then the oldest results, each file again after
//! [`CheckLimits::every`].

use std::collections::BTreeSet;
use std::os::unix::fs::MetadataExt;
use std::time::{Duration, Instant, SystemTime};

use ikeloud_local::LocalDisk;
use ikeloud_safety::{ContentHash, RelPath};
use ikeloud_state::{JournalEntry, JournalKind, StateDb, StateError, Verified};
use ikeloud_sync::{BaseEntry, DocId};

use crate::effects::unix_secs;

/// How much one part of the check may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckLimits {
    /// A file is checked again after this long.
    pub every: Duration,
    /// One part stops after reading this many bytes…
    pub bytes: u64,
    /// …or after this long (the next sync waits for it).
    pub time: Duration,
}

impl CheckLimits {
    /// Every file once a week; a part reads at most 1 GiB or 10 seconds.
    pub const DEFAULT: Self = Self {
        every: Duration::from_secs(7 * 24 * 60 * 60),
        bytes: 1 << 30,
        time: Duration::from_secs(10),
    };
}

impl Default for CheckLimits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// What one part of the check found, and where the whole check stands.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CheckReport {
    /// Files read and compared in this part.
    pub checked: u64,
    /// Files found to differ in this part that were not reported before
    /// (each got a journal entry).
    pub found: Vec<RelPath>,
    /// Files that can be checked: iCloud's checksum of the version of the
    /// common state is known.
    pub checkable: u64,
    /// Of those, files with a result younger than [`CheckLimits::every`]
    /// for the version and content of the common state.
    pub current: u64,
    /// Of the current results, files whose content differs.
    pub differing: u64,
}

/// What must not change while a file is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    size: u64,
    mtime: (i64, i64),
    ctime: (i64, i64),
}

impl Stamp {
    fn of(meta: &std::fs::Metadata) -> Self {
        Self {
            size: meta.len(),
            mtime: (meta.mtime(), meta.mtime_nsec()),
            ctime: (meta.ctime(), meta.ctime_nsec()),
        }
    }
}

/// A file read: its content hash and iCloud's checksum of it.
struct Read {
    hash: ContentHash,
    checksum: String,
    size: u64,
}

/// Reads the local file of `entry`, if it is still that file and did not
/// change while being read. `None`: gone, replaced, unreadable or being
/// written; the sync deals with it, the check skips it.
fn read(disk: &LocalDisk, entry: &BaseEntry) -> Option<Read> {
    let mut file = disk.open(&entry.local_path, entry.local?).ok()?;
    let before = Stamp::of(&file.metadata().ok()?);
    let (sha256, checksum) = ikeloud_hash::content_sums(&mut file).ok()?;
    let after = Stamp::of(&file.metadata().ok()?);
    (before == after).then(|| Read {
        hash: ContentHash::new(ikeloud_hash::hex(&sha256)),
        checksum,
        size: after.size,
    })
}

/// A file due for checking, with iCloud's checksum and the time of its
/// last result for this version (`None`: never checked).
struct Due<'a> {
    entry: &'a BaseEntry,
    checksum: &'a str,
    last: Option<u64>,
}

/// One part of the check, see the module. Stores the results, forgets
/// those of documents gone from the common state, and writes a journal
/// entry for every newly found difference, all in one transaction. Errors
/// are errors of the state database only.
pub fn check_part(
    disk: &LocalDisk,
    db: &mut StateDb,
    now: SystemTime,
    limits: CheckLimits,
) -> Result<CheckReport, StateError> {
    let started = Instant::now();
    let now_secs = unix_secs(now);
    let every = limits.every.as_secs();
    let base = db.base()?;
    let sums = db.remote_checksums()?;
    let known = db.verified()?;

    let docs: BTreeSet<&DocId> = base.iter().map(|b| &b.doc_id).collect();
    let forget: Vec<DocId> = known
        .keys()
        .filter(|d| !docs.contains(d))
        .cloned()
        .collect();

    let mut report = CheckReport::default();
    let mut due = Vec::new();
    for entry in &base {
        let Some((etag, checksum)) = sums.get(&entry.doc_id) else {
            continue;
        };
        if *etag != entry.etag {
            continue;
        }
        report.checkable += 1;
        let last = known
            .get(&entry.doc_id)
            .filter(|v| v.etag == entry.etag && v.hash == entry.hash);
        match last {
            Some(v) if now_secs.saturating_sub(v.time) < every => {
                report.current += 1;
                report.differing += u64::from(!v.same);
            }
            _ => due.push(Due {
                entry,
                checksum,
                last: last.map(|v| v.time),
            }),
        }
    }
    // Never checked first (new and changed files), then the oldest.
    due.sort_by_key(|d| d.last.map_or((false, 0), |t| (true, t)));

    let mut results = Vec::new();
    let mut journal = Vec::new();
    let mut bytes = 0u64;
    for d in due {
        if crate::shutting_down() || bytes >= limits.bytes || started.elapsed() >= limits.time {
            break;
        }
        let Some(read) = read(disk, d.entry) else {
            continue;
        };
        bytes = bytes.saturating_add(read.size);
        // Changed locally since the common state: the sync carries it over.
        if read.hash != d.entry.hash {
            continue;
        }
        let same = read.checksum == d.checksum;
        report.checked += 1;
        report.current += 1;
        if !same {
            report.differing += 1;
            let reported = known
                .get(&d.entry.doc_id)
                .is_some_and(|v| !v.same && v.etag == d.entry.etag && v.hash == d.entry.hash);
            if !reported {
                journal.push(JournalEntry {
                    time: now_secs,
                    kind: JournalKind::CheckDiffers,
                    path: d.entry.path.clone(),
                    target: None,
                });
                report.found.push(d.entry.path.clone());
            }
        }
        results.push(Verified {
            doc_id: d.entry.doc_id.clone(),
            etag: d.entry.etag.clone(),
            hash: d.entry.hash.clone(),
            time: now_secs,
            same,
        });
    }
    if !results.is_empty() || !forget.is_empty() {
        db.store_check(&results, &forget, &journal)?;
    }
    Ok(report)
}
