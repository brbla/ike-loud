----------------------------- MODULE SyncNames -----------------------------
(***************************************************************************)
(* SyncMoves with names as they really are (crates/ikeloud-sync/src/       *)
(* names.rs, plan.rs).                                                     *)
(*                                                                         *)
(* A name has a spelling. The model uses four:                            *)
(*   1 = "a" (NFC), 2 = "a" decomposed (NFD, as a Mac writes it),          *)
(*   3 = "A", 4 = "b".                                                     *)
(* Linux tells all four apart. iCloud tells neither form nor letter case  *)
(* apart: one name per key (NFC, lower case), so 1, 2 and 3 are one name  *)
(* there. The engine:                                                      *)
(*   - compares places with same_place (equal after NFC: 1 and 2 are one  *)
(*     place, 3 is another, a case-only rename is a rename);               *)
(*   - checks server names by key and creates them in NFC;                 *)
(*   - keeps the local spelling (decision of 2026-11-15): a local NFD name *)
(*     paired with its NFC server name is in sync, nothing is renamed;     *)
(*   - leaves alone, on both sides, every name of a local collision (two   *)
(*     local files with one key, e.g. "a" and "A"): it cannot exist on     *)
(*     iCloud, the user must resolve it;                                   *)
(*   - when placing a file locally, evicts by key: a different file whose  *)
(*     name has the same key yields to its Alt name, so sync never makes a *)
(*     local collision itself.                                             *)
(*                                                                         *)
(* ExactNames = TRUE compares spellings instead of places (a bug): the    *)
(* config SyncNamesExact.cfg must fail.                                    *)
(*                                                                         *)
(* The rest as in SyncMoves.tla.                                           *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS
    Ids,          \* file identities
    UserPaths,    \* paths the user uses (numbers 1..k)
    InitIds,      \* identities that exist everywhere at the start (path = id)
    MaxEdits,     \* number of user changes
    MaxCrashes,   \* number of daemon crashes
    ExactNames    \* the bug: compare spellings, not places

Alt(i) == 100 + i

\* Spellings 1..4 (see above); Alt names are plain names of their own.
Key(p) == IF p \in {1, 2, 3} THEN 1 ELSE p
Nfc(p) == IF p = 2 THEN 1 ELSE p
SamePlace(p, q) == IF ExactNames THEN p = q ELSE Nfc(p) = Nfc(q)
Absent == [p |-> 0, c |-> 0]
Present(e) == e.p # 0

VARIABLES local, remote, base, kept, edits, crashes, pc, snap, lost,
    changes,  \* ghost: number of user actions that are not moves
    churn     \* ghost: the engine planned a move between two spellings of one place

vars == <<local, remote, base, kept, edits, crashes, pc, snap, lost, changes, churn>>

NoSnap == [l |-> Absent, r |-> Absent, b |-> Absent, act |-> "none"]

InitEntry(i) == IF i \in InitIds THEN [p |-> i, c |-> i] ELSE Absent

Init ==
    /\ local = [i \in Ids |-> InitEntry(i)]
    /\ remote = [i \in Ids |-> InitEntry(i)]
    /\ base = [i \in Ids |-> InitEntry(i)]
    /\ kept = {}
    /\ edits = 0
    /\ crashes = 0
    /\ pc = [i \in Ids |-> "idle"]
    /\ snap = [i \in Ids |-> NoSnap]
    /\ lost = {}
    /\ changes = 0
    /\ churn = FALSE

Fresh == 1000 + edits

\* Linux: a path is taken by the same spelling.
FreeIn(side, q) == \A j \in Ids : side[j].p # q
\* iCloud: a name is taken by any spelling with the same key, except by i.
FreeKey(side, q, i) == \A j \in Ids \ {i} : ~Present(side[j]) \/ Key(side[j].p) # Key(q)
OccupantKey(side, q, i) == CHOOSE j \in Ids \ {i} : Present(side[j]) /\ Key(side[j].p) = Key(q)

\* Keys of local collisions: left alone on both sides.
Held == {Key(local[i].p) : i \in {i \in Ids : Present(local[i]) /\
            \E j \in Ids \ {i} : Present(local[j]) /\ Key(local[j].p) = Key(local[i].p)}}
LocalCollision == Held # {}

AliveOf(l, r, k) ==
    ({l[i].c : i \in {j \in Ids : Present(l[j])}}
        \cup {r[i].c : i \in {j \in Ids : Present(r[j])}} \cup k)

---------------------------------------------------------------------------
(* User. *)

UserStep(side, side2, i, new) ==
    /\ edits < MaxEdits
    /\ edits' = edits + 1
    /\ changes' = IF Present(side[i]) /\ Present(new) /\ side[i].c = new.c
                  THEN changes ELSE changes + 1
    /\ side' = [side EXCEPT ![i] = new]
    /\ UNCHANGED side2

LocalEdit(i) ==
    /\ Present(local[i])
    /\ UserStep(local, remote, i, [local[i] EXCEPT !.c = Fresh])
    /\ UNCHANGED <<base, kept, crashes, pc, snap, lost, churn>>

LocalMove(i, q) ==
    /\ Present(local[i]) /\ q \in UserPaths /\ FreeIn(local, q)
    /\ UserStep(local, remote, i, [local[i] EXCEPT !.p = q])
    /\ UNCHANGED <<base, kept, crashes, pc, snap, lost, churn>>

LocalDelete(i) ==
    /\ Present(local[i])
    /\ UserStep(local, remote, i, Absent)
    /\ UNCHANGED <<base, kept, crashes, pc, snap, lost, churn>>

\* A new identity appears only where nobody knows about it.
LocalCreate(i, q) ==
    /\ ~Present(local[i]) /\ ~Present(remote[i]) /\ ~Present(base[i])
    /\ q \in UserPaths /\ FreeIn(local, q)
    /\ UserStep(local, remote, i, [p |-> q, c |-> Fresh])
    /\ UNCHANGED <<base, kept, crashes, pc, snap, lost, churn>>

RemoteEdit(i) ==
    /\ Present(remote[i])
    /\ UserStep(remote, local, i, [remote[i] EXCEPT !.c = Fresh])
    /\ UNCHANGED <<base, kept, crashes, pc, snap, lost, churn>>

RemoteMove(i, q) ==
    /\ Present(remote[i]) /\ q \in UserPaths /\ FreeKey(remote, q, i)
    /\ UserStep(remote, local, i, [remote[i] EXCEPT !.p = q])
    /\ UNCHANGED <<base, kept, crashes, pc, snap, lost, churn>>

\* A delete on the iPhone goes to "Recently Deleted".
RemoteDelete(i) ==
    /\ Present(remote[i])
    /\ UserStep(remote, local, i, Absent)
    /\ kept' = kept \cup {remote[i].c}
    /\ UNCHANGED <<base, crashes, pc, snap, lost, churn>>

RemoteCreate(i, q) ==
    /\ ~Present(local[i]) /\ ~Present(remote[i]) /\ ~Present(base[i])
    /\ q \in UserPaths /\ FreeKey(remote, q, i)
    /\ UserStep(remote, local, i, [p |-> q, c |-> Fresh])
    /\ UNCHANGED <<base, kept, crashes, pc, snap, lost, churn>>

User ==
    \E i \in Ids :
        \/ LocalEdit(i) \/ LocalDelete(i) \/ RemoteEdit(i) \/ RemoteDelete(i)
        \/ \E q \in UserPaths :
            LocalMove(i, q) \/ LocalCreate(i, q) \/ RemoteMove(i, q) \/ RemoteCreate(i, q)

---------------------------------------------------------------------------
(* Plan for one identity: content and existence first, then the path. *)

CChanged(x, b) == IF Present(x) /\ Present(b) THEN x.c # b.c ELSE Present(x) # Present(b)
Moved(x, b) == Present(x) /\ Present(b) /\ ~SamePlace(x.p, b.p)

Plan(l, r, b) ==
    LET lc == CChanged(l, b)
        rc == CChanged(r, b)
    IN
    IF ~Present(l) /\ ~Present(r) THEN (IF Present(b) THEN "forget" ELSE "none")
    ELSE IF lc /\ ~rc THEN
        (IF ~Present(l) THEN "delremote"
         ELSE IF Present(r) THEN "upload" ELSE "create")
    ELSE IF rc /\ ~lc THEN
        (IF ~Present(r) THEN "dellocal"
         ELSE IF Present(l) THEN "download" ELSE "fetch")
    ELSE IF lc /\ rc THEN
        (IF ~Present(l) THEN "fetch"
         ELSE IF ~Present(r) THEN "conflictgone"
         \* Same content on both sides: adopt it, path from iCloud.
         ELSE IF l.c = r.c THEN (IF SamePlace(l.p, r.p) THEN "adopt" ELSE "movelocal")
         ELSE "conflict")
    \* Content matches; the path remains. If both sides moved, iCloud wins.
    ELSE IF Moved(r, b) /\ ~SamePlace(l.p, r.p) THEN "movelocal"
    ELSE IF Moved(l, b) /\ ~SamePlace(l.p, r.p) THEN "moveremote"
    \* Nothing to carry over; the common state takes the current spellings
    \* (next_base runs after every run, with or without actions).
    ELSE IF l # b \/ r # b THEN "adopt"
    ELSE "none"

HeldId(i) == \E x \in {local[i], remote[i], base[i]} : Present(x) /\ Key(x.p) \in Held

Scan(i) ==
    /\ pc[i] = "idle"
    /\ ~HeldId(i)
    /\ LET a == Plan(local[i], remote[i], base[i]) IN
        /\ a # "none"
        /\ snap' = [snap EXCEPT ![i] = [l |-> local[i], r |-> remote[i], b |-> base[i], act |-> a]]
        /\ pc' = [pc EXCEPT ![i] = "planned"]
        /\ churn' = (churn \/ (a \in {"movelocal", "moveremote"}
                               /\ Nfc(local[i].p) = Nfc(remote[i].p)))
    /\ UNCHANGED <<local, remote, base, kept, edits, crashes, lost, changes>>

