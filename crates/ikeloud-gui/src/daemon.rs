//! Starting and stopping the daemon process.
//!
//! For now the app starts the daemon itself; a systemd user service (and the
//! Background portal in Flatpak) comes with packaging. Stopping is safe at
//! any moment: the daemon writes its state transactionally and downloads go
//! to temporary files first (decisions 0009, 0015, 0017).

use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus};

use ikeloud_config::Status;

/// Exit code of the daemon when the sign-in expired.
pub const EXIT_SIGNED_OUT: i32 = 3;

/// The daemon as the app sees it.
#[derive(Debug, Default)]
pub struct DaemonProcess {
    child: Option<Child>,
}

impl DaemonProcess {
    /// Starts the daemon unless one is running already (also one started by
    /// an earlier run of the app).
    pub fn start(&mut self, status: Option<&Status>) -> std::io::Result<()> {
        if self.running(status) {
            return Ok(());
        }
        self.child = Some(Command::new(binary()).spawn()?);
        Ok(())
    }

    /// Stops the daemon.
    pub fn stop(&mut self, status: Option<&Status>) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
            return;
        }
        // Started by an earlier run of the app: ask it to end.
        if let Some(pid) = status.map(|s| s.pid).filter(|pid| is_daemon(*pid)) {
            let _ = Command::new("kill")
                .arg("-TERM")
                .arg(pid.to_string())
                .status();
        }
    }

    /// Whether a daemon runs.
    pub fn running(&mut self, status: Option<&Status>) -> bool {
        if self.exited().is_some() {
            return false;
        }
        self.child.is_some() || status.is_some_and(|s| is_daemon(s.pid))
    }

    /// If our daemon ended since the last call: how.
    pub fn exited(&mut self) -> Option<ExitStatus> {
        let child = self.child.as_mut()?;
        match child.try_wait() {
            Ok(Some(status)) => {
                self.child = None;
                Some(status)
            }
            _ => None,
        }
    }
}

/// The daemon binary: next to the app's binary, else from `PATH`.
fn binary() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("ikeloud-daemon")))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from("ikeloud-daemon"))
}

/// Whether `pid` is a running ike loud daemon (not a reused process id).
fn is_daemon(pid: u32) -> bool {
    pid != 0
        && std::fs::read_to_string(format!("/proc/{pid}/comm"))
            .is_ok_and(|comm| comm.trim_end() == "ikeloud-daemon")
}
