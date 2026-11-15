//! Deterministic simulation: the real planner, safety layer, executor and
//! common-state update against a simulated world (local disk, server) with
//! random user actions on both sides, also while a plan is being carried
//! out. Checks the properties of the TLA+ models on the real code:
//!
//! - **no loss**: a sync step never destroys content that exists nowhere else
//!   (on either side, in a trash, in history or in a conflict copy);
//! - **only synced content is set aside**: whatever sync moves to a trash or
//!   to history was at some point on both sides. A change the other side has
//!   never seen must stay a live file (possibly a conflict copy);
//! - **an overwrite replaces a version of the same file**, never another
//!   file's document (each content belongs to a lineage: created once, then
//!   edited; see `World::lineage`). The one exception is what no scan can
//!   tell apart from an editor's "write a new file and rename it over": a
//!   file with a new inode standing where the document's local file was,
//!   which is gone (see `World::note_resaves`);
//! - **unique names** on the server;
//! - **convergence**: once the user stops, both sides become equal (except
//!   names in a collision, which sync deliberately leaves alone).
//!
//! The world is hostile on purpose: the filesystem reuses inode numbers of
//! deleted files (with or without birth times, by variant), names come in
//! NFD and NFC, files may still be being written, and the user keeps working
//! between the plan and its execution.
//!
//! Every run is reproducible from its seed. Failures print the seed and the
//! trace. `SIM_SEEDS` sets the number of seeds (default 500).

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stderr
)] // test code

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use ikeloud_safety::{
    Action, ApprovedAction, ContentHash, Context, DEFAULT_SETTLE_TIME, Decision, Etag,
    LocalFolderState, LocalId, Rejection, RelPath, assess,
};
use ikeloud_sync::names::{key, nfc, same_place};
use ikeloud_sync::{
    BaseEntry, DocId, EffectError, Effects, LocalFile, RemoteFile, States, Transfer, build_plan,
    execute, next_base,
};

// --- deterministic randomness ----------------------------------------------------------

fn n64(n: usize) -> u64 {
    u64::try_from(n).expect("fits in u64")
}

#[derive(Debug)]
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
        usize::try_from(self.next() % n64(n)).expect("below n")
    }

    fn pick<T: Clone>(&mut self, items: &[T]) -> Option<T> {
        if items.is_empty() {
            None
        } else {
            Some(items[self.below(items.len())].clone())
        }
    }
}

// --- the simulated world -----------------------------------------------------------------

#[derive(Debug, Clone)]
struct LFile {
    inode: u64,
    birth: Option<i64>,
    content: u64,
    /// Still being written: left alone by the next sync run.
    busy: bool,
}

#[derive(Debug, Clone)]
struct RFile {
    doc: u64,
    etag: u64,
    content: u64,
}

/// Names the user picks from: letter-case variants, a folder, and one name
/// in NFD (as a Mac writes it) and NFC.
const NAMES: &[&str] = &[
    "a",
    "b",
    "c",
    "A",
    "d/e",
    "d/f",
    "Report.odt",
    "Pr\u{30c}i\u{301}loha",
    "Příloha",
];

#[derive(Debug, Clone, Copy)]
struct Variant {
    /// Does a rename on the server change the etag? (unknown, see 0010)
    etag_changes_on_rename: bool,
    /// Does the filesystem record birth times? (ext4, btrfs, xfs do)
    births_known: bool,
}

#[derive(Debug)]
struct World {
    variant: Variant,
    rng: Rng,
    local: BTreeMap<String, LFile>,
    remote: BTreeMap<String, RFile>,
    base: Vec<BaseEntry>,
    /// Content set aside: trashes and local history.
    kept: BTreeSet<u64>,
    /// Content that was on both sides at some point.
    synced: BTreeSet<u64>,
    /// Which file each content belongs to: a new file starts a lineage, an
    /// edit continues it.
    lineage: BTreeMap<u64, u64>,
    /// Inode numbers of deleted files, handed out again.
    free_inodes: Vec<u64>,
    next: u64,
    /// While a plan runs: the user acts between actions.
    interleave: bool,
    /// Content the user created and destroyed during the current sync step.
    user_created: BTreeSet<u64>,
    user_destroyed: BTreeSet<u64>,
    /// (local file, document) pairs that look like a re-save, see
    /// `note_resaves`.
    excused: BTreeSet<(LocalId, String)>,
    violations: Vec<String>,
    stats: Stats,
    trace: Vec<String>,
}