\* Places identity i on a local path; a file whose name has the same key
\* yields to Alt (evict_local).
PlaceLocal(l, i, e) ==
    IF FreeKey(l, e.p, i)
    THEN [l EXCEPT ![i] = e]
    ELSE LET j == OccupantKey(l, e.p, i) IN
         [l EXCEPT ![j] = [l[j] EXCEPT !.p = Alt(j)], ![i] = e]

\* Result of a write: ok = FALSE means abort (the state changed meanwhile).
Outcome(i) ==
    LET s == snap[i]
        L == local
        R == remote
        K == kept
        lok == L[i] = s.l       \* A1: the local file has not changed since Scan
        rok == R[i] = s.r       \* the server etag matches
        Res(l2, r2, k2) == [ok |-> TRUE, l |-> l2, r |-> r2, k |-> k2]
        Fail == [ok |-> FALSE, l |-> L, r |-> R, k |-> K]
        \* The local file yields to its Alt path.
        Yield == IF lok THEN Res([L EXCEPT ![i] = [L[i] EXCEPT !.p = Alt(i)]], R, K) ELSE Fail
    IN
    CASE s.act = "upload" ->
            IF rok THEN Res(L, [R EXCEPT ![i] = [R[i] EXCEPT !.c = s.l.c]], K \cup {s.r.c})
            ELSE Fail
      [] s.act = "create" ->
            IF Present(R[i]) THEN Fail
            ELSE IF FreeKey(R, s.l.p, i) THEN Res(L, [R EXCEPT ![i] = [p |-> Nfc(s.l.p), c |-> s.l.c]], K)
            ELSE Yield
      [] s.act = "delremote" ->
            IF rok THEN Res(L, [R EXCEPT ![i] = Absent], K \cup {R[i].c}) ELSE Fail
      [] s.act = "download" ->
            IF lok THEN Res([L EXCEPT ![i] = [L[i] EXCEPT !.c = s.r.c]], R, K \cup {L[i].c})
            ELSE Fail
      [] s.act = "fetch" ->
            IF Present(L[i]) THEN Fail
            ELSE Res(PlaceLocal(L, i, s.r), R, K)
      [] s.act = "dellocal" ->
            IF lok THEN Res([L EXCEPT ![i] = Absent], R, K \cup {L[i].c}) ELSE Fail
      [] s.act = "conflict" ->
            IF lok THEN Res([L EXCEPT ![i] = [L[i] EXCEPT !.c = s.r.c]], R, K \cup {s.l.c})
            ELSE Fail
      [] s.act = "conflictgone" ->
            IF lok THEN Res([L EXCEPT ![i] = Absent], R, K \cup {s.l.c}) ELSE Fail
      [] s.act = "movelocal" ->
            IF lok THEN Res(PlaceLocal(L, i, [L[i] EXCEPT !.p = s.r.p]), R, K) ELSE Fail
      [] s.act = "moveremote" ->
            IF ~rok THEN Fail
            ELSE IF FreeKey(R, s.l.p, i) THEN Res(L, [R EXCEPT ![i] = [R[i] EXCEPT !.p = Nfc(s.l.p)]], K)
            ELSE Yield
      [] s.act \in {"adopt", "forget"} -> Res(L, R, K)

