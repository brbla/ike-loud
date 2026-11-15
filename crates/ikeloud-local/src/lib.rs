//! The local side of ike loud.
//!
//! [`scan`] walks the synced folder and describes every regular file the way
//! the planner needs it: identity (device, inode, birth time), content hash,
//! size and how long it has been unchanged. The scan is the truth about the
//! local folder (spec: inotify is only a hint, a full scan decides).
//!
//! Standard library only, no `unsafe`.

mod scan;

pub use scan::{Scan, ScanError, SkipReason, Skipped, scan};