/// What the run exercised (to show the simulation checks something).
#[derive(Debug, Default, Clone, Copy)]
struct Stats {
    reused_inodes: usize,
    interleaved_steps: usize,
    skipped_actions: usize,
}

impl World {
    fn new(seed: u64, variant: Variant) -> Self {
        Self {
            variant,
            rng: Rng::new(seed),
            local: BTreeMap::new(),
            remote: BTreeMap::new(),
            base: vec![],
            kept: BTreeSet::new(),
            synced: BTreeSet::new(),
            lineage: BTreeMap::new(),
            free_inodes: vec![],
            next: 0,
            interleave: false,
            user_created: BTreeSet::new(),
            user_destroyed: BTreeSet::new(),
            excused: BTreeSet::new(),
            violations: vec![],
            stats: Stats::default(),
            trace: vec![],
        }
    }

    fn fresh(&mut self) -> u64 {
        self.next += 1;
        self.next
    }

    /// A new local file: the filesystem often hands out an inode number of
    /// a deleted file.
    fn new_file(&mut self, content: u64) -> LFile {
        let inode = if !self.free_inodes.is_empty() && self.rng.below(2) == 0 {
            self.stats.reused_inodes += 1;
            let i = self.rng.below(self.free_inodes.len());
            self.free_inodes.swap_remove(i)
        } else {
            self.fresh()
        };
        let birth = self
            .variant
            .births_known
            .then(|| i64::try_from(self.fresh()).expect("small"));
        LFile {
            inode,
            birth,
            content,
            busy: false,
        }
    }

