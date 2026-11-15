//! Deterministic simulation of whole sync runs: the daemon's real run
//! (scanner, planner, safety layer, executor, local actions, state
//! database) on real files against the fake iCloud, with a user working on
//! both sides — also during a run, between server writes — and faults: the
//! server going offline in the middle of a run, and the daemon crashing
//! after its actions but before it stored the new common state.
//!
//! Checks: no content is ever lost by sync (it stays in a folder, a trash,
//! the local history or on iCloud; only what the user destroyed may go),
//! and once the user stops, both sides become equal and stay so.
//!
//! Reproducible from the seed; `SIM_SEEDS` sets the number (default 200).

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stderr
)] // test code

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use common::{FakeICloud, copy_name};
use ikeloud_daemon::{
    Download, Listing, Remote, RemoteError, RunOutcome, Started, SyncSetup, Written, start_run,
};
use ikeloud_local::{HashCache, LocalDisk};
use ikeloud_safety::{ApprovedAction, DEFAULT_SETTLE_TIME, Rejection, RelPath};
use ikeloud_state::StateDb;
use ikeloud_sync::StopReason;

struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        let x = self.0.wrapping_mul(0x2545_F491_4F6C_DD1D);
        usize::try_from(x % u64::try_from(n).expect("small")).expect("small")
    }

    fn pick<T: Clone>(&mut self, items: &[T]) -> Option<T> {
        (!items.is_empty()).then(|| items[self.below(items.len())].clone())
    }
}

const NAMES: &[&str] = &["a.txt", "b.txt", "c.odt", "d/e.txt", "d/f.txt", "g/h/i.txt"];

/// The user: works on the local folder and on iCloud, and knows which
/// contents they destroyed themselves (edited over, deleted for good).
struct User {
    rng: Rng,
    root: PathBuf,
    next: u64,
    destroyed: BTreeSet<Vec<u8>>,
    /// Contents the user created (also during a run).
    created: BTreeSet<Vec<u8>>,
    /// Contents the user deleted on the iPhone (in Recently Deleted).
    trashed: BTreeSet<Vec<u8>>,
    /// Contents seen on both sides at once.
    synced: BTreeSet<Vec<u8>>,
    /// Acts between server writes during a run.
    interleave: bool,
    steps_during_runs: usize,
    targeted_edits: usize,
    trace: Vec<String>,
}

impl User {
    fn content(&mut self) -> Vec<u8> {
        self.next += 1;
        let c = format!("content {}", self.next).into_bytes();
        self.created.insert(c.clone());
        c
    }

    fn local_files(&self) -> Vec<String> {
        files_under(&self.root)
    }

    /// Notes every content that is on both sides right now.
    fn note_synced(&mut self, icloud: &FakeICloud) {
        let remote: BTreeSet<&Vec<u8>> = icloud.files.values().map(|f| &f.content).collect();
        for p in self.local_files() {
            if let Ok(c) = fs::read(self.root.join(p))
                && remote.contains(&c)
            {
                self.synced.insert(c);
            }
        }
    }

    fn step(&mut self, icloud: &mut FakeICloud) {
        self.step_inner(icloud);
        let show = |c: &[u8]| String::from_utf8_lossy(c).replace("content ", "#");
        let local: Vec<String> = self
            .local_files()
            .into_iter()
            .map(|p| {
                let c = fs::read(self.root.join(&p)).unwrap_or_default();
                format!("{p}={}", show(&c))
            })
            .collect();
        let remote: Vec<String> = icloud
            .files
            .iter()
            .map(|(p, f)| format!("{p}={}(doc {})", show(&f.content), f.doc))
            .collect();
        self.trace
            .push(format!("user: local {local:?} icloud {remote:?}"));
    }

