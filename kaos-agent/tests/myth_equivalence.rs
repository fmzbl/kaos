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
//! **The mapping did not hold.** Three of the four gates had no Rebis spelling,
//! not one, and two of those three were expected to be fine: the plan's own
//! table wrote `Gate::Vote → [consensus]` and `Gate::First → [first] or %`.
//!
//! Both readings are wrong for one reason, worth stating once: **Rebis has no
//! table of mediator names.** `([anything] A B …)` whose head is pure symbols is
//! a *symbol mediator*, and every one of them runs the same code —
//! `resolve_mediation`, which scores each branch's round-trip holonomy against
//! the mediator's own words and keeps the lowest. `[consensus]` does not count
//! ballots; it prefers whichever branch reads most like the word "consensus".
//! `[first]` is not positional. They were one gate wearing different words.
//!
//! # What that changed
//!
//! Not the direction — the size of A2. The plan proposed a host-supplied
//! mediator so `check` could exist without a 23rd operator; the same seam
//! answered all three. `[vote]`, `[first]` and `[check]` are now host mediators
//! resolved by name ([`kaos_agent::gate`]), and the operator set stayed frozen
//! because to Rebis they are symbol mediators like any other. That is asserted,
//! not assumed: see the end of [`the_gate_gap_list`].
//!
//! # What is still not equivalent, after all of it
//!
//! Three things, each its own test, and A4 must not claim otherwise:
//!
//! - **The bowtie does not translate**, for a reason nothing anticipated. A
//!   square's mediator is handed every accepted *firing* since the square
//!   started, not the values its branches returned — so a gate nested inside a
//!   vote has its verdict discarded. See
//!   [`the_bowtie_does_not_translate_and_here_is_why`].
//! - **`first` was never deterministic in `myth`.** Its `spread` races for call
//!   indices, so `first` means "an arbitrary one of the N". Rebis's is
//!   positional, which is better and therefore not the same.
//! - **A vote with no majority.** `myth` answers with the first candidate; Kaos
//!   refuses. Deliberate — see
//!   [`kaos_refuses_a_vote_that_myth_would_have_answered`] — and the shape D1's
//!   abstention attaches to.
//!
//! And one the scripted oracle cannot see at all: the two languages do not send
//! the same prompt text. [`the_prompt_text_is_not_identical`].

use std::collections::BTreeMap;
use std::sync::Mutex;

use kaos_agent::gate::Mediators;
use kaos_agent::myth::{self, Cast};
use rebis_lang::{Mediation, Oracle, Record};

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
    /// The names this host claims. Empty for the cases written before A2, so
    /// they keep measuring what they measured.
    mediators: Mediators,
}

impl Scripted {
    /// `rules` are `(substring, answers)`; the first rule whose substring the
    /// prompt contains wins, and its next unused answer is returned. A rule
    /// whose queue is exhausted repeats its last answer, so a case never fails
    /// because the script ran short rather than because the languages disagreed.
    fn new(rules: &[(&str, &[&str])], fallback: &str) -> Self {
        Self::gated(rules, fallback, Mediators::none())
    }

