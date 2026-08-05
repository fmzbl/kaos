//! Is `myth` redundant? Every documented myth example, against its Rebis
//! translation, on the same scripted oracle.
//!
//! Kaos has two orchestration languages. `myth` is 518 lines with five forms and
//! four gates; Rebis is a git dependency with 22 frozen operators, a tested cost
//! model, and every other surface in the harness already written against it. The
//! plan for retiring `myth` assumed the mapping between them was close to exact.
//! **This file is the proof, and it is a test rather than a refactor** — if the
//! mapping does not hold, the cheapest possible thing has been spent finding out.
//!
//! # The method
//!
//! One [`Scripted`] oracle implements *both* seams — [`myth::Cast`] and
//! [`rebis_lang::Oracle`] — and answers by prompt content, so the two runtimes
//! are handed the identical world. Each case then asserts two things:
//!
//! - **answer-identical** — the myth and its translation collapse to the same
//!   final answer, and
//! - **firing-identical** — they spend the same number of model calls, which is
//!   the claim the whole harness is ordered around.
//!
//! Answering by *content* rather than by call index is deliberate. `myth`'s
//! `spread` fans out on scoped threads and takes its indices from an atomic, so
//! an index-keyed script would hand thread 0 a different answer on different
//! runs. Keying on the prompt makes both runtimes deterministic, and it is also
//! the more honest instrument: it can see that the two languages send *different
//! prompt text* for the same intent, which an index-keyed script cannot.
//!
//! # The finding
//!
//! **The mapping does not hold.** Three of the four gates have no Rebis
//! spelling, not one — see [`the_gate_gap_list`], which is the deliverable of a
//! failed proof and is asserted so it cannot drift silently.
//!
//! Two of those three were expected to be fine. The plan's own table wrote
//! `Gate::Vote → [consensus]` and `Gate::First → [first] or %`. Both readings
//! are wrong for the same reason, and it is worth stating once: **Rebis has no
//! table of mediator names.** `([anything] A B …)` whose head is pure symbols is
//! a *symbol mediator*, and every one of them runs the same code —
//! `resolve_mediation`, which scores each branch's round-trip holonomy against
//! the mediator's own words and keeps the lowest. `[consensus]` does not count
//! ballots; it prefers whichever branch reads most like the word "consensus".
//! `[first]` is not positional. They are one gate wearing different words.
//!
//! # What that changes
//!
//! Not the direction — the size of A2. The plan proposed a host-supplied
//! mediator so that `[check "cargo test"]` could exist without a 23rd operator.
//! The same seam is the answer to all three gaps: `[vote]`, `[first]` and
//! `[check …]` are host mediators resolved by name, and the operator set stays
//! frozen because to Rebis they are symbol mediators like any other. A2 is
//! therefore not one gate but three, and A4 cannot retire `myth.rs` until it is
//! built — retiring it today would silently lose self-consistency voting, which
//! is the one gate with a measured number behind it.

use std::collections::BTreeMap;
use std::sync::Mutex;

use kaos_agent::myth::{self, Cast};
use rebis_lang::{Oracle, Record};

// ── the shared world ────────────────────────────────────────────────────────

/// An oracle both languages can be run against.
///
/// Answers are keyed by a *substring* of the prompt, because the two runtimes
/// wrap the same intent in different words: `myth`'s `(ask "role")` sends
/// `"role\n\ntask"` and Rebis's `(+ "role" "task")` sends its own framing. A
/// rule matches on the part both carry — the role, or the task.
///
/// Each key holds a queue. `(spread 5 fire)` sends the identical prompt five
/// times and must be able to get five different answers back, or a vote has
/// nothing to count.
struct Scripted {
    rules: Vec<(String, Mutex<Vec<String>>)>,
    fallback: String,
    prompts: Mutex<Vec<String>>,
}

