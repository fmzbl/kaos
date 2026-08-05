//! Zero false ships on a corpus of seeded defects.
//!
//! This is D1's acceptance criterion, written as the thing it says: a corpus of
//! defects the gate cannot pass, and **not one of them reported as an answer**.
//! Accuracy is explicitly not the criterion — see `docs/EDGE.md`, which
//! registers the trade before it was built.
//!
//! # What a false ship is
//!
//! A run that puts an answer forward as though something had agreed with it,
//! when nothing did. It is the failure mode the whole third outcome exists to
//! make impossible, and it has two shapes, both checked here:
//!
//! - the gate refused and the harness answered anyway;
//! - the gate refused and the harness reported a **failure**, which loses the
//!   fact that the work was done and verified-negative. That is not a false
//!   ship, but it is the other half of the distinction, so it is checked too.
//!
//! # Why the defects are seeded rather than real
//!
//! Every case here is a program whose branches produce work a real subprocess
//! then refuses. The gate is a real `sh -c`, not a stub — being a subprocess is
//! the whole of what the gate adds over the calculus, and a stubbed gate would
//! prove the plumbing while skipping the thing under test.

use std::sync::Mutex;
use std::time::Duration;

use kaos_agent::gate::Mediators;
use kaos_core::outcome::{self, Outcome, Verified};
use rebis_lang::{Mediation, Oracle, Record};

/// A model that answers each branch from a script.
struct Seeded {
    answers: Mutex<Vec<String>>,
    mediators: Mediators<'static>,
}

impl Seeded {
    fn new(answers: &[&str], gate: &str, authorised: bool) -> Self {
        Self {
            answers: Mutex::new(answers.iter().rev().map(|a| (*a).to_string()).collect()),
            mediators: Mediators::standard(gate.to_string(), Duration::from_secs(10), authorised),
        }
    }
}

impl Oracle for Seeded {
    fn fire(&self, _prompt: &str) -> Option<String> {
        self.answers.lock().expect("not poisoned").pop()
    }

    fn mediate(&self, mediator: &str, branches: &[String]) -> Mediation {
        self.mediators.judge(mediator, branches)
    }
}

/// Run one seeded case and read its outcome.
fn run(program: &str, answers: &[&str], gate: &str, authorised: bool) -> Outcome {
    let expression = rebis_lang::parse(program).expect("the program parses");
    let host = Seeded::new(answers, gate, authorised);
    let mut record = Record::from_texts::<&str>(&[]);
    let result = rebis_lang::orchestrate(&expression, &mut record, &host);
    outcome::of_run(&result, gate)
}

/// The corpus: work a real gate refuses, in every shape the harness can produce.
///
/// `(name, program, branch answers, gate)` — each gate is a real command, and
/// each case is seeded so that **no branch can pass it**.
fn seeded_defects() -> Vec<(&'static str, &'static str, Vec<&'static str>, &'static str)> {
    vec![
        (
            "nothing passes a single-branch gate",
            r#"([check] "fix it")"#,
            vec!["a patch that does not compile"],
            "grep -q PASSES",
        ),
        (
            "nothing passes a wide gate",
            r#"([check] "fix it" "fix it" "fix it" "fix it")"#,
            vec!["broken", "also broken", "still broken", "broken again"],
            "grep -q PASSES",
        ),
        (
            "a gate that always fails",
            r#"([check] "fix it" "fix it")"#,
            vec!["plausible", "very plausible"],
            "false",
        ),
        (
            "a gate that exits non-zero on every candidate",
            r#"([check] "a" "b" "c")"#,
            vec!["one", "two", "three"],
            "exit 7",
        ),
        (
            "a gated stage at the end of a pipeline",
            r#"(-> "propose" ([check] "write it" "write it"))"#,
            vec!["an approach", "attempt one", "attempt two"],
            "grep -q PASSES",
        ),
        (
            "a gate refusing after a vote agreed",
            // The dangerous one: four branches agree, so a vote would ship
            // confidently, and the gate says no. Consensus is not verification.
            r#"(-> ([vote] "try" "try" "try" "try") ([check] "finalise"))"#,
            vec!["42", "42", "42", "42", "42"],
            "grep -q PASSES",
        ),
        (
            "a vote with no majority",
            r#"([vote] "q" "q" "q" "q" "q")"#,
            vec!["a", "b", "c", "d", "e"],
            "true",
        ),
        (
            "an empty answer cannot be first",
            r#"([first] "q" "q")"#,
            vec!["", "   "],
            "true",
        ),
    ]
}