    fn step_inner(&mut self, icloud: &mut FakeICloud) {
        let local = self.local_files();
        let remote: Vec<String> = icloud.files.keys().cloned().collect();
        let name = (*self.rng.pick(NAMES).expect("names")).to_owned();
        let target = self.root.join(&name);
        match self.rng.below(10) {
            0 | 1 => {
                if let Some(p) = self.rng.pick(&local) {
                    let old = fs::read(self.root.join(&p)).expect("read");
                    let new = self.content();
                    fs::write(self.root.join(&p), new).expect("edit");
                    self.destroyed.insert(old);
                }
            }
            2 => {
                // An editor: a new file renamed over the old one.
                if let Some(p) = self.rng.pick(&local) {
                    let path = self.root.join(&p);
                    let old = fs::read(&path).expect("read");
                    let tmp = path.with_extension("tmp-save");
                    let new = self.content();
                    fs::write(&tmp, new).expect("write");
                    fs::rename(&tmp, &path).expect("rename over");
                    self.destroyed.insert(old);
                }
            }
            3 => {
                if !target.exists() && !blocked_by_file(&self.root, &name) {
                    fs::create_dir_all(target.parent().expect("parent")).expect("folders");
                    let new = self.content();
                    fs::write(&target, new).expect("create");
                }
            }
            4 if local.len() > 1 => {
                if let Some(p) = self.rng.pick(&local) {
                    let old = fs::read(self.root.join(&p)).expect("read");
                    fs::remove_file(self.root.join(&p)).expect("rm");
                    self.destroyed.insert(old);
                }
            }
            5 => {
                if let Some(p) = self.rng.pick(&local)
                    && !target.exists()
                    && !blocked_by_file(&self.root, &name)
                {
                    fs::create_dir_all(target.parent().expect("parent")).expect("folders");
                    fs::rename(self.root.join(&p), &target).expect("mv");
                }
            }
            6 => {
                if let Some(p) = self.rng.pick(&remote) {
                    let old = icloud.files[&p].content.clone();
                    let new = self.content();
                    icloud.edit(&p, &new);
                    self.destroyed.insert(old); // no version history on iCloud
                }
            }
            7 => {
                if icloud.name_free(&name)
                    && !icloud
                        .files
                        .keys()
                        .any(|k| name.starts_with(&format!("{k}/")))
                {
                    let new = self.content();
                    icloud.put(&name, &new);
                }
            }
            8 if remote.len() > 1 => {
                if let Some(p) = self.rng.pick(&remote) {
                    self.trashed.insert(icloud.files[&p].content.clone());
                    icloud.delete(&p); // to Recently Deleted
                }
            }
            _ => {
                if let Some(p) = self.rng.pick(&remote)
                    && icloud.name_free(&name)
                    && !icloud
                        .files
                        .keys()
                        .any(|k| name.starts_with(&format!("{k}/")))
                {
                    let f = icloud.files.remove(&p).expect("exists");
                    icloud.put(&name, b"");
                    let doc_slot = icloud.files.get_mut(&name).expect("put");
                    *doc_slot = f; // same document, new path
                }
            }
        }
    }
}

/// A path whose folder would have to be a file that exists (`a.txt/x`).
fn blocked_by_file(root: &Path, rel: &str) -> bool {
    Path::new(rel)
        .ancestors()
        .skip(1)
        .any(|a| !a.as_os_str().is_empty() && root.join(a).is_file())
}

fn files_under(base: &Path) -> Vec<String> {
    fn walk(dir: &Path, base: &Path, out: &mut Vec<String>) {
        for e in fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, base, out);
            } else {
                out.push(p.strip_prefix(base).expect("inside").display().to_string());
            }
        }
    }
    let mut out = Vec::new();
    walk(base, base, &mut out);
    out.sort();
    out
}

/// The fake iCloud as the daemon sees it; the user may act before each
/// server write.
struct Server<'a> {
    icloud: &'a mut FakeICloud,
    user: &'a mut User,
}

impl Remote for Server<'_> {
    fn list(&mut self) -> Result<Listing, StopReason> {
        self.icloud.list()
    }

    fn download(&mut self, path: &RelPath) -> Result<Download, RemoteError> {
        self.meanwhile();
        // The user is working on the very file being downloaded over.
        let local = self.user.root.join(path.as_str());
        if self.user.interleave && local.is_file() && self.user.rng.below(3) == 0 {
            let old = fs::read(&local).expect("read");
            let new = self.user.content();
            fs::write(&local, new).expect("edit");
            self.user.destroyed.insert(old);
            self.user.targeted_edits += 1;
            self.user
                .trace
                .push(format!("user edits {path} while it is downloaded"));
        }
        self.icloud.download(path)
    }

    fn write(
        &mut self,
        action: ApprovedAction,
        content: Option<&mut dyn Read>,
    ) -> Result<Option<Written>, RemoteError> {
        self.meanwhile();
        self.icloud.write(action, content)
    }
}

impl Server<'_> {
    /// The user may act right before a server operation of a run.
    fn meanwhile(&mut self) {
        self.user.note_synced(self.icloud);
        if self.user.interleave && self.user.rng.below(2) == 0 {
            self.user.steps_during_runs += 1;
            self.user.step(self.icloud);
        }
    }
}

struct World {
    dir: PathBuf,
    db: StateDb,
    /// Hashes kept across runs, as the daemon keeps them.
    hashes: HashCache,
    icloud: FakeICloud,
    user: User,
    stats: Stats,
}

#[derive(Debug, Default, Clone, Copy)]
struct Stats {
    runs: usize,
    outages: usize,
    crashes: usize,
    confirmations: usize,
    actions: usize,
    safe_stops: usize,
}

