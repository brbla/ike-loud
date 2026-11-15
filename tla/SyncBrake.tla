----------------------------- MODULE SyncBrake -----------------------------
(***************************************************************************)
(* The mass-change brake over time (crates/ikeloud-safety/src/assessment.rs *)
(* check_mass_change, crates/ikeloud-daemon/src/run.rs recent_changes).    *)
(*                                                                         *)
(* The user deletes synced files locally; every run plans to delete them   *)
(* on the server. The brake counts the deletions of the plan together with *)
(* those in the journal of the brake window: the last Window hours, but    *)
(* not before the last recorded confirmation. Above the threshold the run  *)
(* stops and asks, listing the files. The user may confirm exactly that    *)
(* list; a later plan that is a subset of it passes. A run may stop after  *)
(* any number of its actions (offline). Overwrites count like deletions   *)
(* (the same counter); moves have their own counter with the same logic.   *)
(*                                                                         *)
(* Two variants show the bugs this model found (decision 0020), each      *)
(* config of it must fail NoNeedlessQuestion:                              *)
(*   ConfirmFirst = TRUE: the confirmation is recorded in the journal      *)
(*     before the confirmed actions, so they count against the next plan;  *)
(*   KeepConfirmation = FALSE: a run stopped midway (offline) uses the     *)
(*     confirmation up, so the rest of the confirmed plan asks again.      *)
(* and one must fail NoSilentMassDeletion:                                 *)
(*   RecordAnyConfirmation = TRUE: a confirmation passed to a run whose    *)
(*     plan it does not cover (the files changed meanwhile) still restarts *)
(*     the window, so deletions the user never saw are not counted.        *)
(***************************************************************************)
EXTENDS Naturals, Sequences, FiniteSets

CONSTANTS
    Files,          \* synced files
    Absolute,       \* above this many the brake always stops
    Minimum,        \* above this many it stops when also above Percent
    Percent,        \* of the synced files
    Window,         \* brake window in hours (journal_since: time >= now - Window)
    MaxTime,        \* hours modelled
    MaxSteps,       \* user deletions and restores
    MaxRuns,        \* sync runs (a run stopped at once still records a
                    \* confirmation, so the journal would grow forever)
    ConfirmFirst,   \* confirmation recorded before the confirmed actions
    KeepConfirmation, \* a confirmation survives a run stopped midway
    RecordAnyConfirmation \* record any passed confirmation, needed or not

\* No confirmation: the empty set (a question always lists some files).
NONE == {}

VARIABLES
    clock,
    steps,
    runs,
    synced,         \* files synced (the common state)
    pending,        \* deleted locally, not yet on the server
    journal,        \* sequence of [t, k \in {"del", "conf"}], oldest first
    asked,          \* the files of the last question, {} if none
    confirmation,   \* the files the user confirmed, NONE ({}) if none
    \* Ghosts:
    epoch,          \* how many times a run passed the brake by a confirmation
    granted,        \* deletions the user confirmed last (not restored since)
    log,            \* executed deletions: [t, epoch, confirmed]
    needless        \* the brake asked although the user had confirmed enough

vars == <<clock, steps, runs, synced, pending, journal, asked, confirmation, epoch, granted, log, needless>>

Init ==
    /\ clock = 0
    /\ steps = 0
    /\ runs = 0
    /\ synced = Files
    /\ pending = {}
    /\ journal = <<>>
    /\ asked = {}
    /\ confirmation = NONE
    /\ epoch = 0
    /\ granted = {}
    /\ log = {}
    /\ needless = FALSE

Exceeded(count, total) ==
    \/ count > Absolute
    \/ count > Minimum /\ count * 100 > Percent * total

\* Deletions in the brake window: newest first, stop at a confirmation.
RECURSIVE RecentFrom(_)
RecentFrom(n) ==
    IF n = 0 THEN 0
    ELSE LET e == journal[n] IN
         IF e.t + Window < clock THEN 0
         ELSE IF e.k = "conf" THEN 0
         ELSE 1 + RecentFrom(n - 1)
Recent == RecentFrom(Len(journal))

\* Sequence of one "del" entry per element of a set, at the current time.
RECURSIVE Dels(_)
Dels(s) ==
    IF s = {} THEN <<>>
    ELSE LET x == CHOOSE y \in s : TRUE IN <<[t |-> clock, k |-> "del"]>> \o Dels(s \ {x})

\* What the spec asks: deletions the user has not confirmed, of the window
\* and since the user last confirmed.
Unconfirmed == {e \in log : ~e.confirmed /\ e.epoch = epoch /\ e.t + Window >= clock}

