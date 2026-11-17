//! Starting and stopping the daemon.
//!
//! Installed (`scripts/install-user.sh`): the daemon is the systemd user
//! service `ikeloud.service`, and the app starts and stops it with
//! `systemctl --user`. The service runs without the app, with core dumps
//! off (decision 0016). From a build tree without the service: the app
//! starts the daemon as its child.
//!
//! Stopping is safe at any moment: the daemon writes its state
//! transactionally and downloads go to temporary files first (decisions
//! 0009, 0015, 0017).

use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};

use ikeloud_config::{Request, Status};

/// Exit code of the daemon when the sign-in expired.
pub const EXIT_SIGNED_OUT: i32 = 3;

/// Name of the systemd user service.
const SERVICE: &str = "ikeloud.service";

/// The daemon as the app sees it.
#[derive(Debug)]
pub struct DaemonProcess {
    /// The systemd user service is installed.
    service: bool,
    child: Option<Child>,
}

impl Default for DaemonProcess {
    fn default() -> Self {
        Self {
            service: service_installed(),
            child: None,
        }
    }
}

impl DaemonProcess {
    /// Starts the daemon unless one is running already (also one started by
    /// an earlier run of the app, or the service).
    pub fn start(&mut self, status: Option<&Status>) -> std::io::Result<()> {
        if self.service {
            // Also restarts a service that exited "signed out" or "not set
            // up" (no automatic restart for those).
            return systemctl(&["start", SERVICE]);
        }
        if self.running(status) {
            return Ok(());
        }
        self.child = Some(Command::new(binary()).spawn()?);
        Ok(())
    }

    /// Stops the daemon: it finishes the transfers under way and records
    /// them first (the service's ExecStop does the same).
    pub fn stop(&mut self, status: Option<&Status>, request: &std::path::Path) {
        if self.service {
            let _ = systemctl(&["stop", SERVICE]);
            return;
        }
        if let Some(mut child) = self.child.take() {
            let _ = Request::Stop.save(request);
            for _ in 0..120 {
                if matches!(child.try_wait(), Ok(Some(_))) {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
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

    /// If the daemon ended since the last call: how. For the service, only
    /// "signed out" is reported (from its last exit code), once.
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

/// Whether the systemd user service is installed for this user.
fn service_installed() -> bool {
    Command::new("systemctl")
        .args(["--user", "cat", SERVICE])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn systemctl(args: &[&str]) -> std::io::Result<()> {
    let status = Command::new("systemctl")
        .arg("--user")
        .args(args)
        .stdout(Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "systemctl --user {} failed",
            args.join(" ")
        )))
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