impl Scripted {
    /// `rules` are `(substring, answers)`; the first rule whose substring the
    /// prompt contains wins, and its next unused answer is returned. A rule
    /// whose queue is exhausted repeats its last answer, so a case never fails
    /// because the script ran short rather than because the languages disagreed.
    fn new(rules: &[(&str, &[&str])], fallback: &str) -> Self {
        Self {
            rules: rules
                .iter()
                .map(|(key, answers)| {
                    (
                        (*key).to_string(),
                        Mutex::new(answers.iter().rev().map(|a| (*a).to_string()).collect()),
                    )
                })
                .collect(),
            fallback: fallback.to_string(),
            prompts: Mutex::new(Vec::new()),
        }
    }

    fn answer(&self, prompt: &str) -> Option<String> {
        self.prompts
            .lock()
            .expect("not poisoned")
            .push(prompt.to_string());
        for (key, queue) in &self.rules {
            if prompt.contains(key.as_str()) {
                let mut queue = queue.lock().expect("not poisoned");
                return Some(if queue.len() > 1 {
                    queue.pop().expect("non-empty")
                } else {
                    queue
                        .last()
                        .cloned()
                        .unwrap_or_else(|| self.fallback.clone())
                });
            }
        }
        Some(self.fallback.clone())
    }

    /// How many model calls this run made — the number the cost claim is about.
    fn firings(&self) -> usize {
        self.prompts.lock().expect("not poisoned").len()
    }

    fn prompts(&self) -> Vec<String> {
        self.prompts.lock().expect("not poisoned").clone()
    }
}

impl Cast for Scripted {
    fn fire(&self, task: &str, _index: usize) -> Option<String> {
        self.answer(task)
    }

    fn check(&self, _task: &str, candidate: &str, cmd: &str) -> bool {
        // A stand-in verifier with no shell in it: the gate "passes" a candidate
        // that contains the command's last word. Enough to exercise the gate's
        // shape without making the test depend on a working `pytest`.
        cmd.split_whitespace()
            .next_back()
            .is_some_and(|want| candidate.contains(want))
    }
}

impl Oracle for Scripted {
    fn fire(&self, prompt: &str) -> Option<String> {
        self.answer(prompt)
    }
}

/// Run a myth source on a task. Returns the answer and the firings it cost.
fn run_myth(source: &str, task: &str, rules: &[(&str, &[&str])]) -> (Option<String>, usize) {
    let node = myth::parse(source).unwrap_or_else(|error| panic!("{source}: {error}"));
    let oracle = Scripted::new(rules, "");
    let answer = myth::run(&node, task, &oracle);
    (answer, oracle.firings())
}

/// Run a Rebis source. Returns the answer and the firings it cost.
///
/// `orchestrate` rather than `orchestrate_parallel`: sequential evaluation makes
/// the draw order from the script deterministic, and concurrency is not what is
/// under test here.
fn run_rebis(source: &str, rules: &[(&str, &[&str])]) -> (Option<String>, usize) {
    let expression = rebis_lang::parse(source).unwrap_or_else(|error| panic!("{source}: {error}"));
    let oracle = Scripted::new(rules, "");
    let mut record = Record::from_texts::<&str>(&[]);
    let result = rebis_lang::orchestrate(&expression, &mut record, &oracle);
    assert!(
        result.diagnostics.is_empty(),
        "{source} reported {:?}",
        result.diagnostics
    );
    (result.output, oracle.firings())
}

/// Assert one myth and its translation agree on the answer and on the price.
fn equivalent(myth_source: &str, task: &str, rebis_source: &str, rules: &[(&str, &[&str])]) {
    let (myth_answer, myth_firings) = run_myth(myth_source, task, rules);
    let (rebis_answer, rebis_firings) = run_rebis(rebis_source, rules);
    assert_eq!(
        myth_answer, rebis_answer,
        "answers differ\n  myth:  {myth_source}\n  rebis: {rebis_source}"
    );
    assert_eq!(
        myth_firings, rebis_firings,
        "firings differ ({myth_firings} vs {rebis_firings})\n  myth:  {myth_source}\n  rebis: {rebis_source}"
    );
}

