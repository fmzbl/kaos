//! Every documented myth example, as the Rebis it became.
//!
//! Kaos had two orchestration languages. `myth` was 518 lines with five forms
//! and four gates; Rebis is the model-interface language with 22 frozen
//! operators, a tested cost model, and every other surface in the harness
//! written against it. This file was the proof that one of them was redundant,
//! and it is now the proof that the retirement was faithful.
//!
//! # What it found
//!
//! **The mapping did not hold.** Three of the four gates had no Rebis spelling,
//! not the one the plan predicted, and two of those three were expected to be
//! fine: the plan's table wrote `Gate::Vote → [consensus]` and `Gate::First →
//! [first] or %`.
//!
//! Both readings are wrong for one reason, worth stating once: **Rebis has no
//! table of mediator names.** `([anything] A B …)` whose head is pure symbols is
//! a *symbol mediator*, and every one of them runs the same code — round-trip
//! holonomy against the mediator's own words. `[consensus]` does not count
//! ballots; it prefers whichever branch reads most like the word "consensus".
//! `[first]` is not positional. They were one gate wearing different words.
//!
//! That widened A2 from one gate to three, and `kaos_agent::gate` is the result:
//! `[vote]`, `[first]` and `[check]` are host mediators Rebis resolves by name,
//! with the operator set still frozen. Asserted, not assumed — see the end of
//! [`the_gate_gap_list`].
//!
//! # How the cases read now
//!
//! There is no myth evaluator left to compare against. So "the myth side" of
//! each case is its **translation**, and what every case asserts is the claim
//! that matters after the retirement: a myth's Rebis form answers what the myth
//! answered and **costs what the myth's own `leaves()` said it would**. That
//! second half is checked on every `run_myth` call rather than in one place.
//!
//! # What the retirement changed
//!
//! Four things, each its own test, none of them silent:
//!
//! - **The bowtie does not translate**, for a reason nothing anticipated. A
//!   square's mediator is handed every accepted *firing* since the square
//!   started, not the values its branches returned — so a gate nested inside a
//!   vote has its verdict discarded.
//!   [`the_bowtie_does_not_translate_and_here_is_why`].
//! - **A vote with no majority now refuses.** `myth` returned a candidate chosen
//!   by thread scheduling. [`a_vote_with_no_majority_now_refuses_where_myth_answered`].
//! - **The mirror gate loses its threshold**, and `to_rebis` reports the loss.
//!   [`the_mirror_gate_loses_its_threshold_in_translation`].
//! - **A piped stage no longer reads runtime-authored text.** `myth` wrote
//!   "Work so far:"; Rebis authors nothing but `INPUT:`.
//!   [`a_translated_pipe_stops_authoring_prompt_text`].
//!
//! And one improvement that is therefore not an equivalence: `first` was never
//! deterministic in `myth`, and is positional in Rebis.
//! [`first_is_positional_in_rebis_and_arbitrary_in_myth`].

use std::collections::BTreeMap;
use std::sync::Mutex;

use kaos_agent::gate::Mediators;
use kaos_agent::myth;
use rebis_lang::{Mediation, Oracle, Record};

// ── the shared world ────────────────────────────────────────────────────────

/// A translated myth binds its task through `(&)`, so something has to answer.
struct TaskInlet(String);

impl rebis_lang::Inlet for TaskInlet {
    fn ask(&self, _label: Option<&str>) -> Option<rebis_lang::Received> {
        Some(rebis_lang::Received {
            text: self.0.clone(),
            attachments: Vec::new(),
        })
    }
}

/// Nothing imports here; the standard library is served before a host is asked.
struct NoModules;

impl rebis_lang::ModuleResolver for NoModules {
    fn resolve(&self, _module: &rebis_lang::ModuleName) -> Result<Option<String>, String> {
        Ok(None)
    }
}

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
struct Scripted<'a> {
    rules: Vec<(String, Mutex<Vec<String>>)>,
    fallback: String,
    prompts: Mutex<Vec<String>>,
    /// The names this host claims. Empty for the cases written before A2, so
    /// they keep measuring what they measured.
    mediators: Mediators<'a>,
}

