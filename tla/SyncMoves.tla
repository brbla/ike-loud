----------------------------- MODULE SyncMoves -----------------------------
(***************************************************************************)
(* Extension of the Sync model with moves and renames.                    *)
(*                                                                         *)
(* A file has an identity (inode locally, document ID on the server), a   *)
(* path and content. A move changes only the path. The engine pairs the    *)
(* sides by identity and carries a move over as a move, not as a delete   *)
(* and a new upload.                                                       *)
(*                                                                         *)
(* Every identity i has a reserved fallback path Alt(i): it stands for the *)
(* conflict copy name. The user never uses it directly.                    *)
(*                                                                         *)
(* Path collisions (iCloud wins, nothing is ever pushed aside on the       *)
(* server):                                                                *)
(*   - a move or new file from the server targets a locally taken path:    *)
(*     the local file standing there is renamed to its Alt path;           *)
(*   - a local move or new file targets a path taken on the server:        *)
(*     the local file yields to its Alt path.                              *)
(*                                                                         *)
(* The server enforces etags (see Sync.tla for why this is necessary).     *)
(* Assumptions A1–A3 as in Sync.tla.                                       *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS
    Ids,          \* file identities
    UserPaths,    \* paths the user uses (numbers 1..k)
    InitIds,      \* identities that exist everywhere at the start (path = id)
    MaxEdits,     \* number of user changes
    MaxCrashes    \* number of daemon crashes

Alt(i) == 100 + i
Absent == [p |-> 0, c |-> 0]
Present(e) == e.p # 0

VARIABLES local, remote, base, kept, edits, crashes, pc, snap, lost,
    changes   \* ghost: number of user actions that are not moves

vars == <<local, remote, base, kept, edits, crashes, pc, snap, lost, changes>>

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

Fresh == 1000 + edits

FreeIn(side, q) == \A j \in Ids : side[j].p # q
Occupant(side, q) == CHOOSE j \in Ids : side[j].p = q

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
    /\ UNCHANGED <<base, kept, crashes, pc, snap, lost>>

LocalMove(i, q) ==
    /\ Present(local[i]) /\ q \in UserPaths /\ FreeIn(local, q)
    /\ UserStep(local, remote, i, [local[i] EXCEPT !.p = q])
    /\ UNCHANGED <<base, kept, crashes, pc, snap, lost>>

LocalDelete(i) ==
    /\ Present(local[i])
    /\ UserStep(local, remote, i, Absent)
    /\ UNCHANGED <<base, kept, crashes, pc, snap, lost>>

\* A new identity appears only where nobody knows about it.
LocalCreate(i, q) ==
    /\ ~Present(local[i]) /\ ~Present(remote[i]) /\ ~Present(base[i])
    /\ q \in UserPaths /\ FreeIn(local, q)
    /\ UserStep(local, remote, i, [p |-> q, c |-> Fresh])
    /\ UNCHANGED <<base, kept, crashes, pc, snap, lost>>

RemoteEdit(i) ==
    /\ Present(remote[i])
    /\ UserStep(remote, local, i, [remote[i] EXCEPT !.c = Fresh])
    /\ UNCHANGED <<base, kept, crashes, pc, snap, lost>>

RemoteMove(i, q) ==
    /\ Present(remote[i]) /\ q \in UserPaths /\ FreeIn(remote, q)
    /\ UserStep(remote, local, i, [remote[i] EXCEPT !.p = q])
    /\ UNCHANGED <<base, kept, crashes, pc, snap, lost>>

\* A delete on the iPhone goes to "Recently Deleted".
RemoteDelete(i) ==
    /\ Present(remote[i])
    /\ UserStep(remote, local, i, Absent)
    /\ kept' = kept \cup {remote[i].c}
    /\ UNCHANGED <<base, crashes, pc, snap, lost>>

RemoteCreate(i, q) ==
    /\ ~Present(local[i]) /\ ~Present(remote[i]) /\ ~Present(base[i])
    /\ q \in UserPaths /\ FreeIn(remote, q)
    /\ UserStep(remote, local, i, [p |-> q, c |-> Fresh])
    /\ UNCHANGED <<base, kept, crashes, pc, snap, lost>>

User ==
    \E i \in Ids :
        \/ LocalEdit(i) \/ LocalDelete(i) \/ RemoteEdit(i) \/ RemoteDelete(i)
        \/ \E q \in UserPaths :
            LocalMove(i, q) \/ LocalCreate(i, q) \/ RemoteMove(i, q) \/ RemoteCreate(i, q)

---------------------------------------------------------------------------
(* Plan for one identity: content and existence first, then the path. *)

CChanged(x, b) == IF Present(x) /\ Present(b) THEN x.c # b.c ELSE Present(x) # Present(b)
Moved(x, b) == Present(x) /\ Present(b) /\ x.p # b.p

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
         ELSE IF l.c = r.c THEN (IF l.p = r.p THEN "adopt" ELSE "movelocal")
         ELSE "conflict")
    \* Content matches; the path remains. If both sides moved, iCloud wins.
    ELSE IF Moved(r, b) /\ l.p # r.p THEN "movelocal"
    ELSE IF Moved(l, b) /\ l.p # r.p THEN "moveremote"
    ELSE IF l # b THEN "adopt"
    ELSE "none"

