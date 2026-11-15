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
    ikeloud_testkit::transfer(doc, "e", 1, "h")
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