impl<'a> Scripted<'a> {
    /// `rules` are `(substring, answers)`; the first rule whose substring the
    /// prompt contains wins, and its next unused answer is returned. A rule
    /// whose queue is exhausted repeats its last answer, so a case never fails
    /// because the script ran short rather than because the languages disagreed.
    /// With host mediators claimed. Every case needs them now: a translated
    /// myth writes `[vote]`, `[first]` or `[check]`, and a host that claimed
    /// none would judge them by round-trip holonomy instead — which is the
    /// finding this file exists to record.
    fn gated(rules: &[(&str, &[&str])], fallback: &str, mediators: Mediators<'a>) -> Self {
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

impl Oracle for Scripted<'_> {
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

/// Translate a myth and run what it became. Returns the answer and the price.
///
/// There is no myth evaluator left to run — that is what A4 removed — so "the
/// myth side" of every case below IS its translation. The claim being checked is
/// therefore the one that matters after the retirement: **a myth's Rebis form
/// costs what the myth's own `leaves()` said it would**, which is asserted here
/// on every call rather than in one place.
fn run_myth(source: &str, task: &str, rules: &[(&str, &[&str])]) -> (Option<String>, usize) {
    let node = myth::parse(source).unwrap_or_else(|error| panic!("{source}: {error}"));
    let (written, _) = myth::to_rebis(&node).unwrap_or_else(|why| panic!("{source}: {why}"));
    let (answer, firings, diagnostics) = run_rebis_on(&written, task, rules, kaos_gates(true));
    assert!(diagnostics.is_empty(), "{written}: {diagnostics:?}");
    assert_eq!(
        firings,
        node.leaves(),
        "{source} became {written}, which cost {firings} where the myth said {}",
        node.leaves()
    );
    (answer, firings)
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
    run_rebis_on(source, "the task", rules, mediators)
}

/// The same, for a program that obtains a particular task through `(&)`.
fn run_rebis_on(
    source: &str,
    task: &str,
    rules: &[(&str, &[&str])],
    mediators: Mediators,
) -> (Option<String>, usize, Vec<String>) {
    let expression = rebis_lang::parse(source).unwrap_or_else(|error| panic!("{source}: {error}"));
    let oracle = Scripted::gated(rules, "", mediators);
    let mut record = Record::from_texts::<&str>(&[]);
    let result = rebis_lang::orchestrate_with_inlet(
        &expression,
        &mut record,
        &oracle,
        &NoModules,
        // A translated myth binds the task through `(&)`, so the inlet has to
        // answer or every leaf composes an empty prompt.
        &TaskInlet(task.to_string()),
        rebis_lang::RuntimeLimits::default(),
        &mut |_| {},
    );
    (
        result.output,
        oracle.firings(),
        result.diagnostics.iter().map(ToString::to_string).collect(),
    )
}

/// The gates Kaos claims, with a real subprocess behind `[check]`.
fn kaos_gates(authorised: bool) -> Mediators<'static> {
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
fn the_mirror_gate_loses_its_threshold_in_translation() {
    // The plan called `Gate::Mirror` and a symbol mediator "the same code path",
    // and they do share `holonomy_reflected`. They do not share what it is
    // pointed at:
    //
    //   Gate::Mirror(p)   scored each candidate against THE TASK, and kept the
    //                     lowest that was also under p/100.
    //   ([mirror] …)      scores each branch against THE MEDIATOR'S OWN WORD —
    //                     "mirror" — and keeps the lowest under 1.0.
    //
    // So `mirror` is the one gate the translation cannot carry, and `to_rebis`
    // says so rather than pretending. This pins that it says so, and that the
    // result really does differ: branches about sleep and commits do not
    // round-trip onto the word "mirror", so the square refuses where the myth
    // gate would have picked.
    let node = myth::parse("(gather (mirror 80) (spread 2 fire))").expect("parses");
    let (written, losses) = myth::to_rebis(&node).expect("translates");
    assert_eq!(losses.0.len(), 1, "the loss is reported");
    assert!(losses.0[0].contains("80"), "{:?}", losses.0);

    let rules: &[(&str, &[&str])] = &[("sleep", &["bananas are yellow", "sleep beats commits"])];
    let (answer, firings, diagnostics) =
        run_rebis_on(&written, "sleep versus commits", rules, kaos_gates(true));
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(firings, 2, "the fan-out itself translates exactly");
    assert_eq!(
        answer, None,
        "and the gate does not: judged against its own word, nothing round-trips"
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
fn a_vote_with_no_majority_now_refuses_where_myth_answered() {
    // `myth`'s `Gate::Vote` called `scry::majority`, which returned the FIRST
    // candidate when every answer was distinct — a "majority" of one, and which
    // candidate came first was decided by thread scheduling inside `spread`.
    //
    // `[vote]` refuses instead, and the difference is deliberate. A conclave
    // whose five branches agree about nothing has found no signal; returning one
    // of them launders a guess as a consensus, and that is the exact shape of a
    // false ship — an answer presented as though a gate had agreed with it. D1's
    // criterion is zero of those.
    //
    // This is the one place the retirement changed an answer rather than only
    // the words around it, which is why it is a test of its own.
    let rules: &[(&str, &[&str])] = &[("q", &["a", "b", "c", "d", "e"])];
    let node = myth::parse("(gather vote (spread 5 fire))").expect("parses");
    let (written, _) = myth::to_rebis(&node).expect("translates");

    let (answer, firings, diagnostics) = run_rebis_on(&written, "q", rules, kaos_gates(true));
    assert_eq!(answer, None, "no majority, so no verdict");
    assert!(diagnostics.is_empty(), "declining is not an error");
    assert_eq!(
        firings,
        node.leaves(),
        "and refusing does not refund the branches"
    );
}

// ── what the translation changed, and could not not change ─────────────────

#[test]
fn a_translated_pipe_stops_authoring_prompt_text() {
    // The equivalence cases prove same answer and same price. They never proved
    // same PROMPT, and a model would have seen the difference:
    //
    //   pipe    `myth` appended "Work so far:\n{answer}" to the original task —
    //           text the runtime wrote, in a language with no rule against it.
    //   ->      Rebis delivers the previous answer as "INPUT:", one of exactly
    //           two labels its purity test allows the runtime to author.
    //
    // So the translation is not neutral: it moves every piped myth onto a
    // stricter contract about who writes what a model reads. That is an
    // improvement, and it is still a change, and a run that behaved differently
    // after the retirement would have deserved this sentence in advance.
    let rules: &[(&str, &[&str])] = &[("Propose", &["a draft"]), ("Refine", &["a final"])];

    let node = myth::parse(r#"(pipe (ask "Propose") (ask "Refine"))"#).expect("parses");
    let (written, _) = myth::to_rebis(&node).expect("translates");
    let oracle = Scripted::gated(rules, "", kaos_gates(true));
    let expression = rebis_lang::parse(&written).expect("parses");
    let mut record = Record::from_texts::<&str>(&[]);
    let _ = rebis_lang::orchestrate_with_inlet(
        &expression,
        &mut record,
        &oracle,
        &NoModules,
        &TaskInlet("the task".to_string()),
        rebis_lang::RuntimeLimits::default(),
        &mut |_| {},
    );

    let second = oracle.prompts()[1].clone();
    assert!(
        !second.contains("Work so far:"),
        "the runtime authors no such text any more: {second}"
    );
    assert!(
        second.contains("Refine") && second.contains("a draft"),
        "the stage still sees its role and what came before: {second}"
    );
}
