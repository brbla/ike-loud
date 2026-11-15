//! Rules for file names and for files that are never synced.
//!
//! Comparing names (Unicode normalization and letter case) needs Unicode
//! tables this crate does not have: it uses the standard library only. The
//! engine's comparison is passed in [`crate::Context::name_key`], so there
//! is one definition of "the same name" (`ikeloud_sync::names::key`).

use crate::action::RelPath;

/// Why a name cannot be uploaded to the server. (Whether a path is well
/// formed at all is decided when it is made: [`RelPath::parse`].)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameError {
    /// The name contains a character iCloud does not accept (a control
    /// character such as a newline).
    ForbiddenChar(char),
    /// A path component is longer than 255 bytes.
    TooLong,
}

/// Longest allowed path component in bytes (UTF-8).
pub const MAX_NAME_LEN: usize = 255;

/// Checks that a path can be uploaded to the server.
///
/// The rules are deliberately strict. The exact server rules are found in
/// milestone 0 and added here with tests.
pub fn validate_path(path: &RelPath) -> Result<(), NameError> {
    for name in path.as_str().split('/') {
        if name.len() > MAX_NAME_LEN {
            return Err(NameError::TooLong);
        }
        if let Some(c) = name.chars().find(|c| c.is_control()) {
            return Err(NameError::ForbiddenChar(c));
        }
    }
    Ok(())
}

/// Is this a temporary or lock file that is never synced?
///
/// Spec: `.~lock.*#`, `*.swp`, `*.part`, `.DS_Store`.
pub fn is_ignored(name: &str) -> bool {
    (name.starts_with(".~lock.") && name.ends_with('#'))
        || name.ends_with(".swp")
        || name.ends_with(".part")
        || name == ".DS_Store"
}
