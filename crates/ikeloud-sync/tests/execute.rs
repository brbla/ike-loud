//! Tests of the executor with scripted effects.

#![allow(clippy::expect_used, clippy::panic)] // test code may panic

use std::collections::VecDeque;

use ikeloud_safety::{Action, ApprovedAction, Context, Decision, assess};
use ikeloud_sync::names::key;
use ikeloud_sync::{EffectError, Effects, Report, StopReason, Transfer, execute};
use ikeloud_testkit::{LOCAL_SIZE, STABLE, id, p};

/// Effects that answer from a script and record what they were asked to do.
struct Scripted {
    answers: VecDeque<Result<Option<Transfer>, EffectError>>,
    seen: Vec<Action>,
}

impl Effects for Scripted {
    fn apply(&mut self, action: ApprovedAction) -> Result<Option<Transfer>, EffectError> {
        self.seen.push(action.action().clone());
        self.answers
            .pop_front()
            .expect("an answer for every action")
    }
}

fn upload(path: &str) -> Action {
    Action::Upload {
        local: p(path),
        remote: p(path),
        local_id: id(1),
        expected_etag: None,
        size: LOCAL_SIZE,
        stable_for: STABLE,
    }
}

/// Approved actions, through the real safety layer (the only way to get them).
fn approved(actions: Vec<Action>) -> Vec<ApprovedAction> {
    match assess(actions, &Context::new(0, key)).expect("approved") {
        Decision::Approved(plan) => plan.actions,
        Decision::NothingToDo => vec![],
    }
}

fn transfer(doc: &str) -> Transfer {
    ikeloud_testkit::transfer("f", doc, "e", 1, "h")
}

#[test]
fn executor_outcomes() {
    type Answer = Result<Option<Transfer>, EffectError>;
    struct Case {
        name: &'static str,
        answers: Vec<Answer>,
        expected: Report,
        attempted: usize,
    }
    let (a, b, c) = (upload("a"), upload("b"), upload("c"));
    let cases = vec![
        Case {
            name: "all done, transfers collected in order",
            answers: vec![Ok(Some(transfer("1"))), Ok(None), Ok(Some(transfer("3")))],
            expected: Report {
                done: vec![a.clone(), b.clone(), c.clone()],
                transfers: vec![transfer("1"), transfer("3")],
                ..Report::default()
            },
            attempted: 3,
        },
        Case {
            name: "a changed precondition skips one action and goes on",
            answers: vec![Ok(None), Err(EffectError::Changed), Ok(None)],
            expected: Report {
                done: vec![a.clone(), c.clone()],
                skipped: vec![b.clone()],
                ..Report::default()
            },
            attempted: 3,
        },
        Case {
            name: "a stop ends the run; the rest is not attempted",
            answers: vec![
                Ok(Some(transfer("1"))),
                Err(EffectError::Stop(StopReason::Offline)),
            ],
            expected: Report {
                done: vec![a.clone()],
                skipped: vec![b.clone()],
                not_attempted: vec![c.clone()],
                transfers: vec![transfer("1")],
                stopped: Some(StopReason::Offline),
            },
            attempted: 2,
        },
        Case {
            name: "a stop on the first action",
            answers: vec![Err(EffectError::Stop(StopReason::SignedOut))],
            expected: Report {
                skipped: vec![a.clone()],
                not_attempted: vec![b.clone(), c.clone()],
                stopped: Some(StopReason::SignedOut),
                ..Report::default()
            },
            attempted: 1,
        },
    ];
    for case in cases {
        let mut effects = Scripted {
            answers: case.answers.into(),
            seen: vec![],
        };
        let report = execute(
            approved(vec![a.clone(), b.clone(), c.clone()]),
            &mut effects,
        );
        assert_eq!(report, case.expected, "{}", case.name);
        assert_eq!(effects.seen.len(), case.attempted, "{}", case.name);
        assert_eq!(
            effects.seen[..],
            [a.clone(), b.clone(), c.clone()][..case.attempted],
            "{}: order",
            case.name
        );
    }
}

#[test]
fn empty_plan() {
    let mut effects = Scripted {
        answers: VecDeque::new(),
        seen: vec![],
    };
    assert_eq!(execute(vec![], &mut effects), Report::default());
}

fn folder(path: &str) -> Action {
    Action::CreateRemoteFolder { path: p(path) }
}

fn move_local(path: &str) -> Action {
    Action::MoveLocal {
        from: p(path),
        to: p(&format!("{path}-moved")),
        local_id: id(9),
    }
}

#[test]
fn independent_actions_are_grouped_in_plan_order() {
    let plan = approved(vec![
        folder("a"),
        folder("b"),
        folder("a/x"),
        upload("a/x/1"),
        upload("a/x/2"),
        upload("b/3"),
        move_local("c"),
        move_local("d"),
        upload("e"),
    ]);
    let groups: Vec<Vec<Action>> = ikeloud_sync::independent(&plan)
        .into_iter()
        .map(|r| plan[r].iter().map(|a| a.action().clone()).collect())
        .collect();
    assert_eq!(
        groups,
        vec![
            vec![folder("a"), folder("b")],
            vec![folder("a/x")],
            vec![upload("a/x/1"), upload("a/x/2"), upload("b/3")],
            vec![move_local("c")],
            vec![move_local("d")],
            vec![upload("e")],
        ],
        "folders by depth, transfers together, moves one by one"
    );
}

/// Effects that run a group in reverse order, as threads might finish.
struct Reversed {
    answers: std::collections::HashMap<String, Result<Option<Transfer>, EffectError>>,
}

impl Effects for Reversed {
    fn apply(&mut self, action: ApprovedAction) -> Result<Option<Transfer>, EffectError> {
        let key = match action.action() {
            Action::Upload { remote, .. } => remote.as_str().to_string(),
            _ => String::new(),
        };
        self.answers.remove(&key).expect("an answer")
    }

    fn apply_group(
        &mut self,
        actions: Vec<ApprovedAction>,
    ) -> Vec<Option<ikeloud_sync::ApplyResult>> {
        let mut results: Vec<_> = actions
            .into_iter()
            .rev()
            .map(|a| Some(self.apply(a)))
            .collect();
        results.reverse();
        results
    }
}

#[test]
fn a_stop_in_a_group_keeps_what_the_others_did() {
    let plan = approved(vec![upload("a"), upload("b"), upload("c"), move_local("d")]);
    let mut effects = Reversed {
        answers: [
            ("a".to_string(), Ok(Some(transfer("da")))),
            ("b".to_string(), Err(EffectError::Stop(StopReason::Offline))),
            ("c".to_string(), Ok(Some(transfer("dc")))),
        ]
        .into(),
    };
    let report = execute(plan, &mut effects);
    // c finished although b stopped the run: done, so the state learns it.
    assert_eq!(report.done, vec![upload("a"), upload("c")]);
    assert_eq!(report.skipped, vec![upload("b")]);
    assert_eq!(report.not_attempted, vec![move_local("d")]);
    assert_eq!(report.stopped, Some(StopReason::Offline));
    assert_eq!(report.transfers.len(), 2);
}