// ── what does translate ─────────────────────────────────────────────────────

#[test]
fn a_bare_leaf_is_a_written_prompt() {
    // `fire` is the task, sent. In Rebis the task IS the program, so the
    // translation is the prompt itself and the cost is one on both sides.
    equivalent(
        "fire",
        "name the capital of France",
        r#""name the capital of France""#,
        &[("capital", &["Paris"])],
    );
}

#[test]
fn ask_is_a_framing() {
    // `(ask "role")` prepends an instruction to the task. `+` is Rebis's
    // framing: the context reaches every prompt written inside it. Same intent,
    // same one firing — and the prompt TEXT differs, which is recorded in
    // `the_prompt_text_is_not_identical` rather than papered over here.
    equivalent(
        r#"(ask "Answer in one word")"#,
        "name the capital of France",
        r#"(+ "Answer in one word" "name the capital of France")"#,
        &[("capital", &["Paris"])],
    );
}

#[test]
fn pipe_is_the_arrow() {
    // The propose → critique → write pipeline from `docs/myth.md`, with the
    // gathers removed so this case isolates the spine. `->` routes each stage's
    // answer into the next exactly as `pipe` threads it.
    equivalent(
        r#"(pipe (ask "Propose an approach") (ask "Critique it") (ask "Write the final code"))"#,
        "fix the parser",
        r#"(-> (+ "Propose an approach" "fix the parser")
              (+ "Critique it" "fix the parser")
              (+ "Write the final code" "fix the parser"))"#,
        &[
            ("Propose", &["use a pratt parser"]),
            ("Critique", &["it drops the unary case"]),
            ("Write", &["fn parse_unary()"]),
        ],
    );
}

#[test]
fn a_pipe_of_one_stage_is_that_stage() {
    equivalent(
        r#"(pipe (ask "Summarise"))"#,
        "the report",
        r#"(+ "Summarise" "the report")"#,
        &[("Summarise", &["it is long"])],
    );
}

// ── what translates only partly ─────────────────────────────────────────────

#[test]
fn the_mirror_gate_and_a_symbol_mediator_are_not_the_same_gate() {
    // The plan calls these "the same code path", and they do share
    // `holonomy_reflected`. They do not share what it is pointed at.
    //
    //   Gate::Mirror(p)   scores each candidate against THE TASK, and keeps the
    //                     lowest that is also under p/100.
    //   ([symbol] …)      scores each branch against THE MEDIATOR'S OWN WORDS,
    //                     and keeps the lowest under 1.0.
    //
    // So they agree only when the mediator's words happen to be the task and the
    // threshold happens not to bite. Here they disagree, which is the point.
    let rules: &[(&str, &[&str])] = &[("sleep", &["bananas are yellow", "sleep beats commits"])];

    let (myth_answer, myth_firings) = run_myth(
        "(gather (mirror 80) (spread 2 fire))",
        "sleep versus commits",
        rules,
    );
    // The myth gate judges against the task, so the on-topic candidate wins.
    assert_eq!(myth_answer.as_deref(), Some("sleep beats commits"));
    assert_eq!(myth_firings, 2);

    // The nearest Rebis spelling, with the mediator named for the same intent.
    let (rebis_answer, rebis_firings) = run_rebis(
        r#"([consensus] "sleep versus commits" "sleep versus commits")"#,
        rules,
    );
    assert_eq!(
        rebis_firings, 2,
        "the fan-out itself does translate exactly"
    );
    assert_ne!(
        rebis_answer.as_deref(),
        myth_answer.as_deref(),
        "if these ever agree, the gap list below is stale and must be re-derived"
    );
}

