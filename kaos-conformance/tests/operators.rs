//! Every operator, run against a real model.
//!
//! These are `#[ignore]`d: they need ollama running with the model pulled, and
//! a suite that fails on a laptop without it would be noise rather than a
//! signal. Run them deliberately:
//!
//! ```text
//! cargo test -p kaos-conformance -- --ignored --test-threads=1 --nocapture
//! ```
//!
//! Single-threaded because a 4B model on one machine is one model, and eight
//! tests racing for it measure the queue rather than the language.

use kaos_conformance::{
    assert_clean, decisions, remembers, run, run_recording, run_with, unavailable, Wired,
};

/// One prompt, one firing. Everything else is measured against this.
#[test]
#[ignore = "needs a local model"]
fn a_prompt_is_one_call() {
    let (result, model) = run("01-prompt.rebis");
    assert_clean("01-prompt", &result);
    assert_eq!(model.calls(), 1);
    assert!(result.output.is_some(), "a prompt produced no answer");
    // The prompt reaches the model as the program wrote it — the runtime
    // authors no prompt text of its own.
    assert_eq!(model.prompts()[0], "Answer with exactly the word: alpha");
}

/// A group runs in source order and answers with its last form.
#[test]
#[ignore = "needs a local model"]
fn a_group_runs_in_order_and_answers_with_its_last() {
    let (result, model) = run("02-group.rebis");
    assert_clean("02-group", &result);
    assert_eq!(model.calls(), 2);
    let prompts = model.prompts();
    assert!(prompts[0].contains("first"), "{prompts:?}");
    assert!(prompts[1].contains("last"), "{prompts:?}");
    // Neither prompt saw the other: a group sequences, it does not route.
    assert!(!prompts[1].contains("INPUT:"), "{prompts:?}");
    assert!(result.output.is_some());
}

/// An arrow routes each answer into the next stage.
#[test]
#[ignore = "needs a local model"]
fn an_arrow_routes_the_answer_forward() {
    let (result, model) = run("03-arrow.rebis");
    assert_clean("03-arrow", &result);
    assert_eq!(model.calls(), 2, "an arrow added a call of its own");
    let prompts = model.prompts();
    assert!(
        prompts[1].contains("INPUT:"),
        "the consumer got no input: {prompts:?}"
    );
    // What arrived is what the producer answered — the run's own first answer,
    // whatever the model happened to say.
    let produced = result
        .firings
        .first()
        .and_then(|firing| firing.answer.clone())
        .expect("the producer answered");
    assert!(
        prompts[1].contains(produced.trim()),
        "the consumer did not receive the producer's answer.\n\
         produced: {produced:?}\nconsumer saw: {:?}",
        prompts[1]
    );
}

/// A backflow is the same transport, right side first.
#[test]
#[ignore = "needs a local model"]
fn a_backflow_runs_its_right_side_first() {
    let (result, model) = run("04-backflow.rebis");
    assert_clean("04-backflow", &result);
    assert_eq!(model.calls(), 2);
    let prompts = model.prompts();
    // The program writes the consumer first and the producer second; execution
    // reverses that, so the PRODUCER is the first call.
    assert!(
        prompts[0].contains("reversed"),
        "the left side ran first: {prompts:?}"
    );
    assert!(prompts[1].contains("INPUT:"), "{prompts:?}");
}

/// `$` composes text and fires the result once. Its operands never run.
#[test]
#[ignore = "needs a local model"]
fn a_composition_fires_once_and_runs_no_operand() {
    let (result, model) = run("05-concat.rebis");
    assert_clean("05-concat", &result);
    assert_eq!(
        model.calls(),
        1,
        "an operand of `$` was executed: {:?}",
        model.prompts()
    );
    assert_eq!(
        model.prompts()[0],
        "Answer with exactly the word: composed",
        "the pieces were not joined into one prompt"
    );
}

/// A square's branches are independent and its mediator sees both.
#[test]
#[ignore = "needs a local model"]
fn a_square_mediates_its_branches() {
    let (result, model) = run("06-square.rebis");
    assert_clean("06-square", &result);
    assert_eq!(model.calls(), 3, "two branches and one mediator");
    let prompts = model.prompts();
    // Branches saw nothing of each other.
    assert!(!prompts[0].contains("INPUT:"), "{prompts:?}");
    assert!(!prompts[1].contains("INPUT:"), "{prompts:?}");
    // The mediator saw both, labelled and in source order.
    let mediator = prompts.last().expect("a mediator fired");
    assert!(mediator.contains("RESULT 1:"), "{mediator:?}");
    assert!(mediator.contains("RESULT 2:"), "{mediator:?}");
}