\* New common state after a write: whatever matches on both sides.
NewBase(b, l, r) ==
    IF ~Present(l) /\ ~Present(r) THEN Absent
    ELSE IF ~Present(l) \/ ~Present(r) THEN b
    ELSE IF ~Present(b) THEN (IF SamePlace(l.p, r.p) /\ l.c = r.c THEN r ELSE b)
    \* The stored path is the server's spelling once both sides are one place.
    ELSE [p |-> IF SamePlace(l.p, r.p) THEN r.p ELSE b.p, c |-> IF l.c = r.c THEN l.c ELSE b.c]

Write(i) ==
    /\ pc[i] = "planned"
    /\ LET o == Outcome(i) IN
        IF o.ok
        THEN /\ local' = o.l
             /\ remote' = o.r
             /\ kept' = o.k
             /\ lost' = lost \cup (AliveOf(local, remote, kept) \ AliveOf(o.l, o.r, o.k))
             \* Commit: the new common state is stored in one transaction with
             \* the write for i; a crash in between is modelled in Sync.tla.
             /\ base' = [base EXCEPT ![i] = NewBase(base[i], o.l[i], o.r[i])]
             /\ pc' = [pc EXCEPT ![i] = "idle"]
        ELSE /\ pc' = [pc EXCEPT ![i] = "idle"]
             /\ UNCHANGED <<local, remote, kept, lost, base>>
    /\ UNCHANGED <<edits, crashes, snap, changes, churn>>

