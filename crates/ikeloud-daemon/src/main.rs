//! ike loud daemon: background sync.
//!
//! Runs through the Background portal in Flatpak and as a systemd user
//! service in native packages (packaging/systemd: core dumps off, so no
//! secret from the sign-in can end up on disk after a crash). Talks to the
//! GUI over D-Bus.

use std::process::ExitCode;

fn main() -> ExitCode {
    // The sync run exists (ikeloud_daemon::sync_once); the iCloud client it
    // needs comes with the test account (milestone 0).
    ExitCode::from(2)
}