impl World {
    fn new(seed: u64) -> Self {
        let dir =
            std::env::temp_dir().join(format!("ikeloud-daemon-sim-{}-{seed}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        for sub in ["root", "trash", "history"] {
            fs::create_dir_all(dir.join(sub)).expect("folders");
        }
        let db = StateDb::open(&dir.join("state.db")).expect("db");
        Self {
            user: User {
                rng: Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1),
                root: dir.join("root"),
                next: 0,
                destroyed: BTreeSet::new(),
                created: BTreeSet::new(),
                trashed: BTreeSet::new(),
                synced: BTreeSet::new(),
                interleave: true,
                steps_during_runs: 0,
                targeted_edits: 0,
                trace: vec![],
            },
            dir,
            db,
            hashes: HashCache::default(),
            icloud: FakeICloud::default(),
            stats: Stats::default(),
        }
    }

    /// Contents in a trash or the local history (Recently Deleted counts
    /// only what sync put there, not the user's own deletions).
    fn set_aside(&self) -> BTreeSet<Vec<u8>> {
        let mut all = BTreeSet::new();
        for sub in ["trash/files", "history"] {
            let base = self.dir.join(sub);
            for f in files_under(&base) {
                all.insert(fs::read(base.join(f)).expect("read"));
            }
        }
        all.extend(
            self.icloud
                .trash
                .iter()
                .map(|(_, c)| c.clone())
                .filter(|c| !self.user.trashed.contains(c)),
        );
        all
    }

    /// Every content kept anywhere: both sides, both trashes, the history.
    fn alive(&self) -> BTreeSet<Vec<u8>> {
        let mut all = BTreeSet::new();
        for sub in ["root", "trash/files", "history"] {
            let base = self.dir.join(sub);
            for f in files_under(&base) {
                all.insert(fs::read(base.join(f)).expect("read"));
            }
        }
        all.extend(self.icloud.files.values().map(|f| f.content.clone()));
        all.extend(self.icloud.trash.iter().map(|(_, c)| c.clone()));
        all
    }

    /// One sync run, maybe with an outage, and maybe crashing after its
    /// actions but before storing the new common state (only the first
    /// part of the run happens). `None`: the safety layer stopped the sync
    /// for good (an empty side that should have content), a safe end of the
    /// scenario.
    fn run(&mut self, faults: bool) -> Option<usize> {
        self.stats.runs += 1;
        let outage = faults && self.user.rng.below(6) == 0;
        let crash = faults && self.user.rng.below(6) == 0;
        if outage {
            self.icloud.offline_after = Some(self.user.rng.below(3));
            self.stats.outages += 1;
        }
        let setup = SyncSetup {
            disk: LocalDisk::new(
                self.dir.join("root"),
                self.dir.join("trash"),
                self.dir.join("history"),
            ),
            settle_time: DEFAULT_SETTLE_TIME,
            conflict_name: &copy_name,
        };
        let now = SystemTime::now() + Duration::from_secs(60);
        let mut server = Server {
            icloud: &mut self.icloud,
            user: &mut self.user,
        };
        let db = &mut self.db;
        let hashes = &mut self.hashes;
        // `None`: the daemon crashed after the first part of the run.
        let mut one = |confirmation| -> Option<RunOutcome> {
            match start_run(&setup, &mut server, db, hashes, confirmation, now).expect("state db") {
                Started::Ended(outcome) => Some(outcome),
                // The daemon dies here; what it recorded stays.
                Started::Executed(_) if crash => None,
                Started::Executed(run) => Some(
                    run.finish(&setup, &mut server, db, hashes)
                        .expect("state db"),
                ),
            }
        };
        let mut outcome = one(None);
        if let Some(RunOutcome::Rejected(Rejection::MassChangeNeedsConfirmation(s))) = &outcome {
            // The user confirms.
            self.stats.confirmations += 1;
            outcome = one(Some(s.confirm()));
        }
        self.user.trace.push(format!(
            "run (outage {outage}, crash {crash}): {:?}",
            outcome.as_ref().map(|o| match o {
                RunOutcome::Ran(r) => format!(
                    "done {:?} skipped {:?} stopped {:?}",
                    r.executed.done, r.executed.skipped, r.executed.stopped
                ),
                other => format!("{other:?}"),
            })
        ));
        let done = match outcome {
            // A crashed run did something, so the scenario goes on; the
            // convergence check waits for a run that finished.
            None => Some(1),
            Some(RunOutcome::Ran(r)) => Some(r.executed.done.len() + r.executed.skipped.len()),
            Some(RunOutcome::RemoteUnavailable(_)) => Some(0),
            Some(RunOutcome::Rejected(
                Rejection::LocalFolderUnavailable | Rejection::SuspiciousRemoteListing,
            )) => None,
            Some(other) => panic!("unexpected outcome {other:?}"),
        };
        self.stats.actions += done.unwrap_or(0);
        if done.is_none() {
            self.stats.safe_stops += 1;
        }
        if crash {
            self.stats.crashes += 1;
        }
        self.icloud.offline_after = None;
        done
    }