/// `%` is lazy: the branch not taken never fires.
#[test]
#[ignore = "needs a local model"]
fn a_conditional_expands_exactly_one_branch() {
    let (result, model) = run("07-conditional.rebis");
    assert_clean("07-conditional", &result);
    assert_eq!(
        decisions(&result),
        vec![true],
        "the condition did not answer 1"
    );
    assert_eq!(model.calls(), 2, "the condition and one branch, no more");
    let prompts = model.prompts();
    assert!(
        !prompts.iter().any(|prompt| prompt.contains("skipped")),
        "the unselected branch fired: {prompts:?}"
    );
}

/// A macro substitutes syntax structurally.
#[test]
#[ignore = "needs a local model"]
fn a_macro_expands_its_argument_into_its_body() {
    let (result, model) = run("08-macro.rebis");
    assert_clean("08-macro", &result);
    // The definition costs nothing; the call is the arrow inside it.
    assert_eq!(model.calls(), 2);
    let prompts = model.prompts();
    assert_eq!(prompts[0], "Answer with exactly the word: expanded");
    assert!(prompts[1].contains("INPUT:"), "{prompts:?}");
}

/// A quoted template splices the caller's syntax and runs it.
#[test]
#[ignore = "needs a local model"]
fn a_quoted_template_runs_what_was_spliced_into_it() {
    let (result, model) = run("09-quote.rebis");
    assert_clean("09-quote", &result);
    assert_eq!(model.calls(), 2);
    assert_eq!(model.prompts()[0], "Answer with exactly the word: spliced");
}

/// `^` exchanges arrow direction before anything runs.
#[test]
#[ignore = "needs a local model"]
fn inversion_reverses_which_side_runs_first() {
    let (result, model) = run("10-invert.rebis");
    assert_clean("10-invert", &result);
    assert_eq!(model.calls(), 2);
    // Written left-to-right, the repeater would run first. Inverted, the
    // producer does.
    assert!(
        model.prompts()[0].contains("inverted"),
        "inversion did not exchange the direction: {:?}",
        model.prompts()
    );
}

/// A flashback reads what the run already learned, and fires nothing.
#[test]
#[ignore = "needs a local model"]
fn a_flashback_recalls_without_firing() {
    let (result, model) = run("11-flashback.rebis");
    assert_clean("11-flashback", &result);
    // Two firings: the sentence, and the repeater. The recall is free.
    assert_eq!(
        model.calls(),
        2,
        "recall cost a call: {:?}",
        model.prompts()
    );
    let prompts = model.prompts();
    assert!(
        prompts[1].contains("INPUT:") && prompts[1].to_lowercase().contains("beacon"),
        "the recalled evidence did not reach the consumer: {prompts:?}"
    );
}

/// A dream answers its body and marks that answer to outlive the run.
#[test]
#[ignore = "needs a local model"]
fn a_dream_keeps_what_its_body_answered() {
    let (result, model) = run("12-dream.rebis");
    assert_clean("12-dream", &result);
    assert_eq!(model.calls(), 1, "the wrapper fired something of its own");
    assert_eq!(
        result.kept,
        result.output.clone().into_iter().collect::<Vec<_>>(),
        "what was kept is not what was answered"
    );
}

/// A binding fires its value once however often the name is used.
#[test]
#[ignore = "needs a local model"]
fn a_binding_fires_its_value_once() {
    let (result, model) = run("13-bind.rebis");
    assert_clean("13-bind", &result);
    assert_eq!(
        model.calls(),
        2,
        "the bound value fired more than once: {:?}",
        model.prompts()
    );
    // The name stood for the answer in BOTH positions of the composition.
    let bound = result
        .firings
        .first()
        .and_then(|firing| firing.answer.clone())
        .expect("the value answered");
    let composed = &model.prompts()[1];
    assert_eq!(
        composed.matches(bound.trim()).count(),
        2,
        "the name did not stand for the answer twice.\nbound: {bound:?}\nsaw: {composed:?}"
    );
}

/// `&` obtains a value; under a host that wires nothing it is diagnosed.
#[test]
#[ignore = "needs a local model"]
fn an_ask_is_diagnosed_when_nothing_is_wired() {
    let (result, model) = run("14-ask.rebis");
    assert!(unavailable(&result), "an unwired ask was not reported");
    // The program still runs: the arrow routes an empty value.
    assert_eq!(model.calls(), 1);
}