Scan(i) ==
    /\ pc[i] = "idle"
    /\ LET a == Plan(local[i], remote[i], base[i]) IN
        /\ a # "none"
        /\ snap' = [snap EXCEPT ![i] = [l |-> local[i], r |-> remote[i], b |-> base[i], act |-> a]]
        /\ pc' = [pc EXCEPT ![i] = "planned"]
    /\ UNCHANGED <<local, remote, base, kept, edits, crashes, lost, changes>>

\* Places identity i on a local path; whoever stands there yields to Alt.
PlaceLocal(l, i, e) ==
    IF FreeIn(l, e.p) \/ Occupant(l, e.p) = i
    THEN [l EXCEPT ![i] = e]
    ELSE LET j == Occupant(l, e.p) IN
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
            ELSE IF FreeIn(R, s.l.p) THEN Res(L, [R EXCEPT ![i] = [p |-> s.l.p, c |-> s.l.c]], K)
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
            ELSE IF FreeIn(R, s.l.p) THEN Res(L, [R EXCEPT ![i] = [R[i] EXCEPT !.p = s.l.p]], K)
            ELSE Yield
      [] s.act \in {"adopt", "forget"} -> Res(L, R, K)

\* New common state after a write: whatever matches on both sides.
NewBase(b, l, r) ==
    IF ~Present(l) /\ ~Present(r) THEN Absent
    ELSE IF ~Present(l) \/ ~Present(r) THEN b
    ELSE IF ~Present(b) THEN (IF l = r THEN l ELSE b)
    ELSE [p |-> IF l.p = r.p THEN l.p ELSE b.p, c |-> IF l.c = r.c THEN l.c ELSE b.c]

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
    /\ UNCHANGED <<edits, crashes, snap, changes>>

Crash ==
    /\ crashes < MaxCrashes
    /\ \E i \in Ids : pc[i] # "idle"
    /\ pc' = [i \in Ids |-> "idle"]
    /\ crashes' = crashes + 1
    /\ UNCHANGED <<local, remote, base, kept, edits, snap, lost, changes>>

SyncStep(i) == Scan(i) \/ Write(i)

Next == User \/ Crash \/ \E i \in Ids : SyncStep(i)

Spec == Init /\ [][Next]_vars /\ \A i \in Ids : WF_vars(SyncStep(i))

---------------------------------------------------------------------------
(* Properties. *)

\* Sync never destroys content that exists nowhere else.
NoLoss == lost = {}

\* No side ever has two files on the same path (sync never moves a file
\* over another one).
UniquePaths ==
    \A i, j \in Ids :
        i # j =>
            /\ (Present(local[i]) /\ Present(local[j]) => local[i].p # local[j].p)
            /\ (Present(remote[i]) /\ Present(remote[j]) => remote[i].p # remote[j].p)

\* When the user only moves files, nothing ends up in the trash, in history
\* or in a conflict copy: a move is never carried over as delete + upload.
PureMovesKeepNothing == changes = 0 => kept = {}

\* Once things settle both sides are equal, paths included, and the common
\* state is right.
Converge == <>[](local = remote /\ base = local)

=============================================================================
