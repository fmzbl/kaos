//! Every Rebis example in the README and docs must call macros that exist.
//!
//! Parsing is not enough: an undefined macro parses fine and only fails when
//! the program runs. These examples are what a reader copies, and — for the
//! authoring reference — what every model is handed as ground truth, so a call
//! that cannot resolve is a wrong instruction rather than a typo.
//!
//! An example may legitimately import a sigil the reader has not saved
//! (`(# repair-tools)`), so a missing MODULE is allowed. A missing MACRO is not.

use std::cell::RefCell;

struct Any(RefCell<usize>);

impl rebis_lang::Oracle for Any {
    fn fire(&self, _prompt: &str) -> Option<String> {
        *self.0.borrow_mut() += 1;
        // `1` so a `%` gate selects a branch instead of diagnosing.
        Some("1".to_string())
    }
}

/// `(start_line, source)` for each ```rebis fence in `text`.
fn rebis_examples(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut current: Option<(usize, Vec<&str>)> = None;
    for (number, line) in text.lines().enumerate() {
        match current.as_mut() {
            None if line.trim() == "```rebis" => current = Some((number + 2, Vec::new())),
            None => {}
            Some(_) if line.trim() == "```" => {
                let (start, body) = current.take().expect("open fence");
                out.push((start, body.join("\n")));
            }
            Some((_, body)) => body.push(line),
        }
    }
    out
}

#[test]
fn every_documented_example_calls_macros_that_exist() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = vec![root.join("README.md")];
    let mut docs: Vec<_> = std::fs::read_dir(root.join("docs"))
        .expect("docs/")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "md"))
        .collect();
    docs.sort();
    files.extend(docs);

    let mut checked = 0;
    let mut failures: Vec<String> = Vec::new();
    for path in files {
        let text = std::fs::read_to_string(&path).expect("read");
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        for (line, source) in rebis_examples(&text) {
            checked += 1;
            let expression = match rebis_lang::parse(&source) {
                Ok(expression) => expression,
                Err(error) => {
                    failures.push(format!("{name}:{line} does not parse: {error}"));
                    continue;
                }
            };
            let oracle = Any(RefCell::new(0));
            let mut record = rebis_lang::Record::from_texts::<&str>(&[]);
            let result = rebis_lang::orchestrate(&expression, &mut record, &oracle);
            // An example may import a sigil the reader has not saved. When that
            // module is absent the macros it would define are undefined too, so
            // the whole cascade is expected rather than a broken example.
            let absent_module = result.diagnostics.iter().any(|diagnostic| {
                matches!(
                    diagnostic,
                    rebis_lang::RuntimeDiagnostic::ModuleNotFound { .. }
                )
            });
            // The same allowance for a file. An example showing `(&: "./x.md")`
            // is showing the SHAPE of reading a file, and the reader does not
            // have that file — nor does this test run under a host that would
            // read one. A name bound to what the read did not return is the
            // same cascade a missing module causes.
            let absent_file = result.diagnostics.iter().any(|diagnostic| {
                matches!(
                    diagnostic,
                    rebis_lang::RuntimeDiagnostic::InputUnavailable { .. }
                )
            });
            for diagnostic in &result.diagnostics {
                let expected = matches!(
                    diagnostic,
                    rebis_lang::RuntimeDiagnostic::ModuleNotFound { .. }
                        | rebis_lang::RuntimeDiagnostic::InputUnavailable { .. }
                ) || (absent_module
                    && matches!(
                        diagnostic,
                        rebis_lang::RuntimeDiagnostic::UndefinedMacro { .. }
                    ))
                    || (absent_file
                        && matches!(
                            diagnostic,
                            rebis_lang::RuntimeDiagnostic::UnboundValue { .. }
                        ));
                if expected {
                    continue;
                }
                failures.push(format!("{name}:{line} {diagnostic:?}"));
            }
        }
    }

    assert!(checked >= 10, "expected the docs to carry examples");
    assert!(
        failures.is_empty(),
        "{} of {checked} examples are broken:\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
}
