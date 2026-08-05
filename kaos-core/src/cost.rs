//! What a program will cost, read off the page before it is paid for.
//!
//! Rebis's claim is **cost readability**: one written prompt is one firing, and
//! a program's price can be counted by reading it. `rebis/tests/costs.rs` keeps
//! that true inside the library. This is the other half — the same count, made
//! available to a person about to launch a run.
//!
//! It matters because the harness did not have it. A composed program was queued
//! in the run browser showing its source and nothing else, so the one number the
//! language exists to make countable was the one number a user could not see.
//!
//! # Exact, and conditional
//!
//! The split is the one `tests/costs.rs` already makes, and it is honest about
//! where the line falls:
//!
//! - **[`Cost::firings`] is exact.** Every prompt written on the page, counted
//!   once — including the copies a mediator square writes out, which is why the
//!   square writes them out.
//! - **[`Cost::conditional`] is prose.** Some costs cannot be known without
//!   running the program, and a number that pretended otherwise would be worse
//!   than a sentence that admits it. A conditional writes one branch or the
//!   other; a `><` fires once and then runs whatever it was answered with.
//!
//! A caller shows both. Showing only the exact part would under-report exactly
//! the programs whose price is hardest to guess.
//!
//! # What counts as a firing
//!
//! A model call. A prompt fires; so does a composition, which assembles its
//! operands into one prompt and sends it. Everything else is arithmetic on
//! answers the program already has: the numeric plane fires nothing by
//! construction, a mediator judges what its branches produced, an inverted or
//! quoted form is syntax rather than work, and interpolating a name reads a
//! value rather than asking for one.
//!
//! **A quoted form costs nothing.** `'(…)` is syntax the program holds, not work
//! it does — and the one path from a quoted form back to work is running it,
//! which is a conditional the walk reports rather than a number it invents.

use rebis_lang::Expr;

/// A program's price: the part that is countable, and the part that is not.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Cost {
    /// Model calls this program makes no matter what happens.
    pub firings: usize,
    /// Costs that depend on what the run finds, one sentence each, in the order
    /// they appear in the source.
    pub conditional: Vec<String>,
}

impl Cost {
    /// Whether anything about this price depends on the run.
    #[must_use]
    pub fn is_exact(&self) -> bool {
        self.conditional.is_empty()
    }

    /// One line for a status bar or a queue entry.
    ///
    /// `3 firings` when exact, `3 firings + conditional` when not — the caller
    /// shows [`Cost::conditional`] itself where there is room, and this is what
    /// fits where there is not.
    #[must_use]
    pub fn summary(&self) -> String {
        let plural = if self.firings == 1 { "" } else { "s" };
        if self.is_exact() {
            format!("{} firing{plural}", self.firings)
        } else {
            format!("{} firing{plural} + conditional", self.firings)
        }
    }
}

/// The exact firing count, for a caller that only needs the number.
#[must_use]
pub fn firings(expr: &Expr) -> usize {
    of(expr).firings
}

/// What running `expr` will cost.
#[must_use]
pub fn of(expr: &Expr) -> Cost {
    let mut cost = Cost::default();
    walk(expr, &mut cost);
    cost
}

/// Sum the costs of several subtrees into `cost`.
fn walk_each(items: &[Expr], cost: &mut Cost) {
    for item in items {
        walk(item, cost);
    }
}