Crash ==
    /\ crashes < MaxCrashes
    /\ \E i \in Ids : pc[i] # "idle"
    /\ pc' = [i \in Ids |-> "idle"]
    /\ crashes' = crashes + 1
    /\ UNCHANGED <<local, remote, base, kept, edits, snap, lost, changes, churn>>

SyncStep(i) == Scan(i) \/ Write(i)

Next == User \/ Crash \/ \E i \in Ids : SyncStep(i)

Spec == Init /\ [][Next]_vars /\ \A i \in Ids : WF_vars(SyncStep(i))

---------------------------------------------------------------------------
(* Properties. *)

\* Sync never destroys content that exists nowhere else.
NoLoss == lost = {}

\* Linux never has two files on one path, iCloud never two with one key;
\* sync never makes a local collision (only the user can).
UniqueNames ==
    \A i, j \in Ids :
        i # j =>
            /\ (Present(local[i]) /\ Present(local[j]) => local[i].p # local[j].p)
            /\ (Present(remote[i]) /\ Present(remote[j]) => Key(remote[i].p) # Key(remote[j].p))

\* When the user only moves files, nothing ends up in the trash, in history
\* or in a conflict copy.
PureMovesKeepNothing == changes = 0 => kept = {}

\* A file is never moved between two spellings of the same place (no
\* rename ping-pong between an NFD name on Linux and NFC on iCloud).
NoRenameChurn == ~churn

\* Both sides hold the same files in the same places (spellings may differ
\* in form) and the common state is the server's.
Synced ==
    \A i \in Ids :
        /\ Present(local[i]) = Present(remote[i])
        /\ Present(local[i]) => /\ SamePlace(local[i].p, remote[i].p)
                                /\ local[i].c = remote[i].c
                                /\ base[i] = remote[i]
        /\ ~Present(local[i]) => ~Present(base[i])

\* Once things settle both sides agree, unless the user left a local
\* collision, which waits for the user.
Converge == <>[](Synced \/ LocalCollision)

=============================================================================
