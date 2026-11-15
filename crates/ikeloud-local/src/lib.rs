//! The local side of ike loud.
//!
//! [`scan`] walks the synced folder and describes every regular file the way
//! the planner needs it: identity (device, inode, birth time), content hash,
//! size and how long it has been unchanged. The scan is the truth about the
//! local folder (spec: inotify is only a hint, a full scan decides).
//!
//! [`LocalDisk`] carries out the local actions of a plan, checking their
//! preconditions right before acting.
//!
//! Standard library only, no `unsafe`.

mod disk;
mod scan;

pub use disk::{DiskError, LocalDisk};
pub use scan::{Scan, ScanError, SkipReason, Skipped, scan};