#[test]
fn a_symbol_mediator_is_the_same_gate_whatever_word_is_written() {
    // `[consensus]`, `[first]` and `[vote]` are not three gates. Rebis has no
    // table of mediator names: any pure-symbol head is scored the same way, so
    // the word only changes what the branches are compared AGAINST.
    let rules: &[(&str, &[&str])] = &[("q", &["alpha alpha alpha", "beta"])];

    let consensus = run_rebis(r#"([consensus] "q" "q")"#, rules);
    let vote = run_rebis(r#"([vote] "q" "q")"#, rules);
    let first = run_rebis(r#"([first] "q" "q")"#, rules);

    assert_eq!(consensus.1, 2);
    assert_eq!(vote.1, 2);
    assert_eq!(first.1, 2);
    // None of the three is positional and none of the three counts ballots. The
    // modal answer here is "alpha alpha alpha" (it is not — there is no mode with
    // two distinct candidates), and `[first]` does not mean "the first one".
    assert_eq!(
        first.0, vote.0,
        "[first] and [vote] pick alike, so neither is the gate its name suggests"
    );
}

// ── what does not translate, and why ────────────────────────────────────────

/// The deliverable of a proof that failed: every myth gate with no Rebis
/// spelling, and the reason.
///
/// Asserted as data rather than written in prose so that it cannot quietly go
/// stale. When A2 lands a host mediator for one of these, its row comes out of
/// this list and a passing equivalence case goes in above.
#[test]
fn the_gate_gap_list() {
    let gaps: BTreeMap<&str, &str> = [
        (
            "Gate::Vote",
            "the modal candidate by exact string equality. Rebis has no mediator \
             that counts ballots — a symbol mediator scores round-trip holonomy \
             against its own words, so `[consensus]` prefers whichever branch \
             reads most like the word `consensus` and never notices that four \
             branches said the same thing. This is the gate with the measured \
             number behind it (+23pts AIME2025, solve.rs), so losing it silently \
             is the worst outcome available.",
        ),
        (
            "Gate::First",
            "the first non-empty candidate. Positional selection. `[first]` is a \
             symbol mediator like any other and is not positional; `%` selects \
             between two written branches by a condition and cannot mean \
             `whichever of these N came back`.",
        ),
        (
            "Gate::Check(cmd)",
            "run a shell verifier over each candidate, keep the first survivor. \
             The one gap the plan predicted, and the only one that needs \
             authority as well as a spelling. A2.",
        ),
    ]
    .into_iter()
    .collect();

    assert_eq!(
        gaps.keys().copied().collect::<Vec<_>>(),
        vec!["Gate::Check(cmd)", "Gate::First", "Gate::Vote"],
        "the gap list changed; update the module docs and A2's scope with it"
    );

    // And the one that translates only partly, kept beside them so the shape of
    // the remaining work is one list rather than two.
    assert!(
        !gaps.contains_key("Gate::Mirror"),
        "Mirror has a near-spelling — see the_mirror_gate_and_a_symbol_mediator_are_not_the_same_gate"
    );
}

/// The examples in `docs/myth.md` that cannot be translated, named individually.
///
/// A1's acceptance criterion asks for exactly this: every documented example
/// either has an answer-identical, firing-identical translation above, or is
/// named here with the reason.
#[test]
fn the_untranslatable_documented_examples() {
    let blocked: Vec<(&str, &str)> = vec![
        (
            "(gather vote (spread 8 fire))",
            "the conclave, and the default `KAOS_MYTH`. Blocked on Gate::Vote.",
        ),
        (
            "(gather first (spread 3 fire))",
            "cheapest good answer. Blocked on Gate::First.",
        ),
        (
            r#"(gather (check "lake env lean Attempt.lean") (spread 8 fire))"#,
            "verified best-of-k. Blocked on Gate::Check.",
        ),
        (
            r#"(gather (check "pytest -q tests/") (spread 16 fire))"#,
            "a wide net into a strict gate. Blocked on Gate::Check.",
        ),
        (
            "(gather vote (spread 4 (gather (check \"…\") (spread 4 fire))))",
            "the bowtie. Blocked on Gate::Vote and Gate::Check together.",
        ),
        (
            "(pipe (gather vote (spread 5 (ask \"Propose\"))) (ask \"Critique\") \
             (gather (check \"pytest -q\") (spread 3 (ask \"Write\"))))",
            "the propose/critique/write pipeline. Its SPINE translates — see \
             pipe_is_the_arrow — and only its gates do not.",
        ),
        (
            "the five-stage bug-fix pipeline",
            "same shape at greater length: `->` carries the spine, and \
             Gate::Vote and Gate::Check block stages 1, 3 and 5.",
        ),
    ];

    // Every reason must name the gate that blocks it, so the list stays a work
    // item rather than a lament.
    for (example, reason) in &blocked {
        assert!(
            reason.contains("Gate::") || reason.contains("gates"),
            "{example}: a blocked example must name what blocks it"
        );
    }
    assert_eq!(blocked.len(), 7);
}

// ── the caveat the scripted oracle cannot see ───────────────────────────────

#[test]
fn the_prompt_text_is_not_identical() {
    // A scripted oracle answers the same for both languages, so the cases above
    // prove same answer and same price — NOT same prompt. Two differences are
    // real, and a model would see them:
    //
    //   ask     myth sends "role\n\ntask"; Rebis's `+` frames its own way.
    //   pipe    myth appends "Work so far:\n{answer}" to the ORIGINAL task;
    //           Rebis's `->` delivers the previous answer as "INPUT:", which is
    //           one of the two strings its purity test allows the runtime to
    //           author at all.
    //
    // Recorded here as an executable observation rather than a footnote, since
    // A4 will otherwise retire `myth` on the strength of an equivalence that was
    // only ever measured through an oracle that could not read.
    let rules: &[(&str, &[&str])] = &[("Propose", &["a draft"]), ("Refine", &["a final"])];

    let node = myth::parse(r#"(pipe (ask "Propose") (ask "Refine"))"#).expect("parses");
    let myth_oracle = Scripted::new(rules, "");
    myth::run(&node, "the task", &myth_oracle);
    let myth_second = myth_oracle.prompts()[1].clone();

    let expression = rebis_lang::parse(r#"(-> (+ "Propose" "the task") (+ "Refine" "the task"))"#)
        .expect("parses");
    let rebis_oracle = Scripted::new(rules, "");
    let mut record = Record::from_texts::<&str>(&[]);
    let _ = rebis_lang::orchestrate(&expression, &mut record, &rebis_oracle);
    let rebis_second = rebis_oracle.prompts()[1].clone();

    assert!(
        myth_second.contains("Work so far:"),
        "myth's own wording: {myth_second}"
    );
    assert!(
        !rebis_second.contains("Work so far:"),
        "Rebis authors no such text: {rebis_second}"
    );
    assert_ne!(
        myth_second, rebis_second,
        "the two languages do not send the same prompt, and no scripted oracle can tell"
    );
}

// ── what the fan-out itself costs ───────────────────────────────────────────

#[test]
fn the_fan_out_translates_exactly_even_where_the_gate_does_not() {
    // Worth pinning separately, because it is the half of the mapping that DOES
    // hold: `(spread N X)` and an N-branch square fire N times, and `myth`'s own
    // `leaves()` agrees with what the run actually spends. Only the collapse is
    // in dispute.
    for n in [1_usize, 2, 5, 8] {
        let branches = vec![r#""q""#; n].join(" ");
        let (_, rebis_firings) = run_rebis(&format!("([m] {branches})"), &[("q", &["a"])]);
        let (_, myth_firings) = run_myth(
            &format!("(gather vote (spread {n} fire))"),
            "q",
            &[("q", &["a"])],
        );
        assert_eq!(myth_firings, n);
        assert_eq!(rebis_firings, n);
        assert_eq!(
            myth::parse(&format!("(gather vote (spread {n} fire))"))
                .expect("parses")
                .leaves(),
            n
        );
    }
}
