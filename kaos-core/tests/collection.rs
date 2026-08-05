//! Static checks over `rebis-collection`, the vendored module tree.
//!
//! The collection is deliberately a source collection rather than a Rust
//! crate, so nothing in it is compiled and nothing in it was checked. Its only
//! other coverage is one conformance program that imports one module and needs
//! a running model to say anything at all — so a paren dropped in any of the
//! other thirteen files reached a person before it reached a test.
//!
//! Everything here is free: no model, no network, no host. Three questions,
//! each catching a class of mistake the collection has actually made.
//!
//! 1. **Does it parse?** Three unbalanced delimiters were found by hand while
//!    the imaginary space was being threaded through these files.
//! 2. **Is it a module?** A module may hold only definitions and imports. A
//!    stray executable form parses cleanly and then fails at import, in a
//!    diagnostic that names the importing program rather than the typo.
//! 3. **Do its `std/` imports exist?** `(# std/imaginry)` is a perfectly good
//!    parse and a module that can never load.
//!
//! When the submodule is not checked out these tests report that and pass:
//! a workspace without `rebis-collection` is an incomplete checkout, not a
//! broken collection. `EVERY_MODULE` is the guard against that silence
//! becoming permanent — it fails if the tree resolves but holds nothing.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use rebis_lang::{parse, std_modules, Expr};

/// The vendored module tree, resolved from this crate rather than the working
/// directory so the answer does not depend on where `cargo test` was invoked.
fn modules_root() -> Option<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()?
        .join("rebis-collection/modules");
    root.is_dir().then_some(root)
}

/// Every `.rebis` file under the tree, sorted, as (qualified name, source).
///
/// The name is the module path a program would import — `science/method` for
/// `modules/science/method.rebis` — so a failure names the thing a reader
/// would go looking for.
fn collection() -> Vec<(String, String)> {
    let Some(root) = modules_root() else {
        return Vec::new();
    };
    let mut found = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(directory) = stack.pop() {
        let entries = std::fs::read_dir(&directory)
            .unwrap_or_else(|error| panic!("{}: {error}", directory.display()));
        for entry in entries {
            let path = entry
                .unwrap_or_else(|error| panic!("{}: {error}", directory.display()))
                .path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rebis") {
                let name = path
                    .strip_prefix(&root)
                    .expect("walked from the root")
                    .with_extension("")
                    .to_string_lossy()
                    .replace('\\', "/");
                let source = std::fs::read_to_string(&path)
                    .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
                found.push((name, source));
            }
        }
    }
    found.sort();
    found
}

/// Print the reason and skip, when the submodule is not checked out.
fn skipped() -> bool {
    if modules_root().is_none() {
        println!("rebis-collection is not checked out — skipping");
        return true;
    }
    false
}

/// The tree is there and has modules in it.
///
/// Without this every other test here would pass by finding nothing, and the
/// suite would go quiet exactly when the collection went missing.
#[test]
fn the_collection_is_present_and_not_empty() {
    if skipped() {
        return;
    }
    let modules = collection();
    assert!(
        !modules.is_empty(),
        "rebis-collection/modules resolved but holds no .rebis files"
    );
}

/// Every module parses.
#[test]
fn every_module_parses() {
    if skipped() {
        return;
    }
    let mut broken = Vec::new();
    for (name, source) in collection() {
        if let Err(error) = parse(&source) {
            broken.push(format!("{name}: {} at {:?}", error.message, error.offset));
        }
    }
    assert!(broken.is_empty(), "modules that do not parse:\n{broken:#?}");
}

/// Every module is a MODULE: definitions and imports, nothing executable.
///
/// A module body may be one definition, or a group of definitions and nested
/// imports. Anything else parses and then fails at import time, in a
/// diagnostic that names the program doing the importing rather than the file
/// with the mistake in it.
#[test]
fn every_module_holds_only_definitions_and_imports() {
    if skipped() {
        return;
    }

    /// What a module-level form is allowed to be.
    ///
    /// Names the offending form in the language's own vocabulary — a
    /// discriminant tells a reader nothing about which line to open.
    fn offender(form: &Expr) -> Option<String> {
        match form {
            Expr::Function { .. } | Expr::Import { .. } => None,
            // A group at module level is the ordinary shape: recurse.
            Expr::Program(forms) | Expr::Compose(forms) => forms.iter().find_map(offender),
            Expr::Symbol(name) => Some(format!("the bare symbol `{name}`")),
            Expr::Prompt(text) => {
                let head: String = text.chars().take(48).collect();
                Some(format!("a prompt: {head:?}"))
            }
            Expr::Call { name, .. } => Some(format!("a call to `{name}`")),
            Expr::Concat(_) => Some("a `$` composition".to_string()),
            Expr::Flashback(_) => Some("a `?` flashback".to_string()),
            Expr::Imaginary(_) => Some("a `{ }` imaginary space".to_string()),
            Expr::Bind { name, .. } => Some(format!("a `=` binding of `{name}`")),
            Expr::Square { .. } => Some("a `[ ]` mediator square".to_string()),
            Expr::Forward(..) | Expr::Backflow(..) => Some("an arrow".to_string()),
            other => Some(format!("{other:?}")),
        }
    }

    let mut broken = Vec::new();
    for (name, source) in collection() {
        let Ok(expression) = parse(&source) else {
            continue; // reported by every_module_parses
        };
        if let Some(what) = offender(&expression) {
            broken.push(format!("{name}: holds an executable form ({what})"));
        }
    }
    assert!(
        broken.is_empty(),
        "modules that are not definition-only:\n{broken:#?}"
    );
}

/// Every `std/` import names a module the standard library actually has.
///
/// A misspelled import is a clean parse and a module that can never load, and
/// the collection depends only on `std/`, so every one of its imports is
/// checkable here without a resolver or a host.
#[test]
fn every_std_import_resolves() {
    if skipped() {
        return;
    }
    let known: BTreeSet<&str> = std_modules().iter().map(|(name, _)| *name).collect();
    let available: BTreeSet<String> = collection().iter().map(|(name, _)| name.clone()).collect();

    fn imports(form: &Expr, into: &mut Vec<String>) {
        match form {
            Expr::Import { module } => into.push(module.to_string()),
            Expr::Program(forms) | Expr::Compose(forms) => {
                for inner in forms {
                    imports(inner, into);
                }
            }
            Expr::Function { body, .. } => imports(body, into),
            _ => {}
        }
    }

    let mut broken = Vec::new();
    for (name, source) in collection() {
        let Ok(expression) = parse(&source) else {
            continue;
        };
        let mut found = Vec::new();
        imports(&expression, &mut found);
        for import in found {
            let resolves = if import == "std" || import.starts_with("std/") {
                known.contains(import.as_str())
            } else {
                // A sibling in the collection, imported by its own name.
                available.contains(&import)
            };
            assert!(!import.is_empty(), "{name}: an import with no module name");
            if !resolves {
                broken.push(format!("{name}: imports `{import}`, which does not exist"));
            }
        }
    }
    assert!(broken.is_empty(), "unresolvable imports:\n{broken:#?}");
}