    fn id(f: &LFile) -> LocalId {
        LocalId {
            device: 1,
            inode: f.inode,
            birth_ns: f.birth,
        }
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

    fn note_synced(&mut self) {
        let remote: BTreeSet<u64> = self.remote.values().map(|f| f.content).collect();
        for f in self.local.values() {
            if remote.contains(&f.content) {
                self.synced.insert(f.content);
            }
        }
    }

    fn states(&self) -> States {
        States {
            local: self
                .local
                .iter()
                .map(|(p, f)| LocalFile {
                    path: RelPath::new(p.as_str()),
                    id: Self::id(f),
                    hash: ContentHash::new(f.content.to_string()),
                    size: 1,
                    stable_for: if f.busy {
                        Duration::ZERO
                    } else {
                        Duration::from_secs(60)
                    },
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

    /// New content of a new file, or (`of`) a new version of an existing one.
    fn new_content(&mut self, of: Option<u64>) -> u64 {
        let c = self.fresh();
        let lineage = of.map_or(c, |old| self.lineage[&old]);
        self.lineage.insert(c, lineage);
        self.user_created.insert(c);
        c
    }

    /// The user destroyed this content (overwritten in place or deleted
    /// for good); not sync's doing.
    fn user_destroys(&mut self, content: u64) {
        self.user_destroyed.insert(content);
    }

    fn maybe_busy(&mut self) -> bool {
        self.rng.below(4) == 0
    }

    fn user_step(&mut self) {
        let local_paths: Vec<String> = self.local.keys().cloned().collect();
        let remote_paths: Vec<String> = self.remote.keys().cloned().collect();
        let name = (*self.rng.pick(NAMES).expect("names")).to_string();
        let local_pick = self.rng.pick(&local_paths);
        let remote_pick = self.rng.pick(&remote_paths);
        match self.rng.below(11) {
            0 | 1 => {
                if let Some(p) = local_pick {
                    let old = self.local[&p].content;
                    let c = self.new_content(Some(old));
                    self.user_destroys(old);
                    let busy = self.maybe_busy();
                    let f = self.local.get_mut(&p).expect("exists");
                    f.content = c;
                    f.busy |= busy;
                    self.trace.push(format!("local edit {p} -> {c}"));
                }
            }
            2 => {
                // An editor that saves by writing a new file and renaming it over.
                if let Some(p) = local_pick {
                    let old = self.local[&p].clone();
                    let c = self.new_content(Some(old.content));
                    let mut f = self.new_file(c);
                    f.busy = self.maybe_busy();
                    self.user_destroys(old.content);
                    self.free_inodes.push(old.inode);
                    self.local.insert(p.clone(), f);
                    self.trace
                        .push(format!("local save-as-new-inode {p} -> {c}"));
                }
            }
            3 => {
                if !self.local.contains_key(&name) {
                    let c = self.new_content(None);
                    let mut f = self.new_file(c);
                    f.busy = self.maybe_busy();
                    let inode = f.inode;
                    self.local.insert(name.clone(), f);
                    self.trace
                        .push(format!("local create {name} = {c} (inode {inode})"));
                }
            }
            // Never delete the last file of a side: an empty side stops the
            // sync by design (tested in ikeloud-safety), which ends the run.
            4 if local_paths.len() > 1 => {
                if let Some(p) = local_pick
                    && let Some(f) = self.local.remove(&p)
                {
                    self.user_destroys(f.content);
                    self.free_inodes.push(f.inode);
                    self.trace.push(format!("local delete {p}"));
                }
            }
            5 => {
                if let Some(p) = local_pick
                    && !self.local.contains_key(&name)
                    && let Some(f) = self.local.remove(&p)
                {
                    self.local.insert(name.clone(), f);
                    self.trace.push(format!("local move {p} -> {name}"));
                }
            }
            6 | 7 => {
                if let Some(p) = remote_pick {
                    let old = self.remote[&p].content;
                    let c = self.new_content(Some(old));
                    let e = self.fresh();
                    self.user_destroys(old); // no version history on iCloud
                    let f = self.remote.get_mut(&p).expect("exists");
                    f.content = c;
                    f.etag = e;
                    self.trace.push(format!("remote edit {p} -> {c}"));
                }
            }
            8 => {
                if self.remote_key_free(&name, None) {
                    let c = self.new_content(None);
                    let (d, e) = (self.fresh(), self.fresh());
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
                if let Some(p) = remote_pick
                    && let Some(f) = self.remote.remove(&p)
                {
                    // Deleting on the iPhone goes to "Recently Deleted".
                    self.kept.insert(f.content);
                    self.trace.push(format!("remote delete {p}"));
                }
            }
            _ => {
                if let Some(p) = remote_pick {
                    let doc = self.remote[&p].doc;
                    if p != name && self.remote_key_free(&name, Some(doc)) {
                        let mut f = self.remote.remove(&p).expect("exists");
                        if self.variant.etag_changes_on_rename {
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
        self.note_synced();
        self.note_resaves();
        let states = self.states();
        let plan = build_plan(&states, DEFAULT_SETTLE_TIME, &copy_name);
        // A file is busy for one run only.
        for f in self.local.values_mut() {
            f.busy = false;
        }

        let mut context = Context::new(n64(self.base.len()), key);
        context.local_folder = LocalFolderState::Available {
            file_count: n64(self.local.len()),
        };
        context.remote_item_count = n64(self.remote.len());
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
        self.stats.skipped_actions += report.skipped.len();
        self.note_synced();
        // next_base pairs by the same rules as the planner, against the
        // scan after the run (the user may have worked meanwhile).
        self.note_resaves();
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

    /// Sync sets `content` aside (trash or history): it must have been on
    /// both sides.
    fn set_aside(&mut self, content: u64, why: &str) {
        if !self.synced.contains(&content) {
            self.violations.push(format!(
                "{why}: content {content} was never on both sides, yet sync set it aside"
            ));
        }
        self.kept.insert(content);
    }

    /// An overwrite must replace a version of the same file.
    fn check_same_file(&mut self, new: u64, old: u64, what: &str) {
        self.check_same_file_unless(new, old, what, false);
    }

    fn check_same_file_unless(&mut self, new: u64, old: u64, what: &str, excused: bool) {
        if self.lineage[&new] != self.lineage[&old] && !excused {
            self.violations.push(format!(
                "{what}: content {new} replaced {old}, a version of another file"
            ));
        }
    }

    /// Records the local files a scan cannot tell apart from a new version
    /// of document `doc` (spec: an editor's "write a new file and rename it
    /// over", and a deleted file replaced by a new one on the same path): a
    /// file standing where the document's local file was, that is not
    /// (possibly) another known file, while the document's own file is not
    /// elsewhere. Computed from the spec rules, not by the code under test;
    /// once sync pairs such a file with the document, it stays excused.
    fn note_resaves(&mut self) {
        let mut excuse = Vec::new();
        for b in &self.base {
            let own_file_elsewhere = b.local.is_some_and(|k| {
                self.local.iter().any(|(path, f)| {
                    !same_place(b.local_path.as_str(), path)
                        && match k.same_file(&Self::id(f)) {
                            Some(same) => same,
                            // Without birth times: the documented rule.
                            None => b.hash.as_str() == f.content.to_string(),
                        }
                })
            });
            if own_file_elsewhere {
                continue;
            }
            for (path, f) in &self.local {
                let id = Self::id(f);
                let another_files = self.base.iter().any(|other| {
                    other.doc_id != b.doc_id
                        && other.local.is_some_and(|k| k.same_file(&id) != Some(false))
                });
                if same_place(b.local_path.as_str(), path) && !another_files {
                    excuse.push((id, b.doc_id.as_str().to_string()));
                }
            }
        }
        self.excused.extend(excuse);
    }

    /// The local file at `path` if it is still the file the plan saw.
    fn local_if(&self, path: &RelPath, id: &LocalId) -> Option<LFile> {
        self.local
            .get(path.as_str())
            .filter(|f| Self::id(f) == *id)
            .cloned()
    }

    /// Carries out one action like the real effects must: check the
    /// preconditions right before acting (server writes conditional on the
    /// etag; local writes conditional on the file and its content, never
    /// overwriting anything else), keep replaced content in history, and
    /// report what each transfer produced.
    fn perform(&mut self, action: &Action) -> Result<Option<Transfer>, EffectError> {
        let changed = Err(EffectError::Changed);
        match action {
            Action::CreateRemoteFolder { .. } => Ok(None),
            Action::Upload {
                local,
                remote,
                local_id,
                expected_etag,
                ..
            } => {
                let Some(l) = self.local_if(local, local_id) else {
                    return changed;
                };
                let doc = match (expected_etag, self.remote.get(remote.as_str()).cloned()) {
                    (Some(e), Some(r)) if e.as_str() == r.etag.to_string() => {
                        let resaved = self.excused.contains(&(Self::id(&l), r.doc.to_string()));
                        self.check_same_file_unless(l.content, r.content, "upload", resaved);
                        self.set_aside(r.content, "upload over a server file");
                        r.doc
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
                Ok(Some(transfer(doc, etag, &l)))
            }
            Action::UploadConflictCopy { path, local_id, .. } => {
                let Some(l) = self.local_if(path, local_id) else {
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
                Ok(Some(transfer(doc, etag, &l)))
            }
            Action::TrashRemote {
                path,
                expected_etag,
            } => match self.remote.get(path.as_str()) {
                Some(r) if r.etag.to_string() == expected_etag.as_str() => {
                    let r = self.remote.remove(path.as_str()).expect("exists");
                    self.set_aside(r.content, "iCloud trash");
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
                if self.variant.etag_changes_on_rename {
                    r.etag = self.fresh();
                }
                self.remote.insert(to.as_str().into(), r);
                Ok(None)
            }
            Action::Download {
                remote,
                local,
                replaces,
                ..
            } => {
                let Some(r) = self.remote.get(remote.as_str()).cloned() else {
                    return changed;
                };
                let old = match (replaces, self.local.get(local.as_str()).cloned()) {
                    (Some(v), Some(old))
                        if Self::id(&old) == v.id && old.content.to_string() == v.hash.as_str() =>
                    {
                        Some(old)
                    }
                    (None, None) => None,
                    _ => return changed,
                };
                if let Some(old) = old {
                    self.check_same_file(r.content, old.content, "download");
                    self.set_aside(old.content, "local history");
                    self.free_inodes.push(old.inode);
                }
                // Written to a temporary file and renamed over: a new inode.
                let f = self.new_file(r.content);
                let t = transfer(r.doc, r.etag, &f);
                self.local.insert(local.as_str().into(), f);
                Ok(Some(t))
            }
            Action::TrashLocal { path, expected } => match self.local_if(path, &expected.id) {
                Some(l) if l.content.to_string() == expected.hash.as_str() => {
                    self.local.remove(path.as_str());
                    self.set_aside(l.content, "system trash");
                    self.free_inodes.push(l.inode);
                    Ok(None)
                }
                _ => changed,
            },
            Action::MoveLocal { from, to, local_id } => {
                if self.local_if(from, local_id).is_none() || self.local.contains_key(to.as_str()) {
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

fn copy_name(p: &RelPath, n: u32) -> RelPath {
    RelPath::new(format!("{} (conflict {n})", p.as_str()))
}

fn transfer(doc: u64, etag: u64, l: &LFile) -> Transfer {
    Transfer {
        doc_id: DocId::new(doc.to_string()),
        etag: Etag::new(etag.to_string()),
        local: World::id(l),
        hash: ContentHash::new(l.content.to_string()),
    }
}

impl Effects for World {
    fn apply(&mut self, action: ApprovedAction) -> Result<Option<Transfer>, EffectError> {
        // The user keeps working while the plan runs.
        if self.interleave && self.rng.below(4) == 0 {
            self.stats.interleaved_steps += 1;
            self.trace.push("  (meanwhile)".into());
            self.user_step();
        }
        let result = self.perform(action.action());
        self.note_synced();
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
    stats: Stats,
}

fn run(seed: u64, variant: Variant) -> Result<Outcome, String> {
    let mut world = World::new(seed, variant);
    let result = simulate(&mut world);
    result
        .map(|o| Outcome {
            stats: world.stats,
            ..o
        })
        .map_err(|e| format!("{e}\ntrace:\n{}", world.trace.join("\n")))
}

/// One sync step with the checks of every step.
fn checked_sync(world: &mut World) -> Result<bool, String> {
    world.user_created.clear();
    world.user_destroyed.clear();
    let mut before = world.alive();
    let synced = world.sync_step();
    before.extend(&world.user_created);
    let after = world.alive();
    let lost: Vec<&u64> = before
        .difference(&after)
        .filter(|c| !world.user_destroyed.contains(c))
        .collect();
    if !lost.is_empty() {
        return Err(format!("content lost by sync: {lost:?}"));
    }
    if !world.violations.is_empty() {
        return Err(world.violations.join("\n"));
    }
    check_unique_remote(world)?;
    Ok(synced)
}

fn simulate(world: &mut World) -> Result<Outcome, String> {
    let stopped = Outcome {
        stopped_by_safety: true,
        ..Outcome::default()
    };
    let steps = 10 + world.rng.below(30);
    world.interleave = true;
    for _ in 0..steps {
        if world.rng.below(3) == 0 {
            if !checked_sync(world)? {
                return Ok(stopped); // a safe stop; nothing more to check
            }
        } else {
            world.user_step();
        }
    }

    // The user stops; sync must converge.
    world.interleave = false;
    for _ in 0..12 {
        if !checked_sync(world)? {
            return Ok(stopped);
        }
    }
    // Files in a name collision are left alone by design, on both sides:
    // the colliding names, and both sides of every known file that touches
    // a colliding name (its local file, its document, its common path).
    let mut skipped = world.colliding_keys();
    let mut skipped_ids: BTreeSet<LocalId> = BTreeSet::new();
    loop {
        let before = (skipped.len(), skipped_ids.len());
        skipped_ids.extend(
            world
                .local
                .iter()
                .filter(|(p, _)| skipped.contains(&key(p)))
                .map(|(_, f)| World::id(f)),
        );
        for b in &world.base {
            let remote = world
                .remote
                .iter()
                .find(|(_, r)| r.doc.to_string() == b.doc_id.as_str());
            let touches = b.local.is_some_and(|i| skipped_ids.contains(&i))
                || skipped.contains(&key(b.path.as_str()))
                || remote.is_some_and(|(p, _)| skipped.contains(&key(p)));
            if touches {
                skipped.insert(key(b.path.as_str()));
                skipped_ids.extend(b.local);
                if let Some((p, _)) = remote {
                    skipped.insert(key(p));
                }
                for (p, l) in &world.local {
                    if Some(World::id(l)) == b.local {
                        skipped.insert(key(p));
                    }
                }
            }
        }
        if (skipped.len(), skipped_ids.len()) == before {
            break;
        }
    }
    // A local name stays as the user wrote it (NFD from a Mac stays NFD);
    // the server has it in NFC. Compared after NFC.
    let local: BTreeMap<String, u64> = world
        .local
        .iter()
        .filter(|(p, _)| !skipped.contains(&key(p)))
        .map(|(p, f)| (nfc(p), f.content))
        .collect();
    let remote: BTreeMap<String, u64> = world
        .remote
        .iter()
        .filter(|(p, _)| !skipped.contains(&key(p)))
        .map(|(p, f)| (nfc(p), f.content))
        .collect();
    if local != remote {
        return Err(format!(
            "did not converge:\n local  {local:?}\n remote {remote:?}"
        ));
    }
    let plan = build_plan(&world.states(), DEFAULT_SETTLE_TIME, &copy_name);
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
        stats: Stats::default(),
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

fn check_all(variant: Variant) {
    let (mut stopped, mut converged, mut compared, mut skipped) = (0u64, 0u64, 0usize, 0usize);
    let mut stats = Stats::default();
    for seed in 0..seeds() {
        match run(seed, variant) {
            Ok(o) => {
                stats.reused_inodes += o.stats.reused_inodes;
                stats.interleaved_steps += o.stats.interleaved_steps;
                stats.skipped_actions += o.stats.skipped_actions;
                if o.stopped_by_safety {
                    stopped += 1;
                } else {
                    converged += 1;
                    compared += o.compared_files;
                    skipped += o.skipped_files;
                }
            }
            Err(e) => panic!(
                "seed {seed} ({variant:?}): {e}\n\
                 reproduce: SIM_SEEDS={} cargo test -p ikeloud-sync --test simulation",
                seed + 1
            ),
        }
    }
    eprintln!(
        "{variant:?}: {converged} runs converged ({compared} files compared, {skipped} left \
         alone in name collisions), {stopped} stopped safely by the safety layer; \
         {stats:?}"
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
    assert!(
        n64(stats.reused_inodes) > seeds(),
        "inodes are rarely reused"
    );
    assert!(
        n64(stats.interleaved_steps) > seeds(),
        "the user rarely acts during a run"
    );
    assert!(
        stats.skipped_actions > 0,
        "no precondition was ever violated"
    );
}

#[test]
fn simulation_etag_unchanged_on_rename() {
    check_all(Variant {
        etag_changes_on_rename: false,
        births_known: true,
    });
}

#[test]
fn simulation_etag_changes_on_rename() {
    check_all(Variant {
        etag_changes_on_rename: true,
        births_known: true,
    });
}

#[test]
fn simulation_without_birth_times() {
    check_all(Variant {
        etag_changes_on_rename: false,
        births_known: false,
    });
}