#[allow(clippy::match_same_arms)]
fn walk(expr: &Expr, cost: &mut Cost) {
    match expr {
        // ── the things that fire ────────────────────────────────────────────
        //
        // A written prompt is one firing. That is the whole claim.
        Expr::Prompt(_) => cost.firings += 1,
        // A composition assembles its operands into ONE prompt and sends it.
        // The operands are interpolated rather than run — reading is free and
        // firing is not — so the composition costs one, not one per operand.
        Expr::Concat(_) => cost.firings += 1,

        // ── the things that cost what they contain ──────────────────────────
        Expr::Program(items)
        | Expr::Compose(items)
        | Expr::Meta(items)
        | Expr::Imaginary(items) => walk_each(items, cost),
        Expr::Forward(left, right) | Expr::Backflow(left, right) => {
            walk(left, cost);
            walk(right, cost);
        }
        Expr::Bind { value, body, .. } => {
            walk(value, cost);
            walk(body, cost);
        }
        // A framing is text prepended to the prompts inside it. It reaches
        // them; it is NOT one of them, and `context` is deliberately not walked
        // — `(+ "be terse" ("a") ("b"))` is two calls, which
        // `kaos-conformance/programs/16-context.rebis` pins.
        Expr::Context { context: _, body } => walk(body, cost),
        Expr::Dream(inner)
        | Expr::Supersede { body: inner, .. }
        | Expr::Model { body: inner, .. } => walk(inner, cost),

        // `(&: source)` names a file. The name is written in prompt position
        // because that is where a piece of text goes, but nothing asks a model
        // about it — `15-load.rebis` costs one call and its `->` is the one.
        Expr::Load(_) => {}

        // `(@ check body)` runs `check` at every arrow edge inside its scope,
        // so its price is the body plus one check per edge. Counting the edges
        // statically is what makes this exact rather than prose, and
        // `64-invariant.rebis` is the case that proves it: two stages and one
        // arrow, so three calls, and that third one is the whole point of the
        // form.
        Expr::Invariant { check, body } => {
            walk(body, cost);
            let per_edge = of(check);
            let edges = arrow_edges(body);
            cost.firings += per_edge.firings * edges;
            if !per_edge.conditional.is_empty() && edges > 0 {
                cost.conditional.extend(per_edge.conditional);
            }
            // And the counted total is an UPPER bound, not a price. A refused
            // edge stops the scope, so the stages after it never fire —
            // `65-invariant-refuses.rebis` costs 5 unbroken and 3 refused, and
            // nothing readable off the page decides which. Saying so is the
            // whole difference between a prediction and a guess.
            if edges > 0 {
                cost.conditional.push(format!(
                    "an invariant may refuse an edge and stop its scope, so fewer \
                     than this may fire ({edges} edge{} checked)",
                    if edges == 1 { "" } else { "s" }
                ));
            }
        }

        // A square fires every branch, then judges. The judgement is free when
        // the mediator is symbols and is a program when it is not — in which
        // case the mediator's own cost is real and countable, so it is walked.
        Expr::Square { mediator, branches } => {
            walk_each(branches, cost);
            walk(mediator, cost);
        }

        // ── the things whose cost the run decides ───────────────────────────
        Expr::Conditional {
            condition,
            when_yes,
            when_no,
        } => {
            walk(condition, cost);
            // Exactly one branch runs, so neither can be counted. Reporting the
            // larger would over-charge every run that took the other; reporting
            // the smaller would under-charge, which is worse. The honest number
            // is the pair.
            let (yes, no) = (of(when_yes), of(when_no));
            if yes.firings != 0 || no.firings != 0 {
                cost.conditional.push(format!(
                    "a conditional runs one branch of two: {} or {} firings",
                    yes.firings, no.firings
                ));
            }
            cost.conditional.extend(yes.conditional);
            cost.conditional.extend(no.conditional);
        }

        // ── the things that are not work ────────────────────────────────────
        //
        // Syntax the program holds rather than does. `'(…)` costs nothing until
        // something runs it, and running it is a call — counted where the call
        // is, not here.
        Expr::Quote(_) => {}
        // The numeric plane fires nothing, by construction and by test. It is
        // the reason a program can measure itself for free.
        Expr::Numeric(_) => {}
        // Reading a value, not asking for one.
        Expr::Symbol(_) | Expr::Unquote(_) | Expr::Source => {}
        // An obtain reaches the host, which may block a run for a long time. It
        // does not reach a model, and this counts model calls.
        Expr::Ask => {}
        // A definition costs nothing; its uses cost.
        Expr::Function { .. } => {}
        // An inverted form is its dual, which has the same shape and so the
        // same price.
        Expr::Invert(inner) => walk(inner, cost),
        // A recall reads the record. Free, like every other read.
        Expr::Flashback(_) => {}

        // ── the things whose cost is elsewhere ──────────────────────────────
        //
        // A macro call costs whatever it expands to, and the expansion is not
        // available here: resolving it means running the expander, which needs a
        // module resolver and a budget. Reporting it as prose keeps this
        // function a pure read of the source, which is what makes it usable
        // before a run rather than during one.
        Expr::Call { name, args } => {
            walk_each(args, cost);
            cost.conditional
                .push(format!("`{name}` costs whatever it expands to"));
        }
        Expr::Import { module } => {
            cost.conditional
                .push(format!("`{module}` brings in macros with their own costs"));
        }
    }
}

