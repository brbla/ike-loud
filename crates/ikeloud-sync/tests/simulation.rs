//! Deterministic simulation: the real planner and safety layer against a
//! simulated world (local disk, server) with random user actions on both
//! sides. Checks the properties of the TLA+ models on the real code:
//!
//! - no loss: a sync step never destroys content that exists nowhere else
//!   (on either side, in a trash, in history or in a conflict copy);
//! - unique names on the server;
//! - convergence: once the user stops, both sides become equal (except
//!   names in a collision, which sync deliberately leaves alone).
//!
//! Every run is reproducible from its seed. Failures print the seed and the
//! trace. `SIM_SEEDS` sets the number of seeds (default 500).
//!
//! The executor here is an oracle for tests, not the product's executor: it
//! checks each action's preconditions like the real one must (enforced etag,
//! free targets) and skips the action if they do not hold.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stderr
)] // test code

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use ikeloud_safety::{
    Action, ApprovedAction, Context, Decision, Etag, LocalFolderState, Rejection, RelPath, assess,
};
use ikeloud_sync::names::key;
use ikeloud_sync::{
    BaseEntry, ContentHash, DocId, EffectError, Effects, LocalFile, RemoteFile, States, Transfer,
    build_plan, execute, next_base,
};

// --- deterministic randomness ----------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> Option<&'a T> {
        if items.is_empty() {
            None
        } else {
            Some(&items[self.below(items.len())])
        }
    }
}

// --- the simulated world -----------------------------------------------------------------

#[derive(Debug, Clone)]
struct LFile {
    inode: u64,
    content: u64,
}

#[derive(Debug, Clone)]
struct RFile {
    doc: u64,
    etag: u64,
    content: u64,
}

/// Names the user picks from; includes letter-case variants and a folder.
const NAMES: &[&str] = &["a", "b", "c", "A", "d/e", "d/f", "Report.odt"];

#[derive(Debug, Default)]
struct World {
    local: BTreeMap<String, LFile>,
    remote: BTreeMap<String, RFile>,
    base: Vec<BaseEntry>,
    /// Content set aside: trashes, local history, conflict copies' sources.
    kept: BTreeSet<u64>,
    next: u64,
    /// Does a rename on the server change the etag? (unknown, see 0010)
    etag_changes_on_rename: bool,
    trace: Vec<String>,
}

impl World {
    fn fresh(&mut self) -> u64 {
        self.next += 1;
        self.next
    }

    fn remote_key_free(&self, path: &str, except_doc: Option<u64>) -> bool {
        let k = key(path);
        !self
            .remote
            .iter()
            .any(|(p, f)| key(p) == k && Some(f.doc) != except_doc)
    }

    fn alive(&self) -> BTreeSet<u64> {
        let mut all: BTreeSet<u64> = self.local.values().map(|f| f.content).collect();
        all.extend(self.remote.values().map(|f| f.content));
        all.extend(&self.kept);
        all
    }

    fn states(&self) -> States {
        States {
            local: self
                .local
                .iter()
                .map(|(p, f)| LocalFile {
                    path: RelPath::new(p.as_str()),
                    inode: f.inode,
                    hash: ContentHash::new(f.content.to_string()),
                    size: 1,
                    stable_for: Duration::from_secs(60),
                })
                .collect(),
            remote: self
                .remote
                .iter()
                .map(|(p, f)| RemoteFile {
                    path: RelPath::new(p.as_str()),
                    doc_id: DocId::new(f.doc.to_string()),
                    etag: Etag::new(f.etag.to_string()),
                    size: 1,
                })
                .collect(),
            remote_folders: vec![],
            base: self.base.clone(),
        }
    }

    // --- user actions ---------------------------------------------------------------