---------------------------------------------------------------------------

Tick ==
    /\ clock < MaxTime
    /\ clock' = clock + 1
    /\ UNCHANGED <<steps, runs, synced, pending, journal, asked, confirmation, epoch, granted, log, needless>>

Delete(f) ==
    /\ steps < MaxSteps /\ steps' = steps + 1
    /\ f \in synced \ pending
    /\ pending' = pending \cup {f}
    /\ UNCHANGED <<clock, runs, synced, journal, asked, confirmation, epoch, granted, log, needless>>

\* Restored from the system trash before the next run. Deleting it again
\* later is a new deletion, not the one the user confirmed.
Restore(f) ==
    /\ steps < MaxSteps /\ steps' = steps + 1
    /\ f \in pending
    /\ pending' = pending \ {f}
    /\ granted' = granted \ {f}
    /\ UNCHANGED <<clock, runs, synced, journal, asked, confirmation, epoch, log, needless>>

Confirm ==
    /\ asked # {}
    /\ confirmation' = asked
    \* A file restored meanwhile is not what the user agreed to delete.
    /\ granted' = asked \cap pending
    /\ UNCHANGED <<clock, steps, runs, synced, pending, journal, asked, epoch, log, needless>>

Run ==
    /\ pending # {}
    /\ runs < MaxRuns /\ runs' = runs + 1
    /\ LET plan == pending
           total == Cardinality(synced)
           brakes == Exceeded(Recent + Cardinality(plan), total)
           confirmed == confirmation # NONE /\ plan \subseteq confirmation
       IN
       IF brakes /\ ~confirmed
       THEN /\ asked' = plan
            \* Needless: about files all confirmed already, or because
            \* confirmed deletions were counted.
            /\ needless' = (needless \/ plan \subseteq granted
                   \/ ~Exceeded(Cardinality(Unconfirmed) + Cardinality(plan), total))
            /\ UNCHANGED <<clock, steps, synced, pending, journal, confirmation, epoch, granted, log>>
       \* Carried out, maybe only partly (offline after some actions).
       ELSE \E done \in SUBSET plan :
            LET \* The user's confirmation is what let the plan pass.
                used == brakes /\ confirmed
                record == IF RecordAnyConfirmation THEN confirmation # NONE ELSE confirmed
                conf == IF record THEN <<[t |-> clock, k |-> "conf"]>> ELSE <<>>
            IN
            /\ journal' = IF ConfirmFirst THEN journal \o conf \o Dels(done)
                          ELSE journal \o Dels(done) \o conf
            /\ synced' = synced \ done
            /\ pending' = pending \ done
            \* What the spec means by "the window restarts": the user's
            \* answer was acted upon.
            /\ epoch' = IF used THEN epoch + 1 ELSE epoch
            /\ log' = log \cup {[t |-> clock, epoch |-> epoch', f |-> f,
                                 \* Confirmed: carried out under a confirmation
                                 \* that covers the plan it was part of.
                                 confirmed |-> confirmed]
                                : f \in done}
            \* The daemon keeps a confirmation until a run carried out its
            \* whole plan (KeepConfirmation; before decision 0020: until a
            \* run got past the brake).
            /\ confirmation' = IF KeepConfirmation /\ done # plan THEN confirmation ELSE NONE
            /\ asked' = {}
            /\ UNCHANGED <<clock, steps, granted, needless>>

Next ==
    \/ Tick \/ Run \/ Confirm
    \/ \E f \in Files : Delete(f) \/ Restore(f)

Spec == Init /\ [][Next]_vars

---------------------------------------------------------------------------
(* Properties. *)

\* No more than Absolute deletions the user did not confirm, in any window,
\* since the user last confirmed: spreading them over runs does not help.
NoSilentMassDeletion ==
    \A t0 \in 0..MaxTime :
        Cardinality({e \in log : ~e.confirmed /\ e.epoch = epoch
                                 /\ t0 <= e.t /\ e.t <= t0 + Window}) <= Absolute

\* The brake asks only when it must: never about files all confirmed
\* already, and never because deletions the user confirmed were counted.
\* (A plan that grows after a confirmation asks again, by design: the
\* confirmation covers exactly the listed files.)
NoNeedlessQuestion == ~needless

TypeOK ==
    /\ clock \in 0..MaxTime /\ steps \in 0..MaxSteps /\ runs \in 0..MaxRuns
    /\ synced \subseteq Files /\ pending \subseteq synced
    /\ asked \subseteq Files
    /\ confirmation \subseteq Files

=============================================================================
