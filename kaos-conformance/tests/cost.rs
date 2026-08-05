//! Does the predicted price match the one the suite already measured?
//!
//! `kaos_core::cost` claims to read a program's firing count off the page
//! before it runs. Every conformance program declares what it actually costs in
//! a `; expect: calls N` header, checked against a real model by `run.sh`. So
//! the suite is already a corpus of programs with verified prices, and this
//! points the predictor at it.
//!
//! That makes this a stronger check than a unit test could be. The unit tests in
//! `cost.rs` assert what its author believed; these assert against numbers that
//! were measured by running seventy-six programs against a model — including the
//! ones whose shape nobody would have thought to write down. Two bugs in the
//! predictor were found exactly this way: a framing counted as a firing, and a
//! bound value not counted at all.
//!
//! # What is checked, and what is skipped
//!
//! Only programs whose price is **exact** — no macro calls, no imports, no
//! conditionals. A program that imports `std/loops` costs whatever the macro
//! expands to, and the predictor says so in prose rather than inventing a
//! number; asserting a number there would be asserting the wrong thing.
//!
//! The skips are counted and floored, so the check cannot quietly erode into
//! covering nothing.

use std::path::Path;

use kaos_core::cost;

/// The `; expect: calls N` a program declares, when it declares an exact one.
fn declared_calls(source: &str) -> Option<usize> {
    source.lines().find_map(|line| {
        let claim = line.trim().strip_prefix("; expect: calls ")?;
        // `>=N` is a floor, not a price. The predictor claims an exact number,
        // and a floor cannot confirm or refute one.
        claim.trim().parse().ok()
    })
}

#[test]
fn the_predicted_price_matches_every_program_that_has_an_exact_one() {
    let mut checked = 0;
    let mut skipped: Vec<(String, String)> = Vec::new();
    let mut wrong: Vec<String> = Vec::new();

    let mut programs: Vec<_> = std::fs::read_dir(kaos_conformance::programs())
        .expect("the programs directory")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|kind| kind == "rebis"))
        .collect();
    programs.sort();

    for path in programs {
        let name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_default()
            .to_string();
        let source = std::fs::read_to_string(&path).expect("readable");
        let Some(declared) = declared_calls(&source) else {
            skipped.push((name, "no exact `expect: calls N`".to_string()));
            continue;
        };
        let Ok(expression) = rebis_lang::parse(&source) else {
            skipped.push((name, "does not parse".to_string()));
            continue;
        };
        let cost = cost::of(&expression);
        if !cost.is_exact() {
            skipped.push((name, cost.conditional.join("; ")));
            continue;
        }
        checked += 1;
        if cost.firings != declared {
            wrong.push(format!(
                "{name}: predicted {} firings, the suite measured {declared}",
                cost.firings
            ));
        }
    }

    assert!(
        wrong.is_empty(),
        "the predictor disagrees with what these programs actually cost:\n  {}\n\
         ({checked} checked, {} skipped as conditional)",
        wrong.join("\n  "),
        skipped.len()
    );
    // A floor, so the check cannot erode into covering nothing while still
    // passing. If a change to the predictor makes many programs "conditional",
    // that is the failure worth catching.
    assert!(
        checked >= 20,
        "only {checked} programs had an exactly predictable price; skipped:\n  {}",
        skipped
            .iter()
            .map(|(name, why)| format!("{name}: {why}"))
            .collect::<Vec<_>>()
            .join("\n  ")
    );
}

#[test]
fn a_program_that_imports_is_reported_as_conditional_rather_than_guessed() {
    // The honest half. A macro's cost lives in its expansion, which reading the
    // source cannot reach, so the predictor must decline rather than under-count
    // — a confidently wrong price is worse than an admitted unknown.
    let source =
        std::fs::read_to_string(Path::new(&kaos_conformance::programs()).join("31-std-loop.rebis"))
            .expect("the program");
    let expression = rebis_lang::parse(&source).expect("parses");
    let cost = cost::of(&expression);
    assert!(!cost.is_exact(), "{cost:?}");
    assert!(
        cost.summary().ends_with("+ conditional"),
        "{}",
        cost.summary()
    );
}