    fn user_step(&mut self, rng: &mut Rng) {
        let local_paths: Vec<String> = self.local.keys().cloned().collect();
        let remote_paths: Vec<String> = self.remote.keys().cloned().collect();
        let name = (*rng.pick(NAMES).expect("names")).to_string();
        match rng.below(11) {
            0 | 1 => {
                if let Some(p) = rng.pick(&local_paths) {
                    let c = self.fresh();
                    if let Some(f) = self.local.get_mut(p) {
                        f.content = c;
                    }
                    self.trace.push(format!("local edit {p} -> {c}"));
                }
            }
            2 => {
                // An editor that saves by writing a new file and renaming it over.
                if let Some(p) = rng.pick(&local_paths) {
                    let (c, i) = (self.fresh(), self.fresh());
                    self.local.insert(
                        p.clone(),
                        LFile {
                            inode: i,
                            content: c,
                        },
                    );
                    self.trace
                        .push(format!("local save-as-new-inode {p} -> {c}"));
                }
            }
            3 => {
                if !self.local.contains_key(&name) {
                    let (c, i) = (self.fresh(), self.fresh());
                    self.local.insert(
                        name.clone(),
                        LFile {
                            inode: i,
                            content: c,
                        },
                    );
                    self.trace.push(format!("local create {name} = {c}"));
                }
            }
            // Never delete the last file of a side: an empty side stops the
            // sync by design (tested in ikeloud-safety), which ends the run.
            4 if local_paths.len() > 1 => {
                if let Some(p) = rng.pick(&local_paths) {
                    self.local.remove(p);
                    self.trace.push(format!("local delete {p}"));
                }
            }
            5 => {
                if let Some(p) = rng.pick(&local_paths)
                    && !self.local.contains_key(&name)
                    && let Some(f) = self.local.remove(p)
                {
                    self.local.insert(name.clone(), f);
                    self.trace.push(format!("local move {p} -> {name}"));
                }
            }
            6 | 7 => {
                if let Some(p) = rng.pick(&remote_paths) {
                    let (c, e) = (self.fresh(), self.fresh());
                    if let Some(f) = self.remote.get_mut(p) {
                        f.content = c;
                        f.etag = e;
                    }
                    self.trace.push(format!("remote edit {p} -> {c}"));
                }
            }
            8 => {
                if self.remote_key_free(&name, None) {
                    let (c, d, e) = (self.fresh(), self.fresh(), self.fresh());
                    self.remote.insert(
                        name.clone(),
                        RFile {
                            doc: d,
                            etag: e,
                            content: c,
                        },
                    );
                    self.trace.push(format!("remote create {name} = {c}"));
                }
            }
            9 if remote_paths.len() > 1 => {
                if let Some(p) = rng.pick(&remote_paths)
                    && let Some(f) = self.remote.remove(p)
                {
                    // Deleting on the iPhone goes to "Recently Deleted".
                    self.kept.insert(f.content);
                    self.trace.push(format!("remote delete {p}"));
                }
            }
            _ => {
                if let Some(p) = rng.pick(&remote_paths) {
                    let doc = self.remote[p].doc;
                    if p != &name && self.remote_key_free(&name, Some(doc)) {
                        let mut f = self.remote.remove(p).expect("exists");
                        if self.etag_changes_on_rename {
                            f.etag = self.fresh();
                        }
                        self.remote.insert(name.clone(), f);
                        self.trace.push(format!("remote move {p} -> {name}"));
                    }
                }
            }
        }
    }

    // --- one sync run ----------------------------------------------------------------

