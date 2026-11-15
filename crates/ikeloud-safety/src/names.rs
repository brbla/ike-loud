//! Rules for file names and for files that are never synced.
//!
//! Unicode normalization (NFC) is done by the engine before the plan comes
//! here; this crate uses the standard library only.

/// Why a name cannot be uploaded to the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameError {
    /// Empty path or empty path component (`a//b`, `/a`).
    EmptyComponent,
    /// A path component is `.` or `..`.
    DotOrDotDot,
    /// The name contains a character iCloud does not accept (NUL, control).
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
pub fn validate_path(path: &str) -> Result<(), NameError> {
    if path.is_empty() {
        return Err(NameError::EmptyComponent);
    }
    for component in path.split('/') {
        validate_name(component)?;
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<(), NameError> {
    if name.is_empty() {
        return Err(NameError::EmptyComponent);
    }
    if name == "." || name == ".." {
        return Err(NameError::DotOrDotDot);
    }
    if name.len() > MAX_NAME_LEN {
        return Err(NameError::TooLong);
    }
    if let Some(c) = name.chars().find(|c| c.is_control()) {
        return Err(NameError::ForbiddenChar(c));
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

/// Key for comparing names regardless of letter case.
///
/// iCloud does not distinguish upper and lower case, Linux does.
pub fn case_insensitive_key(name: &str) -> String {
    name.to_lowercase()
}