    fn checked_run(&mut self, faults: bool) -> Result<Option<usize>, String> {
        self.user.created.clear();
        self.user.note_synced(&self.icloud);
        let mut before = self.alive();
        let aside_before = self.set_aside();
        let done = self.run(faults);
        self.user.note_synced(&self.icloud);
        // Sync may set aside (trash, history) only what both sides had: a
        // change the other side never saw must stay a live file.
        let unsynced: Vec<String> = self
            .set_aside()
            .difference(&aside_before)
            .filter(|c| !self.user.synced.contains(*c))
            .map(|c| String::from_utf8_lossy(c).into_owned())
            .collect();
        if !unsynced.is_empty() {
            return Err(format!(
                "sync set aside content the other side never had: {unsynced:?}"
            ));
        }
        // Also what the user created while the run went on.
        before.extend(self.user.created.iter().cloned());
        let after = self.alive();
        let lost: Vec<String> = before
            .difference(&after)
            .filter(|c| !self.user.destroyed.contains(*c))
            .map(|c| String::from_utf8_lossy(c).into_owned())
            .collect();
        if lost.is_empty() {
            Ok(done)
        } else {
            Err(format!("content lost by sync: {lost:?}"))
        }
    }

    fn in_sync(&self) -> Result<(), String> {
        let local: BTreeMap<String, Vec<u8>> = files_under(&self.dir.join("root"))
            .into_iter()
            .map(|p| {
                let c = fs::read(self.dir.join("root").join(&p)).expect("read");
                (p, c)
            })
            .collect();
        let remote: BTreeMap<String, Vec<u8>> = self
            .icloud
            .files
            .iter()
            .map(|(p, f)| (p.clone(), f.content.clone()))
            .collect();
        // Every content is created once by the user and only copied to the
        // other side by sync: the same content twice on one side is a copy
        // sync made for nothing (e.g. a conflict copy of an unchanged file).
        for (side, files) in [("local", &local), ("iCloud", &remote)] {
            let mut seen = BTreeSet::new();
            for (path, content) in files.iter() {
                if !seen.insert(content) {
                    return Err(format!("{side}: {path} duplicates another file: {files:?}"));
                }
            }
        }
        if local == remote {
            Ok(())
        } else {
            Err(format!(
                "did not converge:\n local  {:?}\n remote {:?}",
                local.keys(),
                remote.keys()
            ))
        }
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn simulate(world: &mut World) -> Result<(), String> {
    let steps = 10 + world.user.rng.below(20);
    for _ in 0..steps {
        if world.user.rng.below(3) == 0 {
            if world.checked_run(true)?.is_none() {
                return Ok(());
            }
        } else {
            world.user.step(&mut world.icloud);
        }
    }
    // The user stops; no more faults.
    world.user.interleave = false;
    for _ in 0..8 {
        match world.checked_run(false)? {
            None => return Ok(()),
            Some(0) => {
                world.in_sync()?;
                if world.checked_run(false)? != Some(0) {
                    return Err("converged, but the next run did something".into());
                }
                return Ok(());
            }
            Some(_) => {}
        }
    }
    Err(format!(
        "did not settle in 8 runs; {}",
        world.in_sync().err().unwrap_or_default()
    ))
}

fn seeds() -> u64 {
    std::env::var("SIM_SEEDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(200)
}

#[test]
fn daemon_simulation() {
    let mut total = Stats::default();
    let mut during = 0;
    let mut targeted = 0;
    for seed in 0..seeds() {
        let mut world = World::new(seed);
        if let Err(e) = simulate(&mut world) {
            eprintln!("{}", world.user.trace.join("\n"));
            panic!(
                "seed {seed}: {e}\nreproduce: SIM_SEEDS={} cargo test -p ikeloud-daemon --test simulation",
                seed + 1
            );
        }
        total.runs += world.stats.runs;
        total.outages += world.stats.outages;
        total.crashes += world.stats.crashes;
        total.confirmations += world.stats.confirmations;
        total.actions += world.stats.actions;
        total.safe_stops += world.stats.safe_stops;
        during += world.user.steps_during_runs;
        targeted += world.user.targeted_edits;
    }
    eprintln!(
        "{} seeds: {total:?}, user steps during runs: {during}, \
         edits of a file being downloaded: {targeted}",
        seeds()
    );
    let n = usize::try_from(seeds()).expect("small");
    assert!(total.actions > n * 5, "the runs must do something");
    assert!(
        total.outages > n / 4 && total.crashes > n / 4,
        "faults happen"
    );
    assert!(during > n / 4, "the user acts during runs");
    assert!(
        targeted > n / 40,
        "the user edits files while they are downloaded"
    );
    assert!(
        total.safe_stops * 10 < n,
        "too many scenarios end in a safe stop"
    );
}