/// How many arrow edges an invariant will be asked about.
///
/// `(-> a b c)` is left-associative and so two edges, which is why this counts
/// nodes rather than arrow *forms*. A conditional's branches are counted at
/// their maximum: an invariant that might check twice and might check once is
/// reported at the price a reader should budget for, and the conditional itself
/// already says the branch is a choice.
fn arrow_edges(expr: &Expr) -> usize {
    match expr {
        Expr::Forward(left, right) | Expr::Backflow(left, right) => {
            1 + arrow_edges(left) + arrow_edges(right)
        }
        Expr::Program(items)
        | Expr::Compose(items)
        | Expr::Meta(items)
        | Expr::Imaginary(items)
        | Expr::Numeric(items)
        | Expr::Concat(items)
        | Expr::Flashback(items) => items.iter().map(arrow_edges).sum(),
        Expr::Square { mediator, branches } => {
            arrow_edges(mediator) + branches.iter().map(arrow_edges).sum::<usize>()
        }
        Expr::Conditional {
            condition,
            when_yes,
            when_no,
        } => arrow_edges(condition) + arrow_edges(when_yes).max(arrow_edges(when_no)),
        Expr::Bind { value, body, .. } => arrow_edges(value) + arrow_edges(body),
        Expr::Context { context, body } => arrow_edges(context) + arrow_edges(body),
        Expr::Invariant { check, body } => arrow_edges(check) + arrow_edges(body),
        Expr::Dream(inner)
        | Expr::Supersede { body: inner, .. }
        | Expr::Model { body: inner, .. }
        | Expr::Invert(inner)
        | Expr::Function { body: inner, .. } => arrow_edges(inner),
        Expr::Call { args, .. } => args.iter().map(arrow_edges).sum(),
        // A quoted arrow is syntax, not an edge, until something runs it.
        Expr::Quote(_)
        | Expr::Unquote(_)
        | Expr::Prompt(_)
        | Expr::Symbol(_)
        | Expr::Source
        | Expr::Ask
        | Expr::Load(_)
        | Expr::Import { .. } => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cost(source: &str) -> Cost {
        of(&rebis_lang::parse(source).unwrap_or_else(|error| panic!("{source}: {error}")))
    }

    #[test]
    fn one_written_prompt_is_one_firing() {
        assert_eq!(cost(r#""solve it""#).firings, 1);
        assert!(cost(r#""solve it""#).is_exact());
    }

    #[test]
    fn a_square_costs_its_branches() {
        // The number the whole conclave design is about, and the reason the
        // branches are written out rather than hidden behind a width setting.
        assert_eq!(cost(r#"([vote] "a" "b" "c")"#).firings, 3);
        assert_eq!(cost(r#"([vote] "a" "b" "c" "d" "e")"#).firings, 5);
    }

    #[test]
    fn a_composition_is_one_prompt_not_one_per_operand() {
        // `$` interpolates its operands; it does not run them. A cost model that
        // counted them would over-report every program that builds a prompt.
        // Two: the bound value is itself a written prompt, and the composition
        // is one more. Not four — `$` reads its operands rather than running
        // them, which is the property `map`/`fold` are still blocked on.
        assert_eq!(cost(r#"(= t "x" ($ t t t))"#).firings, 2);
    }

    #[test]
    fn the_shipped_conclave_default_costs_five() {
        let default = crate::config::default_value("KAOS_MYTH").expect("a default");
        assert_eq!(cost(&default).firings, 5);
        assert!(
            cost(&default).is_exact(),
            "the default's price must be exactly readable: {:?}",
            cost(&default).conditional
        );
    }

    #[test]
    fn an_arrow_costs_both_ends() {
        assert_eq!(cost(r#"(-> "draft" "revise")"#).firings, 2);
        assert_eq!(cost(r#"(-> "a" "b" "c")"#).firings, 3);
    }

    #[test]
    fn the_numeric_plane_is_free() {
        // I6: the numeric plane fires nothing. If this ever costs anything, the
        // language has lost the property that lets a program measure itself.
        assert_eq!(cost("|([sum] 1 2 3)|").firings, 0);
        assert_eq!(cost("|(+ 10 ([sum] 1 2))|").firings, 0);
    }

    #[test]
    fn quoted_syntax_is_held_not_done() {
        assert_eq!(cost(r#"'("expensive" "also expensive")"#).firings, 0);
    }

    #[test]
    fn a_framing_reaches_the_prompts_without_being_one() {
        assert_eq!(cost(r#"(+ "be terse" "solve it")"#).firings, 1);
        assert_eq!(cost(r#"(+ "be terse" ([vote] "a" "b"))"#).firings, 2);
    }

    #[test]
    fn a_conditional_reports_both_branches_rather_than_guessing() {
        let cost = cost(r#"(% |([more] 2 1)| ([vote] "a" "b" "c") "d")"#);
        assert_eq!(
            cost.firings, 0,
            "nothing outside the branches is certain to fire"
        );
        assert_eq!(cost.conditional.len(), 1);
        assert!(
            cost.conditional[0].contains('3') && cost.conditional[0].contains('1'),
            "{:?}",
            cost.conditional
        );
        assert!(!cost.is_exact());
    }

    #[test]
    fn a_macro_call_says_it_cannot_be_counted_here() {
        let cost = cost(r#"((~ f (x) ($ x)) (f "go"))"#);
        assert!(
            cost.conditional
                .iter()
                .any(|why| why.contains("expands to")),
            "{cost:?}"
        );
    }

    #[test]
    fn an_invariant_counts_its_checks_and_admits_it_is_an_upper_bound() {
        // `64-invariant.rebis`: two stages, one arrow, one check on top — the
        // third call is what the form exists to make.
        let held = cost(r#"(@ "is it 1?" (-> "Answer alpha" "Repeat it"))"#);
        assert_eq!(held.firings, 3);
        // And `65-invariant-refuses.rebis` is the same shape costing 3 instead
        // of 5, because a refusal stops the scope. Neither number is wrong; the
        // page cannot say which happens, so the prediction says that.
        assert!(
            held.conditional.iter().any(|why| why.contains("refuse")),
            "{held:?}"
        );
        assert!(!held.is_exact());
    }

    #[test]
    fn a_named_source_is_a_path_not_a_prompt() {
        // `(&: "file.txt")` reads a file. The quotes are how text is written,
        // not a prompt anyone asks — `15-load.rebis` costs one call and its
        // arrow is the one.
        assert_eq!(cost(r#"(&: "notes.txt")"#).firings, 0);
        assert_eq!(cost(r#"(-> (&: "notes.txt") "repeat it")"#).firings, 1);
    }

    #[test]
    fn the_summary_reads_as_a_price() {
        assert_eq!(cost(r#""one""#).summary(), "1 firing");
        assert_eq!(cost(r#"([vote] "a" "b")"#).summary(), "2 firings");
        assert_eq!(cost("|([sum] 1 2)|").summary(), "0 firings");
        assert!(cost(r#"(% |([more] 2 1)| "a" "b")"#)
            .summary()
            .ends_with("+ conditional"));
    }
}