    /// Returns false if the safety layer stopped the sync (a safe outcome).
    fn sync_step(&mut self) -> bool {
        let states = self.states();
        let plan = build_plan(&states, &|p: &RelPath, n: u32| {
            RelPath::new(format!("{} (conflict {n})", p.as_str()))
        });

        let mut context = Context::new(self.base.len() as u64);
        context.local_folder = LocalFolderState::Available {
            file_count: self.local.len() as u64,
        };
        context.remote_item_count = self.remote.len() as u64;
        context.remote_etags = self
            .remote
            .iter()
            .map(|(p, f)| (RelPath::new(p.as_str()), Etag::new(f.etag.to_string())))
            .collect();
        let decision = match assess(plan.actions.clone(), &context) {
            Err(Rejection::MassChangeNeedsConfirmation(summary)) => {
                // The user confirms.
                context.confirmation = Some(summary.confirm());
                assess(plan.actions.clone(), &context)
            }
            other => other,
        };
        let approved = match decision {
            Ok(Decision::NothingToDo) => vec![],
            Ok(Decision::Approved(p)) => p.actions,
            Err(Rejection::LocalFolderUnavailable | Rejection::SuspiciousRemoteListing) => {
                self.trace.push("sync stopped by safety".into());
                return false;
            }
            Err(other) => panic!("unexpected rejection {other:?}"),
        };
        // The product's executor and common-state update, not a test oracle.
        let old_base = self.base.clone();
        let report = execute(approved, self);
        self.base = next_base(&old_base, &self.states(), &report.transfers);
        let show = |m: Vec<(String, u64)>| {
            m.into_iter()
                .map(|(p, c)| format!("{p}={c}"))
                .collect::<Vec<_>>()
                .join(" ")
        };
        let local = show(
            self.local
                .iter()
                .map(|(p, f)| (p.clone(), f.content))
                .collect(),
        );
        let remote = show(
            self.remote
                .iter()
                .map(|(p, f)| (p.clone(), f.content))
                .collect(),
        );
        self.trace
            .push(format!("  sync done: local [{local}] remote [{remote}]"));
        true
    }

    /// Carries out one action like the real effects must: check the
    /// preconditions right before acting (server writes conditional on the
    /// etag, local writes never overwrite unexpectedly), keep replaced
    /// content in history, and report what each transfer produced.
    fn perform(&mut self, action: &Action) -> Result<Option<Transfer>, EffectError> {
        let changed = Err(EffectError::Changed);
        match action {
            Action::CreateRemoteFolder { .. } => Ok(None),
            Action::Upload {
                local,
                remote,
                expected_etag,
                ..
            } => {
                let Some(l) = self.local.get(local.as_str()).cloned() else {
                    return changed;
                };
                let doc = match (expected_etag, self.remote.get(remote.as_str())) {
                    (Some(e), Some(r)) if e.as_str() == r.etag.to_string() => {
                        let (doc, old) = (r.doc, r.content);
                        self.kept.insert(old); // local history of the overwritten version
                        doc
                    }
                    (None, None) if self.remote_key_free(remote.as_str(), None) => self.fresh(),
                    _ => return changed,
                };
                let etag = self.fresh();
                self.remote.insert(
                    remote.as_str().into(),
                    RFile {
                        doc,
                        etag,
                        content: l.content,
                    },
                );
                Ok(Some(transfer(doc, etag, l.inode, l.content)))
            }
            Action::UploadConflictCopy { path, .. } => {
                let Some(l) = self.local.get(path.as_str()).cloned() else {
                    return changed;
                };
                if !self.remote_key_free(path.as_str(), None) {
                    return changed;
                }
                let (doc, etag) = (self.fresh(), self.fresh());
                self.remote.insert(
                    path.as_str().into(),
                    RFile {
                        doc,
                        etag,
                        content: l.content,
                    },
                );
                Ok(Some(transfer(doc, etag, l.inode, l.content)))
            }
            Action::TrashRemote {
                path,
                expected_etag,
            } => match self.remote.get(path.as_str()) {
                Some(r) if r.etag.to_string() == expected_etag.as_str() => {
                    let r = self.remote.remove(path.as_str()).expect("exists");
                    self.kept.insert(r.content);
                    Ok(None)
                }
                _ => changed,
            },
            Action::MoveRemote {
                from,
                to,
                expected_etag,
            } => {
                let Some(r) = self.remote.get(from.as_str()) else {
                    return changed;
                };
                if r.etag.to_string() != expected_etag.as_str()
                    || !self.remote_key_free(to.as_str(), Some(r.doc))
                {
                    return changed;
                }
                let mut r = self.remote.remove(from.as_str()).expect("exists");
                if self.etag_changes_on_rename {
                    r.etag = self.fresh();
                }
                self.remote.insert(to.as_str().into(), r);
                Ok(None)
            }
            Action::Download {
                remote,
                local,
                overwrites_local,
                ..
            } => {
                let Some(r) = self.remote.get(remote.as_str()).cloned() else {
                    return changed;
                };
                match (overwrites_local, self.local.get(local.as_str())) {
                    (true, Some(old)) => {
                        let old = old.content;
                        self.kept.insert(old); // local history
                    }
                    (false, None) => {}
                    _ => return changed,
                }
                // Written to a temporary file and renamed: a new inode.
                let inode = self.fresh();
                self.local.insert(
                    local.as_str().into(),
                    LFile {
                        inode,
                        content: r.content,
                    },
                );
                Ok(Some(transfer(r.doc, r.etag, inode, r.content)))
            }
            Action::TrashLocal { path } => match self.local.remove(path.as_str()) {
                Some(l) => {
                    self.kept.insert(l.content);
                    Ok(None)
                }
                None => changed,
            },
            Action::MoveLocal { from, to } => {
                if !self.local.contains_key(from.as_str()) || self.local.contains_key(to.as_str()) {
                    return changed;
                }
                let l = self.local.remove(from.as_str()).expect("exists");
                self.local.insert(to.as_str().into(), l);
                Ok(None)
            }
        }
    }

