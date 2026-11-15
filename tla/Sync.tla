-------------------------------- MODULE Sync --------------------------------
(***************************************************************************)
(* Sync model of ike loud with three states: local, remote and the last   *)
(* common one (base). It follows the decision table of the spec:          *)
(*                                                                         *)
(*   change only on Linux  -> upload                                       *)
(*   change only on iCloud -> download                                     *)
(*   change on both        -> iCloud wins, local goes to a conflict copy   *)
(*   no change             -> nothing                                      *)
(*                                                                         *)
(* File content is a number (every edit gives a new one), NONE = no file.  *)
(* The user may edit or delete a file on either side at any time, even in  *)
(* the middle of a sync. The daemon may crash at any time.                 *)
(*                                                                         *)
(* Sync steps for one file:                                                *)
(*   Scan   -> reads both sides and plans an action (pc = "planned")       *)
(*   Check  -> only if the server does NOT enforce etags: checks the etag  *)
(*             right before the write                                      *)
(*   Write  -> performs the action; if the server enforces etags, the      *)
(*             check is atomic                                             *)
(*   Commit -> stores the new common state (SQLite)                        *)
(* A crash between Write and Commit leaves the old base; the next Scan     *)
(* must cope with it.                                                      *)
(*                                                                         *)
(* Assumptions (the implementation must meet them in code):                *)
(*   A1 Local "check and replace" is atomic: the new version is written    *)
(*      to a temporary file and renamed, the previous one goes to history. *)
(*   A2 Moving to the server trash moves the actual current item.          *)
(*   A3 A conflict copy is a new file and overwrites nothing.              *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS
    N,             \* number of files
    MaxEdits,      \* total user changes (bounds the state space)
    MaxCrashes,    \* total daemon crashes
    EtagEnforced   \* TRUE = the server rejects a write with a different etag

Files == 1..N
NONE == 0
Actions == {"none", "upload", "delremote", "download", "dellocal", "conflict", "adopt"}
RemoteWrites == {"upload", "delremote"}

VARIABLES
    local,    \* file content on Linux
    remote,   \* file content on iCloud
    etag,     \* server version, grows with every write
    base,     \* last common state (SQLite)
    kept,     \* content set aside: trash, local history, conflict copies
    edits,    \* number of user changes
    crashes,  \* number of crashes
    pc,       \* sync step for each file
    snap,     \* what Scan saw and planned
    lost      \* ghost: content destroyed by sync (must stay empty)

vars == <<local, remote, etag, base, kept, edits, crashes, pc, snap, lost>>

(* Decision table from the spec. *)
Plan(l, r, b) ==
    IF l = b /\ r = b THEN "none"
    ELSE IF r = b THEN (IF l = NONE THEN "delremote" ELSE "upload")
    ELSE IF l = b THEN (IF r = NONE THEN "dellocal" ELSE "download")
    ELSE IF l = r THEN "adopt"
    ELSE "conflict"

AliveOf(l, r, k) ==
    ({l[f] : f \in Files} \cup {r[f] : f \in Files} \cup k) \ {NONE}

Alive == AliveOf(local, remote, kept)

NoSnap == [l |-> NONE, r |-> NONE, e |-> 0, act |-> "none", nb |-> NONE]

Init ==
    /\ local = [f \in Files |-> f]
    /\ remote = [f \in Files |-> f]
    /\ etag = [f \in Files |-> 1]
    /\ base = [f \in Files |-> f]
    /\ kept = {}
    /\ edits = 0
    /\ crashes = 0
    /\ pc = [f \in Files |-> "idle"]
    /\ snap = [f \in Files |-> NoSnap]
    /\ lost = {}

Fresh == N + 1 + edits

---------------------------------------------------------------------------
(* User: edits and deletes on both sides, at any time. *)

UserLocalEdit(f) ==
    /\ edits < MaxEdits
    /\ local' = [local EXCEPT ![f] = Fresh]
    /\ edits' = edits + 1
    /\ UNCHANGED <<remote, etag, base, kept, crashes, pc, snap, lost>>

UserLocalDelete(f) ==
    /\ edits < MaxEdits
    /\ local[f] # NONE
    /\ local' = [local EXCEPT ![f] = NONE]
    /\ edits' = edits + 1
    /\ UNCHANGED <<remote, etag, base, kept, crashes, pc, snap, lost>>

UserRemoteEdit(f) ==
    /\ edits < MaxEdits
    /\ remote' = [remote EXCEPT ![f] = Fresh]
    /\ etag' = [etag EXCEPT ![f] = @ + 1]
    /\ edits' = edits + 1
    /\ UNCHANGED <<local, base, kept, crashes, pc, snap, lost>>

\* A delete on the iPhone goes to "Recently Deleted" (not the sync's doing).
UserRemoteDelete(f) ==
    /\ edits < MaxEdits
    /\ remote[f] # NONE
    /\ remote' = [remote EXCEPT ![f] = NONE]
    /\ etag' = [etag EXCEPT ![f] = @ + 1]
    /\ kept' = kept \cup {remote[f]}
    /\ edits' = edits + 1
    /\ UNCHANGED <<local, base, crashes, pc, snap, lost>>

---------------------------------------------------------------------------
(* Sync. *)

Scan(f) ==
    /\ pc[f] = "idle"
    /\ LET a == Plan(local[f], remote[f], base[f]) IN
        /\ a # "none"
        /\ snap' = [snap EXCEPT ![f] =
                [l |-> local[f], r |-> remote[f], e |-> etag[f], act |-> a, nb |-> NONE]]
        /\ pc' = [pc EXCEPT ![f] = "planned"]
    /\ UNCHANGED <<local, remote, etag, base, kept, edits, crashes, lost>>

Abort(f) ==
    /\ pc' = [pc EXCEPT ![f] = "idle"]
    /\ UNCHANGED <<local, remote, etag, base, kept, edits, crashes, snap, lost>>

\* Without an enforced etag: a check right before the write. Between the
\* check and the write there is a window in which an iPhone change can land.
Check(f) ==
    /\ pc[f] = "planned"
    /\ ~EtagEnforced
    /\ snap[f].act \in RemoteWrites
    /\ IF etag[f] = snap[f].e
       THEN /\ pc' = [pc EXCEPT ![f] = "checked"]
            /\ UNCHANGED <<local, remote, etag, base, kept, edits, crashes, snap, lost>>
       ELSE Abort(f)

\* A write with side effects; shared parts: the `lost` ghost and the pc step.
Apply(f, l2, r2, e2, k2, nb) ==
    /\ local' = l2
    /\ remote' = r2
    /\ etag' = e2
    /\ kept' = k2
    /\ lost' = lost \cup (Alive \ AliveOf(l2, r2, k2))
    /\ snap' = [snap EXCEPT ![f].nb = nb]
    /\ pc' = [pc EXCEPT ![f] = "written"]
    /\ UNCHANGED <<base, edits, crashes>>

ReadyToWrite(f) ==
    \/ pc[f] = "checked"
    \/ pc[f] = "planned" /\ (EtagEnforced \/ snap[f].act \notin RemoteWrites)

Write(f) ==
    /\ ReadyToWrite(f)
    /\ LET s == snap[f] IN
       CASE s.act = "upload" ->
              IF EtagEnforced /\ etag[f] # s.e THEN Abort(f)
              \* Local history keeps the version the daemon knows (s.r).
              ELSE Apply(f, local,
                         [remote EXCEPT ![f] = s.l],
                         [etag EXCEPT ![f] = @ + 1],
                         kept \cup {s.r}, s.l)
         [] s.act = "delremote" ->
              IF EtagEnforced /\ etag[f] # s.e THEN Abort(f)
              \* A2: the actual current item goes to the trash.
              ELSE Apply(f, local,
                         [remote EXCEPT ![f] = NONE],
                         [etag EXCEPT ![f] = @ + 1],
                         kept \cup {remote[f]}, NONE)
         [] s.act = "download" ->
              \* A1: the local file must not have changed since Scan.
              IF local[f] # s.l THEN Abort(f)
              ELSE Apply(f, [local EXCEPT ![f] = s.r], remote, etag,
                         kept \cup {local[f]}, s.r)
         [] s.act = "dellocal" ->
              IF local[f] # s.l THEN Abort(f)
              \* The local copy goes to the system trash.
              ELSE Apply(f, [local EXCEPT ![f] = NONE], remote, etag,
                         kept \cup {local[f]}, NONE)
         [] s.act = "conflict" ->
              IF local[f] # s.l THEN Abort(f)
              \* A3: the local version is kept as a conflict copy.
              ELSE Apply(f, [local EXCEPT ![f] = s.r], remote, etag,
                         kept \cup {s.l}, s.r)
         [] s.act = "adopt" ->
              Apply(f, local, remote, etag, kept, s.l)

Commit(f) ==
    /\ pc[f] = "written"
    /\ base' = [base EXCEPT ![f] = snap[f].nb]
    /\ pc' = [pc EXCEPT ![f] = "idle"]
    /\ UNCHANGED <<local, remote, etag, kept, edits, crashes, snap, lost>>

\* Crash: steps in progress are forgotten, base stays from the last transaction.
Crash ==
    /\ crashes < MaxCrashes
    /\ \E f \in Files : pc[f] # "idle"
    /\ pc' = [f \in Files |-> "idle"]
    /\ crashes' = crashes + 1
    /\ UNCHANGED <<local, remote, etag, base, kept, edits, snap, lost>>

---------------------------------------------------------------------------

User == \E f \in Files :
    UserLocalEdit(f) \/ UserLocalDelete(f) \/ UserRemoteEdit(f) \/ UserRemoteDelete(f)

SyncStep(f) == Scan(f) \/ Check(f) \/ Write(f) \/ Commit(f)

Next == User \/ Crash \/ \E f \in Files : SyncStep(f)

Spec == Init /\ [][Next]_vars /\ \A f \in Files : WF_vars(SyncStep(f))

---------------------------------------------------------------------------
(* Properties. *)

TypeOK ==
    /\ local \in [Files -> Nat]
    /\ remote \in [Files -> Nat]
    /\ base \in [Files -> Nat]
    /\ pc \in [Files -> {"idle", "planned", "checked", "written"}]
    /\ \A f \in Files : snap[f].act \in Actions

\* Main invariant: sync never destroys content that exists nowhere else
\* (on Linux, on iCloud, in the trash, in history or in a conflict copy).
NoLoss == lost = {}

\* Once things settle (the user stops changing, the daemon stops crashing)
\* both sides match and the common state is right.
Converge == <>[](\A f \in Files : local[f] = remote[f] /\ base[f] = local[f])

=============================================================================
