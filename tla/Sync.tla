-------------------------------- MODULE Sync --------------------------------
(***************************************************************************)
(* Model synchronizace ike loud se třemi stavy: lokální, vzdálený a       *)
(* poslední společný (base). Odpovídá rozhodovací tabulce v zadání:       *)
(*                                                                         *)
(*   změna jen na Linuxu  -> nahrát                                        *)
(*   změna jen na iCloudu -> stáhnout                                      *)
(*   změna na obou        -> iCloud vyhrává, lokální jde do konfl. kopie   *)
(*   žádná změna          -> nic                                           *)
(*                                                                         *)
(* Obsah souboru je číslo (každá úprava dá nové), NONE = soubor neexistuje.*)
(* Uživatel může kdykoli upravit nebo smazat soubor na kterékoli straně,   *)
(* i uprostřed synchronizace. Démon může kdykoli spadnout.                 *)
(*                                                                         *)
(* Kroky synchronizace jednoho souboru:                                    *)
(*   Scan   -> přečte obě strany a sestaví akci (pc = "planned")           *)
(*   Check  -> jen když server etag NEvynucuje: ověří etag před zápisem    *)
(*   Write  -> provede akci; když server etag vynucuje, ověří ho atomicky  *)
(*   Commit -> uloží nový společný stav (SQLite)                           *)
(* Pád mezi Write a Commit nechá starý base; další Scan to musí ustát.     *)
(*                                                                         *)
(* Předpoklady (v kódu je musí splnit implementace):                       *)
(*   A1 Lokální „ověř a nahraď“ je atomické: nová verze se zapisuje do     *)
(*      dočasného souboru a přejmenuje se, předchozí jde do historie.      *)
(*   A2 Přesun do koše na serveru přesune skutečnou aktuální položku.      *)
(*   A3 Konfliktní kopie je nový soubor, nic nepřepisuje.                  *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS
    N,             \* počet souborů
    MaxEdits,      \* kolik uživatelských změn celkem (omezení stavového prostoru)
    MaxCrashes,    \* kolik pádů démona celkem
    EtagEnforced   \* TRUE = server odmítne zápis při jiném etagu

Files == 1..N
NONE == 0
Actions == {"none", "upload", "delremote", "download", "dellocal", "conflict", "adopt"}
RemoteWrites == {"upload", "delremote"}

VARIABLES
    local,    \* obsah souboru na Linuxu
    remote,   \* obsah souboru na iCloudu
    etag,     \* verze na serveru, roste s každým zápisem
    base,     \* poslední společný stav (SQLite)
    kept,     \* obsah odložený stranou: koš, lokální historie, konfliktní kopie
    edits,    \* počet uživatelských změn
    crashes,  \* počet pádů
    pc,       \* krok synchronizace pro každý soubor
    snap,     \* co Scan viděl a co naplánoval
    lost      \* duch: obsah, který zničila synchronizace (musí zůstat prázdný)

vars == <<local, remote, etag, base, kept, edits, crashes, pc, snap, lost>>

(* Rozhodovací tabulka ze zadání. *)
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
(* Uživatel: úpravy a mazání na obou stranách, kdykoli. *)

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

\* Smazání na iPhonu jde do „Nedávno smazané“ (není to práce synchronizace).
UserRemoteDelete(f) ==
    /\ edits < MaxEdits
    /\ remote[f] # NONE
    /\ remote' = [remote EXCEPT ![f] = NONE]
    /\ etag' = [etag EXCEPT ![f] = @ + 1]
    /\ kept' = kept \cup {remote[f]}
    /\ edits' = edits + 1
    /\ UNCHANGED <<local, base, crashes, pc, snap, lost>>

---------------------------------------------------------------------------
(* Synchronizace. *)

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

\* Bez vynuceného etagu: kontrola těsně před zápisem. Mezi ní a zápisem
\* zůstává okno, ve kterém může přijít změna z iPhonu.
Check(f) ==
    /\ pc[f] = "planned"
    /\ ~EtagEnforced
    /\ snap[f].act \in RemoteWrites
    /\ IF etag[f] = snap[f].e
       THEN /\ pc' = [pc EXCEPT ![f] = "checked"]
            /\ UNCHANGED <<local, remote, etag, base, kept, edits, crashes, snap, lost>>
       ELSE Abort(f)

\* Zápis s vedlejšími účinky; společné části: obsah ducha `lost`, krok pc.
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
              \* Lokální historie uloží verzi, kterou démon zná (s.r).
              ELSE Apply(f, local,
                         [remote EXCEPT ![f] = s.l],
                         [etag EXCEPT ![f] = @ + 1],
                         kept \cup {s.r}, s.l)
         [] s.act = "delremote" ->
              IF EtagEnforced /\ etag[f] # s.e THEN Abort(f)
              \* A2: do koše jde skutečná aktuální položka.
              ELSE Apply(f, local,
                         [remote EXCEPT ![f] = NONE],
                         [etag EXCEPT ![f] = @ + 1],
                         kept \cup {remote[f]}, NONE)
         [] s.act = "download" ->
              \* A1: lokální soubor se nesmí změnit od Scan.
              IF local[f] # s.l THEN Abort(f)
              ELSE Apply(f, [local EXCEPT ![f] = s.r], remote, etag,
                         kept \cup {local[f]}, s.r)
         [] s.act = "dellocal" ->
              IF local[f] # s.l THEN Abort(f)
              \* Lokální kopie jde do koše systému.
              ELSE Apply(f, [local EXCEPT ![f] = NONE], remote, etag,
                         kept \cup {local[f]}, NONE)
         [] s.act = "conflict" ->
              IF local[f] # s.l THEN Abort(f)
              \* A3: lokální verze se uloží jako konfliktní kopie.
              ELSE Apply(f, [local EXCEPT ![f] = s.r], remote, etag,
                         kept \cup {s.l}, s.r)
         [] s.act = "adopt" ->
              Apply(f, local, remote, etag, kept, s.l)

Commit(f) ==
    /\ pc[f] = "written"
    /\ base' = [base EXCEPT ![f] = snap[f].nb]
    /\ pc' = [pc EXCEPT ![f] = "idle"]
    /\ UNCHANGED <<local, remote, etag, kept, edits, crashes, snap, lost>>

\* Pád: rozpracované kroky se zapomenou, base zůstává z poslední transakce.
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
(* Vlastnosti. *)

TypeOK ==
    /\ local \in [Files -> Nat]
    /\ remote \in [Files -> Nat]
    /\ base \in [Files -> Nat]
    /\ pc \in [Files -> {"idle", "planned", "checked", "written"}]
    /\ \A f \in Files : snap[f].act \in Actions

\* Hlavní invariant: synchronizace nikdy nezničí obsah, který není nikde jinde
\* (na Linuxu, na iCloudu, v koši, v historii ani v konfliktní kopii).
NoLoss == lost = {}

\* Po ustálení (uživatel přestal měnit, démon přestal padat) se obě strany
\* shodují a společný stav sedí.
Converge == <>[](\A f \in Files : local[f] = remote[f] /\ base[f] = local[f])

=============================================================================