    /// Names present on more than one local path (a collision sync skips).
    fn colliding_keys(&self) -> BTreeSet<String> {
        let mut count: BTreeMap<String, usize> = BTreeMap::new();
        for p in self.local.keys() {
            *count.entry(key(p)).or_default() += 1;
        }
        count
            .into_iter()
            .filter(|(_, n)| *n > 1)
            .map(|(k, _)| k)
            .collect()
    }
}

fn transfer(doc: u64, etag: u64, inode: u64, content: u64) -> Transfer {
    Transfer {
        doc_id: DocId::new(doc.to_string()),
        etag: Etag::new(etag.to_string()),
        inode,
        hash: ContentHash::new(content.to_string()),
    }
}

impl Effects for World {
    fn apply(&mut self, action: ApprovedAction) -> Result<Option<Transfer>, EffectError> {
        let result = self.perform(action.action());
        let verb = if result.is_ok() { "did " } else { "skip" };
        self.trace.push(format!("  {verb} {:?}", action.action()));
        result
    }
}

// --- the properties ---------------------------------------------------------------------

/// How a run ended (for the statistics that show the simulation checks something).
#[derive(Debug, Default, Clone, Copy)]
struct Outcome {
    stopped_by_safety: bool,
    compared_files: usize,
    skipped_files: usize,
}

const STOPPED: Outcome = Outcome {
    stopped_by_safety: true,
    compared_files: 0,
    skipped_files: 0,
};

fn run(seed: u64, etag_changes_on_rename: bool) -> Result<Outcome, String> {
    let mut rng = Rng::new(seed);
    let mut world = World {
        etag_changes_on_rename,
        ..World::default()
    };
    simulate(&mut world, &mut rng).map_err(|e| format!("{e}\ntrace:\n{}", world.trace.join("\n")))
}

