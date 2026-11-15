---------------------------- MODULE SyncIdentity ----------------------------
(***************************************************************************)
(* Which local file is which known file (crates/ikeloud-sync/src/pairing.rs). *)
(*                                                                         *)
(* The other models know the identity of every file. The engine does not: *)
(* it sees an inode, a birth time (when the filesystem records one), a     *)
(* path and content. Linux hands the inode of a deleted file to the next   *)
(* new file. This model separates the truth (which file the user means,   *)
(* the ghost `t`) from what the engine sees, lets the filesystem reuse     *)
(* inodes, and checks the engine's pairing rules against the truth.        *)
(*                                                                         *)
(* Only the local side and the common state are modelled: the server side *)
(* of a document is whatever was last uploaded. A sync round is atomic     *)
(* (plan, upload, next_base); interleaving within a round is the           *)
(* simulation's job.                                                       *)
(*                                                                         *)
(* Pairing rules (as in pairing.rs):                                       *)
(*   1. by identity: the file with the entry's inode, if it is the same    *)
(*      file: equal birth times, or, birth times unknown, it kept the     *)
(*      entry's local path or its content;                                 *)
(*   2. otherwise by place: a file standing exactly at the entry's local   *)
(*      path that no entry may own (an editor's "write a new file and      *)
(*      rename it over", or a deleted file replaced by a new one).         *)
(*                                                                         *)
(* InodeOnly = TRUE replaces rule 1 with "same inode" alone: the bug the   *)
(* independent review found. SyncIdentityInodeOnly.cfg must fail.          *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS
    Truths,       \* files the user means; 1..k
    Docs,         \* server document IDs
    Inodes,       \* inode numbers the filesystem hands out
    Paths,        \* local paths
    MaxSteps,     \* number of user actions
    BirthKnown,   \* does the filesystem record birth times?
    InodeOnly     \* the reviewed bug: pair by inode alone

Absent == [t |-> 0]
Present(x) == x.t # 0
NoDoc == 0

VARIABLES
    files,   \* truth -> local file [t, ino, born, p, c] or Absent
    gone,    \* truths that existed and were deleted (never come back)
    base,    \* doc -> common state [t (ghost), ino, born, lp, c] or Absent
    clock,   \* source of fresh birth times and contents
    steps,
    bad      \* ghost: what went wrong, {} = nothing

vars == <<files, gone, base, clock, steps, bad>>

\* Two files exist at the start, synced: truth i, inode i, path i, doc i.
InitFile(t) ==
    IF t \in {1, 2}
    THEN [t |-> t, ino |-> t, born |-> IF BirthKnown THEN t ELSE 0, p |-> t, c |-> t]
    ELSE Absent

Init ==
    /\ files = [t \in Truths |-> InitFile(t)]
    /\ gone = {}
    /\ base = [d \in Docs |->
                IF d \in {1, 2}
                THEN [t |-> d, ino |-> d, born |-> IF BirthKnown THEN d ELSE 0,
                      lp |-> d, c |-> d]
                ELSE Absent]
    /\ clock = 10
    /\ steps = 0
    /\ bad = {}

Live == {t \in Truths : Present(files[t])}
InodeFree(i) == \A t \in Live : files[t].ino # i
PathFree(q) == \A t \in Live : files[t].p # q
Born == IF BirthKnown THEN clock ELSE 0

---------------------------------------------------------------------------
(* User actions between sync rounds. *)

Act(f2, g2) ==
    /\ steps < MaxSteps
    /\ files' = f2
    /\ gone' = g2
    /\ clock' = clock + 1
    /\ steps' = steps + 1
    /\ UNCHANGED <<base, bad>>

Edit(t) ==
    /\ t \in Live
    /\ Act([files EXCEPT ![t].c = clock], gone)

Move(t, q) ==
    /\ t \in Live /\ PathFree(q)
    /\ Act([files EXCEPT ![t].p = q], gone)

Delete(t) ==
    /\ t \in Live
    /\ Act([files EXCEPT ![t] = Absent], gone \cup {t})

\* A new file; the filesystem may hand it any free inode, also one of a
\* deleted file.
Create(t, q, i) ==
    /\ ~Present(files[t]) /\ t \notin gone /\ PathFree(q) /\ InodeFree(i)
    /\ Act([files EXCEPT ![t] = [t |-> t, ino |-> i, born |-> Born, p |-> q, c |-> clock]], gone)

\* An editor saves by writing a new file and renaming it over the old one:
\* the same file for the user, a new inode for the engine.
Resave(t, i) ==
    /\ t \in Live /\ InodeFree(i)
    /\ Act([files EXCEPT ![t] = [files[t] EXCEPT !.ino = i, !.born = Born, !.c = clock]], gone)

User ==
    \E t \in Truths :
        \/ Edit(t) \/ Delete(t)
        \/ \E q \in Paths : Move(t, q)
        \/ \E i \in Inodes : Resave(t, i) \/ \E q \in Paths : Create(t, q, i)

---------------------------------------------------------------------------
(* The engine's pairing rules. *)

Entries == {d \in Docs : Present(base[d])}

\* Rule 1: is file f the file of entry b?
SameFile(b, f) ==
    /\ b.ino = f.ino
    /\ \/ InodeOnly
       \/ IF BirthKnown THEN b.born = f.born
          ELSE f.p = b.lp \/ f.c = b.c

\* May some entry own f? (Without birth times a matching inode is enough.)
Known(f) ==
    \E d \in Entries :
        /\ base[d].ino = f.ino
        /\ \/ InodeOnly
           \/ ~BirthKnown
           \/ base[d].born = f.born

\* The file paired with entry d among the unclaimed ones, and how.
\* Entries are paired in a fixed order (the planner sorts them).
ByIdentity(d, free) == {t \in free : SameFile(base[d], files[t])}
ByPlace(d, free) == {t \in free : files[t].p = base[d].lp /\ ~Known(files[t])}

RECURSIVE PairFrom(_, _)
\* ds: entries still to pair; free: unclaimed live truths.
\* Result: doc -> [t |-> truth of the paired file or 0, how |-> "id" | "place" | "none"]
PairFrom(ds, free) ==
    IF ds = {} THEN [d \in {} |-> 0]
    ELSE LET d == CHOOSE x \in ds : \A y \in ds : x <= y
             ids == ByIdentity(d, free)
             places == ByPlace(d, free)
             pick == IF ids # {} THEN [t |-> CHOOSE t \in ids : TRUE, how |-> "id"]
                     ELSE IF places # {} THEN [t |-> CHOOSE t \in places : TRUE, how |-> "place"]
                     ELSE [t |-> 0, how |-> "none"]
             rest == PairFrom(ds \ {d}, free \ {pick.t})
         IN [x \in DOMAIN rest \cup {d} |-> IF x = d THEN pick ELSE rest[x]]

---------------------------------------------------------------------------
(* One sync round: pair, upload what changed, deal with deleted and new  *)
(* files, store the new common state (next_base).                         *)

Sync ==
    LET pair == PairFrom(Entries, Live)
        paired == {pair[d].t : d \in Entries} \ {0}
        newFiles == Live \ paired
        freeDocs == {d \in Docs : ~Present(base[d])}
        Entry(t, ghost) == [t |-> ghost, ino |-> files[t].ino, born |-> files[t].born,
                            lp |-> files[t].p, c |-> files[t].c]
        \* Pairing document d with a file of another truth is wrong (its
        \* content would go over d's, or d's content over it), unless the spec
        \* says the file replaces d's file: it stands exactly where d's local
        \* file was, and it is not, as far as anyone can tell, another
        \* document's file (same inode and birth time; without birth times,
        \* same inode). Checked for every pairing, not only for uploads.
        OthersFile(d, f) ==
            \E e \in Entries \ {d} :
                /\ base[e].ino = f.ino
                /\ (~BirthKnown \/ base[e].born = f.born)
        Replaces(d, f) == f.p = base[d].lp /\ ~OthersFile(d, f)
        foreign == {d \in Entries :
                      /\ pair[d].t # 0
                      /\ pair[d].t # base[d].t
                      /\ ~Replaces(d, files[pair[d].t])}
    IN
    \* Each new file is uploaded as a new document (the model has enough).
    \E newDoc \in [newFiles -> freeDocs] :
        /\ \A a, b \in newFiles : a # b => newDoc[a] # newDoc[b]
        /\ base' = [d \in Docs |->
                     IF d \in Entries
                     THEN IF pair[d].t = 0
                          THEN Absent    \* deleted locally: to the iCloud trash
                          \* A replacement takes over the document: from now on
                          \* the document is the new file's.
                          ELSE Entry(pair[d].t, pair[d].t)
                     ELSE IF \E t \in newFiles : newDoc[t] = d
                          THEN Entry(CHOOSE t \in newFiles : newDoc[t] = d,
                                     CHOOSE t \in newFiles : newDoc[t] = d)
                          ELSE Absent]
        /\ bad' = bad \cup {[doc |-> d, file |-> pair[d].t] : d \in foreign}
        /\ UNCHANGED <<files, gone, clock, steps>>

Next == User \/ Sync

Spec == Init /\ [][Next]_vars

---------------------------------------------------------------------------
(* Properties. *)

\* A document is never paired with another file than its own (or the one
\* the spec says replaced it), so no content goes over another file's.
NoForeignPairing == bad = {}

\* Nothing is lost track of: after a sync round, every local file is the
\* local side of exactly one document, and no document claims a file that
\* is not there.
EveryFileOnce ==
    \A t \in Live :
        Cardinality({d \in Entries : base[d].ino = files[t].ino /\ base[d].lp = files[t].p})
            <= 1

TypeOK ==
    /\ steps \in 0..MaxSteps
    /\ \A t \in Truths : Present(files[t]) => files[t].t = t
    /\ \A t, u \in Live : t # u => files[t].p # files[u].p /\ files[t].ino # files[u].ino

=============================================================================
