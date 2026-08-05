//! Every macro in the library, invoked.
//!
//! `collection.rs` checks that modules parse and that their imports resolve.
//! That is a static check, and it was passing while two real defects sat in the
//! tree: a macro calling a name its module never imported, and four macros that
//! re-entered the whole program instead of reading it — forty-seven model calls
//! and no answer, where each documented one call.
//!
//! Parsing is not running. This calls every macro at its own arity, with
//! callable placeholders for the arguments that are meant to be macro names,
//! and fails on the three things that mean the library disagrees with itself:
//! an undefined name, a wrong arity, and a form that nests until the runtime
//! stops it.
//!
//! It deliberately does NOT judge answers. A scripted oracle says the same
//! thing to everything, so what is under test is that the shapes hold together
//! — which is exactly what a static check cannot see.

use std::collections::BTreeMap;
use std::path::Path;

use rebis_lang::{
    orchestrate_with_runtime, parse, std_modules, ExecutionEvent, ModuleName, ModuleResolver,
    Oracle, Record,
};

/// Resolves the vendored collection first, then the embedded standard library.
struct Library;

impl ModuleResolver for Library {
    fn resolve(&self, name: &ModuleName) -> Result<Option<String>, String> {
        let path = collection().join(format!("{}.rebis", name.as_str()));
        if let Ok(source) = std::fs::read_to_string(path) {
            return Ok(Some(source));
        }
        Ok(std_modules()
            .iter()
            .find(|(module, _)| *module == name.as_str())
            .map(|(_, source)| (*source).to_string()))
    }
}

struct Same;

impl Oracle for Same {
    fn fire(&self, _prompt: &str) -> Option<String> {
        Some("a plausible finding about the retry queue".to_string())
    }
}

fn collection() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .join("rebis-collection/modules")
        .leak()
}

/// Every `(~ name (params…)` defined at a module's top level.
///
/// Matches on the two-space indent every module uses, so the examples inside
/// doc comments — which are indented further and are not definitions — do not
/// register as macros.
fn definitions(source: &str) -> Vec<(String, usize)> {
    let mut found = Vec::new();
    let mut rest = source;
    while let Some(at) = rest.find("\n  (~ ") {
        rest = &rest[at + 6..];
        let name: String = rest.chars().take_while(|c| !c.is_whitespace()).collect();
        let after = &rest[name.len()..];
        let (Some(open), Some(close)) = (after.find('('), after.find(')')) else {
            continue;
        };
        if close < open {
            continue;
        }
        let arity = after[open + 1..close].split_whitespace().count();
        if !name.is_empty() {
            found.push((name, arity));
        }
    }
    found
}

/// Call one macro and report only the failures that mean the library is wrong
/// about itself.
fn invoke(module: &str, name: &str, arity: usize) -> Vec<String> {
    // Real zero-argument macros, so a macro that CALLS its argument — which
    // many do, taking a worker or a judge — gets something callable.
    let placeholders: String = (0..arity)
        .map(|i| format!("(~ arg{i} () \"placeholder\") "))
        .collect();
    let arguments: String = (0..arity).map(|i| format!(" arg{i}")).collect();
    let program = format!("((# {module}) {placeholders}({name}{arguments}))");

    let Ok(tree) = parse(&program) else {
        return vec![format!("{module} :: {name} does not parse when called")];
    };
    let mut record = Record::from_texts::<&str>(&[]);
    let mut observer = |_: &ExecutionEvent| {};
    let run = orchestrate_with_runtime(&tree, &mut record, &Same, &Library, &mut observer);

    run.diagnostics
        .iter()
        .map(std::string::ToString::to_string)
        .filter(|text| {
            // A placeholder is not the library's problem: a macro may require
            // an argument this harness cannot invent.
            !(0..arity).any(|i| text.contains(&format!("arg{i}")))
        })
        .filter(|text| {
            text.contains("undefined") || text.contains("expected") || text.contains("nesting")
        })
        .map(|text| format!("{module} :: {name} — {text}"))
        .collect()
}

/// Every shipped example, run with the whole library resolvable.
///
/// `rebis`'s own `tests/examples.rs` checks that they parse; it cannot check
/// that they RUN, because several import collection modules and the collection
/// is vendored here rather than by the language.
///
/// Two pre-existing breaks were found this way, in a file nobody had run since
/// the `&` form changed under it: it did not parse, and once it did, it called
/// a macro through an import chain that had gone stale.
#[test]
fn every_example_runs_against_the_whole_library() {
    let examples = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .join("../rebis/examples");
    let Ok(entries) = std::fs::read_dir(&examples) else {
        // A checkout without the sibling language repo still builds; there is
        // nothing to check and nothing to fail.
        return;
    };
    let mut checked = 0;
    let mut failures = Vec::new();
    for entry in entries.flatten() {
        if entry.path().extension().is_none_or(|e| e != "rebis") {
            continue;
        }
        let name = entry
            .path()
            .file_stem()
            .expect("stem")
            .to_string_lossy()
            .into_owned();
        let source = std::fs::read_to_string(entry.path()).expect("read");
        let Ok(tree) = parse(&source) else {
            failures.push(format!("{name} does not parse"));
            continue;
        };
        checked += 1;
        let mut record = Record::from_texts::<&str>(&[]);
        let mut observer = |_: &ExecutionEvent| {};
        let run = orchestrate_with_runtime(&tree, &mut record, &Same, &Library, &mut observer);
        for diagnostic in &run.diagnostics {
            let text = diagnostic.to_string();
            if text.contains("undefined") || text.contains("expected") || text.contains("nesting") {
                failures.push(format!("{name} — {text}"));
            }
        }
    }
    assert!(checked >= 8, "only {checked} examples ran");
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn every_standard_library_macro_runs() {
    let mut failures = Vec::new();
    let mut count = 0;
    for (module, source) in std_modules() {
        for (name, arity) in definitions(source) {
            count += 1;
            failures.extend(invoke(module, &name, arity));
        }
    }
    assert!(
        count > 150,
        "only {count} macros found — the scan is broken"
    );
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn every_collection_macro_runs() {
    let mut modules: BTreeMap<String, String> = BTreeMap::new();
    for family in std::fs::read_dir(collection())
        .expect("collection")
        .flatten()
    {
        if !family.path().is_dir() {
            continue;
        }
        for leaf in std::fs::read_dir(family.path()).expect("family").flatten() {
            if leaf.path().extension().is_some_and(|e| e == "rebis") {
                let name = format!(
                    "{}/{}",
                    family.file_name().to_string_lossy(),
                    leaf.path().file_stem().expect("stem").to_string_lossy()
                );
                modules.insert(name, std::fs::read_to_string(leaf.path()).expect("read"));
            }
        }
    }
    let mut failures = Vec::new();
    let mut count = 0;
    for (module, source) in &modules {
        for (name, arity) in definitions(source) {
            count += 1;
            failures.extend(invoke(module, &name, arity));
        }
    }
    assert!(
        count > 140,
        "only {count} macros found — the scan is broken"
    );
    assert!(failures.is_empty(), "{failures:#?}");
}