fn simulate(world: &mut World, rng: &mut Rng) -> Result<Outcome, String> {
    let steps = 10 + rng.below(30);
    for _ in 0..steps {
        if rng.below(3) == 0 {
            let before = world.alive();
            let stopped = !world.sync_step();
            let after = world.alive();
            let lost: Vec<&u64> = before.difference(&after).collect();
            if !lost.is_empty() {
                return Err(format!("content lost by sync: {lost:?}"));
            }
            check_unique_remote(world)?;
            if stopped {
                return Ok(STOPPED); // a safe stop; nothing more to check
            }
        } else {
            world.user_step(rng);
        }
    }

    // The user stops; sync must converge.
    for _ in 0..12 {
        let before = world.alive();
        if !world.sync_step() {
            return Ok(STOPPED);
        }
        let after = world.alive();
        let lost: Vec<&u64> = before.difference(&after).collect();
        if !lost.is_empty() {
            return Err(format!("content lost by sync: {lost:?}"));
        }
        check_unique_remote(world)?;
    }
    // Files in a name collision are left alone by design, on both sides:
    // the colliding names, and both sides of every known file that touches
    // a colliding name (its local file, its document, its common path).
    let mut skipped = world.colliding_keys();
    let mut skipped_inodes: BTreeSet<u64> = BTreeSet::new();
    loop {
        let before = (skipped.len(), skipped_inodes.len());
        skipped_inodes.extend(
            world
                .local
                .iter()
                .filter(|(p, _)| skipped.contains(&key(p)))
                .map(|(_, f)| f.inode),
        );
        for b in &world.base {
            let remote = world
                .remote
                .iter()
                .find(|(_, r)| r.doc.to_string() == b.doc_id.as_str());
            let touches = b.inode.is_some_and(|i| skipped_inodes.contains(&i))
                || skipped.contains(&key(b.path.as_str()))
                || remote.is_some_and(|(p, _)| skipped.contains(&key(p)));
            if touches {
                skipped.insert(key(b.path.as_str()));
                skipped_inodes.extend(b.inode);
                if let Some((p, _)) = remote {
                    skipped.insert(key(p));
                }
                for (p, l) in &world.local {
                    if Some(l.inode) == b.inode {
                        skipped.insert(key(p));
                    }
                }
            }
        }
        if (skipped.len(), skipped_inodes.len()) == before {
            break;
        }
    }
    let local: BTreeMap<String, u64> = world
        .local
        .iter()
        .filter(|(p, _)| !skipped.contains(&key(p)))
        .map(|(p, f)| (p.clone(), f.content))
        .collect();
    let remote: BTreeMap<String, u64> = world
        .remote
        .iter()
        .filter(|(p, _)| !skipped.contains(&key(p)))
        .map(|(p, f)| (p.clone(), f.content))
        .collect();
    if local != remote {
        return Err(format!(
            "did not converge:\n local  {local:?}\n remote {remote:?}"
        ));
    }
    let plan = build_plan(&world.states(), &|p: &RelPath, n: u32| {
        RelPath::new(format!("{} (conflict {n})", p.as_str()))
    });
    if !plan.actions.is_empty() {
        return Err(format!(
            "converged but the plan is not empty: {:?}",
            plan.actions
        ));
    }
    Ok(Outcome {
        stopped_by_safety: false,
        compared_files: local.len(),
        skipped_files: world.local.len() + world.remote.len() - local.len() - remote.len(),
    })
}

fn check_unique_remote(world: &World) -> Result<(), String> {
    let keys: Vec<String> = world.remote.keys().map(|p| key(p)).collect();
    let unique: BTreeSet<&String> = keys.iter().collect();
    if unique.len() != keys.len() {
        return Err(format!(
            "two server files with one name: {:?}",
            world.remote.keys()
        ));
    }
    Ok(())
}

fn seeds() -> u64 {
    std::env::var("SIM_SEEDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(500)
}

fn check_all(etag_changes_on_rename: bool) {
    let (mut stopped, mut converged, mut compared, mut skipped) = (0u64, 0u64, 0usize, 0usize);
    for seed in 0..seeds() {
        match run(seed, etag_changes_on_rename) {
            Ok(o) if o.stopped_by_safety => stopped += 1,
            Ok(o) => {
                converged += 1;
                compared += o.compared_files;
                skipped += o.skipped_files;
            }
            Err(e) => panic!(
                "seed {seed} (etag_changes_on_rename={etag_changes_on_rename}): {e}\n\
                 reproduce: SIM_SEEDS={} cargo test -p ikeloud-sync --test simulation",
                seed + 1
            ),
        }
    }
    eprintln!(
        "etag_changes_on_rename={etag_changes_on_rename}: {converged} runs converged \
         ({compared} files compared, {skipped} left alone in name collisions), \
         {stopped} stopped safely by the safety layer"
    );
    // The simulation must actually check something.
    assert!(
        converged * 2 > seeds(),
        "too few runs reached the convergence check"
    );
    assert!(
        compared > skipped * 4,
        "too many files skipped to be meaningful"
    );
}

#[test]
fn simulation_etag_unchanged_on_rename() {
    check_all(false);
}

#[test]
fn simulation_etag_changes_on_rename() {
    check_all(true);
}
