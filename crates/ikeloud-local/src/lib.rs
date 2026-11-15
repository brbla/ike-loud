//! The local side of ike loud.
//!
//! [`scan`] walks the synced folder and describes every regular file the way
//! the planner needs it: identity (device, inode, birth time), content hash,
//! size and how long it has been unchanged. The scan is the truth about the
//! local folder (spec: inotify is only a hint, a full scan decides);
//! [`scan_with`] reads only the files that changed since the last scan.
//!
//! [`LocalDisk`] carries out the local actions of a plan, checking their
//! preconditions right before acting.
//!
//! [`Watcher`] turns inotify events into hints to scan soon.
//!
//! No `unsafe`; the only library besides our own crates is `inotify`.

mod disk;
mod scan;
mod watch;

pub use disk::{DiskError, LocalDisk};
pub use scan::{HashCache, Scan, ScanError, SkipReason, Skipped, scan, scan_with};
pub use watch::{Coverage, Hint, Watcher, Watches};