    /// The same, with host mediators claimed.
    fn gated(rules: &[(&str, &[&str])], fallback: &str, mediators: Mediators) -> Self {
        Self {
            mediators,
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

    /// The three names Kaos claims, so a translation can use them.
    ///
    /// The gate is `grep -q pass`, which is a real subprocess reading the
    /// candidate on stdin — the shell gate exercised as a shell gate rather than
    /// as a stub, since being a subprocess is the whole of what it adds.
    fn mediate(&self, mediator: &str, branches: &[String]) -> Mediation {
        self.mediators.judge(mediator, branches)
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
    let (answer, firings, diagnostics) = run_rebis_gated(source, rules, Mediators::none());
    assert!(diagnostics.is_empty(), "{source} reported {diagnostics:?}");
    (answer, firings)
}

/// The same, under a host that claims mediator names, and keeping whatever the
/// run reported — an unauthorised gate is supposed to report.
fn run_rebis_gated(
    source: &str,
    rules: &[(&str, &[&str])],
    mediators: Mediators,
) -> (Option<String>, usize, Vec<String>) {
    let expression = rebis_lang::parse(source).unwrap_or_else(|error| panic!("{source}: {error}"));
    let oracle = Scripted::gated(rules, "", mediators);
    let mut record = Record::from_texts::<&str>(&[]);
    let result = rebis_lang::orchestrate(&expression, &mut record, &oracle);
    (
        result.output,
        oracle.firings(),
        result.diagnostics.iter().map(ToString::to_string).collect(),
    )
}

/// The gates Kaos claims, with a real subprocess behind `[check]`.
fn kaos_gates(authorised: bool) -> Mediators {
    Mediators::standard(
        "grep -q pass".to_string(),
        std::time::Duration::from_secs(10),
        authorised,
    )
}

/// Assert a myth and its now-spellable translation agree, under Kaos's gates.
fn equivalent_gated(myth_source: &str, task: &str, rebis_source: &str, rules: &[(&str, &[&str])]) {
    let (myth_answer, myth_firings) = run_myth(myth_source, task, rules);
    let (rebis_answer, rebis_firings, diagnostics) =
        run_rebis_gated(rebis_source, rules, kaos_gates(true));
    assert!(diagnostics.is_empty(), "{rebis_source}: {diagnostics:?}");
    assert_eq!(
        myth_answer, rebis_answer,
        "answers differ\n  myth:  {myth_source}\n  rebis: {rebis_source}"
    );
    assert_eq!(
        myth_firings, rebis_firings,
        "firings differ ({myth_firings} vs {rebis_firings})"
    );
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
    // `[consensus]`, `[first]` and `[vote]` are not three gates ON A HOST THAT
    // CLAIMS NOTHING, which is what `run_rebis` uses. Rebis has no table of
    // mediator names: any pure-symbol head is scored the same way, so the word
    // only changes what the branches are compared AGAINST. This is the property
    // that lets Kaos claim three of those words without the language acquiring
    // an opinion about them — see `the_gate_gap_list`.
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

// ── the gaps, and what closed them ──────────────────────────────────────────

/// Every myth gate Rebis could not spell, and what it is spelled with now.
///
/// This started as the deliverable of a proof that failed. A2 answered it: the
/// host-mediator seam the plan proposed for `check` alone turned out to be the
/// answer to all three, so each row now carries a spelling instead of a reason.
/// Asserted as data rather than written in prose so it cannot go stale.
#[test]
fn the_gate_gap_list() {
    // gate → (what Rebis lacks on its own, how Kaos supplies it)
    let gaps: BTreeMap<&str, (&str, &str)> = [
        (
            "Gate::Vote",
            (
                "no mediator counts ballots — a symbol mediator scores round-trip \
                 holonomy against its own words, so `[consensus]` prefers whichever \
                 branch reads most like the word `consensus` and never notices that \
                 four branches said the same thing",
                "[vote], a host mediator — kaos_agent::gate::Vote",
            ),
        ),
        (
            "Gate::First",
            (
                "no mediator is positional; `%` selects between two written branches \
                 by a condition and cannot mean `whichever of these N came back`",
                "[first], a host mediator — kaos_agent::gate::First",
            ),
        ),
        (
            "Gate::Check(cmd)",
            (
                "no mediator reaches outside the run, which is the one gap the plan \
                 predicted and the only one needing authority as well as a spelling",
                "[check], a host mediator under --allow-tools — kaos_agent::gate::Check",
            ),
        ),
    ]
    .into_iter()
    .collect();

    assert_eq!(
        gaps.keys().copied().collect::<Vec<_>>(),
        vec!["Gate::Check(cmd)", "Gate::First", "Gate::Vote"],
        "the gap list changed; update the module docs and A2's scope with it"
    );
    for (gate, (_, spelling)) in &gaps {
        assert!(
            spelling.starts_with('['),
            "{gate} still has no spelling, so A4 cannot retire myth.rs"
        );
    }
    // And no operator was added to close any of them, which is checkable
    // rather than merely asserted: `[check]` is an ordinary mediator square, so
    // it parses and re-prints identically to `[merger]`'s shape whether or not
    // any host claims the word. A new operator could not do that.
    let claimed = rebis_lang::parse(r#"([check] "a" "b")"#).expect("parses");
    let ordinary = rebis_lang::parse(r#"([merger] "a" "b")"#).expect("parses");
    assert_eq!(
        rebis_lang::format(&claimed).replace("check", "merger"),
        rebis_lang::format(&ordinary),
        "a claimed name must be the same syntax as any other symbol mediator"
    );
}

// ── the documented examples, now that the gates exist ───────────────────────

#[test]
fn the_conclave_translates() {
    // `(gather vote (spread 8 fire))` — the default `KAOS_MYTH`, and the
    // headline of `docs/myth.md`. Eight branches, five say 42, and both
    // languages find the majority for eight firings.
    let rules: &[(&str, &[&str])] = &[(
        "the answer",
        &["42", "7", "42", "13", "42", "42", "7", "42"],
    )];
    equivalent_gated(
        "(gather vote (spread 8 fire))",
        "the answer",
        &format!("([vote] {})", [r#""the answer""#; 8].join(" ")),
        rules,
    );
}

#[test]
fn first_is_positional_in_rebis_and_arbitrary_in_myth() {
    // `(gather first (spread 3 fire))` — and the one documented example whose
    // two sides cannot be made answer-identical, for a reason that is a finding
    // rather than a translation bug.
    //
    // `myth`'s `spread` fans out on scoped threads which take their call indices
    // from an atomic, so which answer lands in which slot of the candidate list
    // is decided by scheduling. `first` then takes the head of that list. It has
    // therefore never meant "the first branch on the page" — it means "an
    // arbitrary one of the N that came back", and reads as deterministic only
    // because nothing ever looked.
    //
    // Rebis's square evaluates in source order, so `[first]` is positional and
    // says which one it will be before it runs. That is a better gate wearing
    // the same name, and pretending the two agree would hide the improvement.
    let rules: &[(&str, &[&str])] = &[("q", &["one", "two", "three"])];

    let (rebis_answer, rebis_firings, diagnostics) =
        run_rebis_gated(r#"([first] "q" "q" "q")"#, rules, kaos_gates(true));
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(
        rebis_answer.as_deref(),
        Some("one"),
        "positional: the first branch in source order"
    );

    let (myth_answer, myth_firings) = run_myth("(gather first (spread 3 fire))", "q", rules);
    let myth_answer = myth_answer.expect("myth answers");
    assert!(
        ["one", "two", "three"].contains(&myth_answer.as_str()),
        "myth returns SOME candidate — which one is not a property of the myth: {myth_answer}"
    );

    // The price is identical, which is the half of the mapping that does hold.
    assert_eq!(myth_firings, rebis_firings);
    assert_eq!(rebis_firings, 3);
}

#[test]
fn verified_best_of_k_translates_and_runs_a_real_subprocess() {
    // `(gather (check "…") (spread 8 fire))` — the shape the whole gate exists
    // for. The Rebis side runs `grep -q pass` per candidate: an actual
    // subprocess reading the candidate on stdin, since being a subprocess is
    // the entire content of what this gate adds over the calculus.
    let rules: &[(&str, &[&str])] = &[("q", &["nope", "nope", "this one is pass", "nope"])];
    let (answer, firings, diagnostics) =
        run_rebis_gated(r#"([check] "q" "q" "q" "q")"#, rules, kaos_gates(true));

    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(answer.as_deref(), Some("this one is pass"));
    assert_eq!(firings, 4, "the branches, and nothing for the gate");
}

#[test]
fn a_gate_that_passes_nothing_answers_nothing() {
    // The outcome D1 attaches to: the work was done, the gate refused it, and
    // the run has an answer neither shipped nor failed.
    let rules: &[(&str, &[&str])] = &[("q", &["nope", "also nope"])];
    let (answer, firings, diagnostics) =
        run_rebis_gated(r#"([check] "q" "q")"#, rules, kaos_gates(true));

    assert_eq!(answer, None);
    assert_eq!(firings, 2, "refusing does not refund the branches");
    assert!(
        diagnostics.is_empty(),
        "a refusal is not an error: {diagnostics:?}"
    );
}

#[test]
fn the_gate_is_refused_without_command_authority() {
    // R7, and A2's third acceptance criterion. A program naming `[check]` in a
    // run that holds no command authority must be refused AT RESOLUTION —
    // reported, answering nothing — rather than quietly answering as though it
    // had been verified.
    let rules: &[(&str, &[&str])] = &[("q", &["this one is pass", "nope"])];
    let (answer, firings, diagnostics) =
        run_rebis_gated(r#"([check] "q" "q")"#, rules, kaos_gates(false));

    assert_eq!(
        answer, None,
        "the candidate that WOULD have passed must not be returned"
    );
    assert_eq!(firings, 2);
    let reported = diagnostics.join(" · ");
    assert!(
        reported.contains("--allow-tools"),
        "the refusal must say what is missing: {reported}"
    );
}

#[test]
fn the_bowtie_does_not_translate_and_here_is_why() {
    // `docs/myth.md`'s nested shape — gate small batches, vote across the
    // verified winners — is the one example that stays blocked after A2, and the
    // obstacle is not a missing gate. It is what a Rebis mediator is shown.
    //
    // **A square's mediator is handed every accepted FIRING since the square
    // started, not the values its branches returned.** For a flat square those
    // are the same list and nothing notices. For a nested one they are not: the
    // outer mediator sees all four leaves, so the inner gates' verdicts are
    // discarded and gating-then-voting silently becomes voting-over-everything.
    //
    // This is a property of the language, found here and recorded rather than
    // fixed: changing what `accepted_since` collects would change the meaning of
    // every nested square already written. A4 must not claim the bowtie
    // translates, and a program wanting one should write the stages as an arrow.
    let rules: &[(&str, &[&str])] = &[("q", &["nope", "a pass", "nope", "a pass"])];
    let (answer, firings, diagnostics) = run_rebis_gated(
        r#"([vote] ([check] "q" "q") ([check] "q" "q"))"#,
        rules,
        kaos_gates(true),
    );

    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(firings, 4, "the leaves are paid for either way");
    // Both inner gates pass "a pass", so a bowtie would answer "a pass". The
    // outer vote instead sees ["nope", "a pass", "nope", "a pass"], finds a tie,
    // and takes the earliest — which is a candidate NO gate accepted.
    assert_eq!(
        answer.as_deref(),
        Some("nope"),
        "if this ever answers `a pass`, nested mediation changed and the bowtie \
         may now translate — re-derive this test rather than deleting it"
    );

    // Written as an arrow instead, each gate's verdict survives, because a stage
    // boundary is what keeps one square's firings out of the next one's view.
    let rules: &[(&str, &[&str])] = &[("q", &["nope", "a pass"]), ("r", &["nope", "a pass"])];
    let (piped, _, diagnostics) = run_rebis_gated(
        r#"(-> ([check] "q" "q") ([check] "r" "r"))"#,
        rules,
        kaos_gates(true),
    );
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(piped.as_deref(), Some("a pass"));
}

#[test]
fn the_pipeline_translates_whole() {
    // The propose → critique → write pipeline from `docs/myth.md`, gates and
    // all: a voted first stage, a bare middle, a gated last. It was the last
    // documented example still blocked, and nothing in it is blocked now.
    let rules: &[(&str, &[&str])] = &[
        (
            "Propose",
            &["use a pratt parser", "rewrite it", "use a pratt parser"],
        ),
        ("Critique", &["it drops the unary case"]),
        ("Write", &["broken", "fn parse_unary() // pass"]),
    ];
    let (answer, firings, diagnostics) = run_rebis_gated(
        r#"(-> ([vote] (+ "Propose an approach" "fix it")
                      (+ "Propose an approach" "fix it")
                      (+ "Propose an approach" "fix it"))
             (+ "Critique it" "fix it")
             ([check] (+ "Write the final code" "fix it")
                      (+ "Write the final code" "fix it")))"#,
        rules,
        kaos_gates(true),
    );

    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(answer.as_deref(), Some("fn parse_unary() // pass"));
    assert_eq!(firings, 3 + 1 + 2);
}

// ── one deliberate divergence ───────────────────────────────────────────────

#[test]
fn kaos_refuses_a_vote_that_myth_would_have_answered() {
    // `myth`'s `Gate::Vote` calls `scry::majority`, which returns the FIRST
    // candidate when every answer is distinct — a "majority" of one. Kaos's
    // `[vote]` refuses instead, and the difference is deliberate rather than an
    // oversight in the translation.
    //
    // A conclave whose five branches agree about nothing has found no signal.
    // Returning the first launders a single guess as a consensus, and it is the
    // exact shape of a false ship: an answer presented as though a gate had
    // agreed with it. D1's criterion is zero of those.
    let rules: &[(&str, &[&str])] = &[("q", &["a", "b", "c", "d", "e"])];

    let (myth_answer, myth_firings) = run_myth("(gather vote (spread 5 fire))", "q", rules);
    let myth_answer = myth_answer.expect("myth answers anyway");
    // WHICH candidate it returns is not asserted, and the reason is the point:
    // `spread` races for call indices, so the "winner" of a tie is whichever
    // answer scheduling happened to put in slot zero. An answer chosen by thread
    // ordering and reported as a consensus is the failure mode in miniature.
    assert!(
        ["a", "b", "c", "d", "e"].contains(&myth_answer.as_str()),
        "myth returns some arbitrary candidate: {myth_answer}"
    );

    let (kaos_answer, kaos_firings, diagnostics) =
        run_rebis_gated(r#"([vote] "q" "q" "q" "q" "q")"#, rules, kaos_gates(true));
    assert_eq!(kaos_answer, None, "and Kaos declines to");
    assert!(diagnostics.is_empty(), "declining is not an error");
    assert_eq!(
        myth_firings, kaos_firings,
        "both still pay for five branches"
    );
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