/// `&` obtains a value, and it flows.
#[test]
#[ignore = "needs a local model"]
fn an_ask_flows_what_the_host_delivered() {
    let (result, model) = run_with("14-ask.rebis", &Wired);
    assert_clean("14-ask wired", &result);
    assert_eq!(model.calls(), 1, "obtaining cost a model call");
    assert!(
        model.prompts()[0].contains("cinnabar"),
        "the delivered value did not reach the prompt: {:?}",
        model.prompts()
    );
}

/// `&:` reads a named source, and its text becomes the value.
#[test]
#[ignore = "needs a local model"]
fn a_load_reads_a_named_source() {
    let (result, model) = run_with("15-load.rebis", &Wired);
    assert_clean("15-load", &result);
    assert_eq!(model.calls(), 1, "reading cost a model call");
    assert!(
        model.prompts()[0].contains("pomegranate"),
        "the file's text did not reach the prompt: {:?}",
        model.prompts()
    );
}

/// `+` frames every prompt inside it, ahead of the prompt's own words.
#[test]
#[ignore = "needs a local model"]
fn framing_reaches_every_prompt_in_its_block() {
    let (result, model) = run("16-context.rebis");
    assert_clean("16-context", &result);
    assert_eq!(model.calls(), 2, "framing fired something of its own");
    for prompt in model.prompts() {
        assert!(
            prompt.starts_with("Whenever you answer, end with the word: framed."),
            "framing is not in front: {prompt:?}"
        );
        // And the prompt's own words survive, after it.
        assert!(prompt.contains("Answer with the word"), "{prompt:?}");
    }
}

/// An import loads definitions without executing the module.
#[test]
#[ignore = "needs a local model"]
fn an_import_costs_nothing_and_defines_what_it_holds() {
    let (result, model) = run("17-import.rebis");
    assert_clean("17-import", &result);
    assert_eq!(
        model.calls(),
        1,
        "importing executed the module: {:?}",
        model.prompts()
    );
    // The macro it defined shaped the prompt.
    assert!(
        model.prompts()[0].to_lowercase().contains("one word"),
        "{:?}",
        model.prompts()
    );
}

/// A model selector routes without changing anything else.
#[test]
#[ignore = "needs a local model"]
fn a_model_selector_routes_and_adds_no_call() {
    let (result, model) = run("18-model.rebis");
    assert_clean("18-model", &result);
    assert_eq!(model.calls(), 1);
    assert_eq!(model.prompts()[0], "Answer with exactly the word: routed");
}

/// Framing, binding, gating and keeping, nested in one program.
#[test]
#[ignore = "needs a local model"]
fn every_scoping_operator_composes() {
    let (result, model) = run("19-nested.rebis");
    assert_clean("19-nested", &result);
    assert_eq!(decisions(&result), vec![true]);
    // The bound value, the condition, and the one selected branch.
    assert_eq!(model.calls(), 3, "{:?}", model.prompts());
    let prompts = model.prompts();
    // Every prompt inside the framing carries it — including the condition,
    // which is written inside the block.
    for prompt in &prompts {
        assert!(
            prompt.starts_with("Answer in one word only, always."),
            "framing missed a prompt: {prompt:?}"
        );
    }
    // The unreachable branch never fired.
    assert!(!prompts.iter().any(|prompt| prompt.contains("unreachable")));
    // And the dream kept exactly the program's answer.
    assert_eq!(result.kept.len(), 1);
    assert_eq!(result.kept.first(), result.output.as_ref());
}

/// A square inside a square: nesting the one form that is not a chain.
#[test]
#[ignore = "needs a local model"]
fn squares_nest() {
    let (result, model) = run("20-deep-square.rebis");
    assert_clean("20-deep-square", &result);
    // Two inner branches, the inner mediator, the outer branch, the outer
    // mediator.
    assert_eq!(model.calls(), 5, "{:?}", model.prompts());
    let mediators = model
        .prompts()
        .iter()
        .filter(|prompt| prompt.contains("RESULT 1:"))
        .count();
    assert_eq!(mediators, 2, "both mediators should have fired");
}

// ── the imaginary space ─────────────────────────────────────────────────────
//
// These are the tests the trace cannot do. Every other assertion in this file
// reads firings and prompts, and by design a space changes neither: the same
// program with and without braces fires the same prompts in the same order.
// What it changes is the record, so these read the record — through a real
// flashback, so a test cannot pass on a coincidence the runtime would never
// have recalled.

/// The whole claim, in one test: the work happens and only the answer is kept.
#[test]
#[ignore = "needs a local model"]
fn a_space_keeps_its_answer_and_forgets_its_working() {
    let (result, model, record) = run_recording("52-imaginary.rebis");
    assert_clean("52-imaginary", &result);
    assert_eq!(
        model.calls(),
        2,
        "a space is free — it fires nothing of its own: {:?}",
        model.prompts()
    );
    assert!(result.output.is_some(), "the space produced no answer");
    assert!(
        !remembers(&record, "phlogiston"),
        "the working crossed out; it should not be evidence"
    );
    assert!(
        remembers(&record, "oxygen"),
        "the space's own answer should be evidence"
    );
}