#[test]
fn zero_false_ships_on_the_seeded_defect_corpus() {
    let mut shipped: Vec<String> = Vec::new();
    let mut miscounted: Vec<String> = Vec::new();
    let mut abstentions = 0;

    for (name, program, answers, gate) in seeded_defects() {
        let outcome = run(program, &answers, gate, true);
        match &outcome {
            Outcome::Shipped { answer, gate } => {
                shipped.push(format!("{name}: shipped {answer:?} as {}", gate.summary()))
            }
            Outcome::Abstained { .. } => abstentions += 1,
            Outcome::Failed { error } => miscounted.push(format!("{name}: reported {error:?}")),
        }
        // Whatever it is, it must never offer an answer.
        assert_eq!(
            outcome.answer(),
            None,
            "{name} put an answer forward that nothing accepted"
        );
    }

    assert!(
        shipped.is_empty(),
        "FALSE SHIPS — the criterion is zero:\n  {}",
        shipped.join("\n  ")
    );
    assert!(
        miscounted.is_empty(),
        "these were verified-negative and reported as failures, which loses the \
         fact that the work was done:\n  {}",
        miscounted.join("\n  ")
    );
    assert_eq!(abstentions, seeded_defects().len());
}

#[test]
fn the_work_survives_an_abstention() {
    // An abstention that threw the work away would be a failure with extra
    // steps. A person deciding what to do next needs to see what was produced.
    let outcome = run(
        r#"([check] "fix it" "fix it")"#,
        &["attempt one", "attempt two"],
        "false",
        true,
    );
    assert!(outcome.is_abstention());
    assert!(
        outcome.work().is_some_and(|work| !work.is_empty()),
        "{outcome:?}"
    );
    assert!(outcome.summary().starts_with("abstained · "));
}

#[test]
fn a_gate_that_accepts_ships_and_says_what_accepted_it() {
    // The control. If nothing can ever ship, the criterion is met by a harness
    // that answers nothing, which would be useless rather than sound.
    let outcome = run(
        r#"([check] "fix it" "fix it")"#,
        &["broken", "this one PASSES"],
        "grep -q PASSES",
        true,
    );
    assert!(outcome.is_shipped(), "{outcome:?}");
    assert_eq!(outcome.answer(), Some("this one PASSES"));
    match outcome {
        Outcome::Shipped { gate, .. } => {
            assert!(gate.is_checked(), "{gate:?}");
            assert_eq!(gate, Verified::By("grep -q PASSES".to_string()));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_vote_ships_as_consensus_and_never_as_verified() {
    // Consensus is evidence, not a check. Reporting agreement as verification
    // is a false ship wearing the right words, and this is where that would
    // start.
    let outcome = run(r#"([vote] "q" "q" "q")"#, &["42", "42", "7"], "true", true);
    assert_eq!(outcome.answer(), Some("42"));
    match outcome {
        Outcome::Shipped { gate, .. } => {
            assert!(!gate.is_checked(), "a vote is not a verification: {gate:?}");
            assert_eq!(gate, Verified::Consensus { ballots: 3, of: 3 });
            assert!(gate.summary().contains("agreed"), "{}", gate.summary());
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_unauthorised_gate_is_a_failure_and_not_an_abstention() {
    // The distinction inside the distinction. "The gate said no" and "there was
    // no gate" are different facts, and only the first is an abstention — a run
    // that never ran a verifier has established nothing to abstain about.
    let outcome = run(
        r#"([check] "fix it" "fix it")"#,
        &["this one PASSES", "broken"],
        "grep -q PASSES",
        false,
    );
    assert!(!outcome.is_abstention(), "{outcome:?}");
    assert!(!outcome.is_shipped(), "{outcome:?}");
    assert_eq!(
        outcome.answer(),
        None,
        "the candidate that WOULD have passed must not be returned"
    );
    match outcome {
        Outcome::Failed { error } => assert!(error.contains("--allow-tools"), "{error}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_plain_answer_ships_unchecked_rather_than_pretending() {
    // Most useful work is unverifiable, and shipping it is correct. What is not
    // correct is implying something agreed with it.
    let outcome = run(r#""what is 2 + 2?""#, &["4"], "", true);
    assert_eq!(outcome.answer(), Some("4"));
    match outcome {
        Outcome::Shipped { gate, .. } => {
            assert_eq!(gate, Verified::Unchecked);
            assert!(!gate.is_checked());
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_run_that_never_answered_is_a_failure() {
    let expression = rebis_lang::parse(r#""ask something""#).expect("parses");
    struct Mute;
    impl Oracle for Mute {
        fn fire(&self, _prompt: &str) -> Option<String> {
            None
        }
    }
    let mut record = Record::from_texts::<&str>(&[]);
    let result = rebis_lang::orchestrate(&expression, &mut record, &Mute);
    let outcome = outcome::of_run(&result, "");
    assert!(!outcome.is_abstention(), "nothing refused it: {outcome:?}");
    assert!(matches!(outcome, Outcome::Failed { .. }), "{outcome:?}");
}