/// The control. Identical but for the delimiter — and the record differs.
///
/// Run as a pair with the test above: alone, either one could pass for a
/// reason that has nothing to do with braces (a model that ignored the first
/// prompt, a recall that reached nothing). Together they isolate the
/// delimiter as the only thing that changed.
#[test]
#[ignore = "needs a local model"]
fn the_same_program_in_parentheses_remembers_everything() {
    let (result, model, record) = run_recording("53-imaginary-real.rebis");
    assert_clean("53-imaginary-real", &result);
    assert_eq!(model.calls(), 2, "{:?}", model.prompts());
    assert!(
        remembers(&record, "phlogiston"),
        "outside a space the working IS evidence — if this fails the pair \
         proves nothing, because the contrast has gone"
    );
    assert!(remembers(&record, "oxygen"));
}

/// Isolation is from the outside. A space still remembers itself while it runs.
#[test]
#[ignore = "needs a local model"]
fn a_space_recalls_its_own_earlier_stages() {
    let (result, model, record) = run_recording("54-imaginary-recall.rebis");
    assert_clean("54-imaginary-recall", &result);
    // Two prompts. The flashback between them is free.
    assert_eq!(model.calls(), 2, "{:?}", model.prompts());
    let prompts = model.prompts();
    assert!(
        prompts[1].contains("INPUT:"),
        "the recall reached nothing, so the second stage got no input: {prompts:?}"
    );
    assert!(
        prompts[1].contains("beacon"),
        "the flashback should have carried the space's own first answer: {prompts:?}"
    );
    // And the space still let exactly one thing out. Content cannot show this
    // one: the crossing repeats the sentence the first stage established, so
    // both answers say "beacon" and only the COUNT distinguishes one crossing
    // from two appended answers. Without the braces this record holds two.
    let answer = result.output.clone().expect("the space produced no answer");
    assert!(
        remembers(&record, "beacon"),
        "the space's own answer is evidence — it crossed out"
    );
    assert_eq!(
        record.len(),
        answer.lines().filter(|line| !line.trim().is_empty()).count(),
        "the working leaked: the record should hold the crossing alone.\n\
         crossing: {answer:?}"
    );
}

/// You do not get to keep a promise you made in a dream.
#[test]
#[ignore = "needs a local model"]
fn a_dream_inside_a_space_is_not_kept() {
    let (result, model, _record) = run_recording("55-imaginary-dream.rebis");
    assert_clean("55-imaginary-dream", &result);
    assert_eq!(model.calls(), 1);
    assert!(
        result.kept.is_empty(),
        "a mark made inside something that did not happen was kept: {:?}",
        result.kept
    );
    // Silent, not fatal: the dream is still transparent to its body's answer.
    assert!(
        result.output.is_some(),
        "suppressing the mark must not suppress the answer"
    );
}

/// std/imaginary composes: a symbol mediator picks, and only the pick survives.
#[test]
#[ignore = "needs a local model"]
fn the_library_shapes_hide_their_deliberation() {
    let (result, model, record) = run_recording("56-imaginary-std.rebis");
    assert_clean("56-imaginary-std", &result);
    // Two branches and the mediator.
    assert_eq!(model.calls(), 3, "{:?}", model.prompts());
    let answer = result.output.clone().expect("std-weighed produced nothing");

    // Both branches fired — the deliberation really happened.
    let prompts = model.prompts();
    assert!(prompts.iter().any(|p| p.contains("phlogiston")), "{prompts:?}");
    assert!(prompts.iter().any(|p| p.contains("cobalt")), "{prompts:?}");

    // And neither branch's ANSWER is evidence. Which option the mediator
    // preferred is the model's business; that the losing case is not sitting
    // in memory afterwards is the language's, and it is what the macro is for.
    assert!(
        !record.is_empty(),
        "the verdict should have crossed out: {answer:?}"
    );
    let branch_answers = result
        .firings
        .iter()
        .take(2)
        .filter_map(|firing| firing.answer.clone())
        .filter(|a| !a.trim().is_empty())
        .count();
    assert_eq!(branch_answers, 2, "both branches should have answered");
    assert_eq!(
        record.len(),
        answer.lines().filter(|l| !l.trim().is_empty()).count(),
        "the record should hold the verdict and nothing else — \
         two branch answers leaked into memory.\nverdict: {answer:?}"
    );
}
