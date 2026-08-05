//! Every environment variable Kaos reads is documented, in one place.
//!
//! There are two inventories and the split is deliberate:
//!
//! - [`CONFIG_DOCS`] — persistent settings a person can save
//! - [`ENVIRONMENT_DOCS`] — credentials, private child-process flags, and
//!   hosted-run transport paths, which are read from the environment and must
//!   never be written to a config file
//!
//! What was missing is the guarantee that between them they cover everything.
//! `ENVIRONMENT_DOCS` also lived inside the visual frontend, so the terminal
//! never showed it and the two could drift with nothing to notice.
//!
//! This scans the source for every variable actually read and fails on any that
//! is in neither list. A variable nobody documented is one a user cannot
//! discover, and one nobody remembers to remove.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use kaos_core::config::{CONFIG_DOCS, ENVIRONMENT_DOCS};

/// Every `.rs` file in the workspace, excluding build output.
fn sources(root: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if name != "target" && !name.starts_with('.') {
                sources(&path, found);
            }
        } else if path.extension().is_some_and(|e| e == "rs") {
            found.push(path);
        }
    }
}

/// Variable names passed to `env::var` / `env::var_os` as a literal.
fn read_from_environment() -> BTreeSet<String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .to_path_buf();
    let mut files = Vec::new();
    sources(&root, &mut files);

    let mut names = BTreeSet::new();
    for file in files {
        // This test's own scanning strings would otherwise register as reads.
        if file.ends_with("env_inventory.rs") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        for marker in ["env::var(\"", "env::var_os(\""] {
            let mut rest = text.as_str();
            while let Some(at) = rest.find(marker) {
                rest = &rest[at + marker.len()..];
                if let Some(end) = rest.find('"') {
                    let name = &rest[..end];
                    // Only the families Kaos owns or overrides. `HOME`, `PATH`
                    // and the like are the operating system's business.
                    if name.starts_with("KAOS_")
                        || name.starts_with("OLLAMA_")
                        || name.starts_with("OPENAI_")
                        || name.starts_with("OPENROUTER_")
                        || name.starts_with("ANTHROPIC_")
                        || name.starts_with("REBIS_")
                    {
                        names.insert(name.to_string());
                    }
                }
            }
        }
    }
    names
}

#[test]
fn every_environment_variable_kaos_reads_is_documented_somewhere() {
    let documented: BTreeSet<&str> = CONFIG_DOCS
        .iter()
        .map(|doc| doc.key)
        .chain(ENVIRONMENT_DOCS.iter().map(|doc| doc.key))
        .collect();

    let read = read_from_environment();
    assert!(
        read.len() > 20,
        "only {} variables found — the scan is broken, not the inventory",
        read.len()
    );

    let undocumented: Vec<&String> = read
        .iter()
        .filter(|name| !documented.contains(name.as_str()))
        .collect();
    assert!(
        undocumented.is_empty(),
        "read from the environment but in neither CONFIG_DOCS nor ENVIRONMENT_DOCS: {undocumented:#?}"
    );
}

/// The two inventories must not overlap.
///
/// A key in both is a key whose status is ambiguous — settable or not — and the
/// whole point of the split is that a credential can never be written to the
/// config file by someone reading the wrong list.
#[test]
fn nothing_is_both_a_setting_and_environment_only() {
    let settings: BTreeSet<&str> = CONFIG_DOCS.iter().map(|doc| doc.key).collect();
    let both: Vec<&str> = ENVIRONMENT_DOCS
        .iter()
        .map(|doc| doc.key)
        .filter(|key| settings.contains(key))
        .collect();
    assert!(both.is_empty(), "in both inventories: {both:?}");
}

/// A credential must never appear among the persistent settings, because those
/// are written to a file on disk in plain text.
#[test]
fn no_credential_is_a_persistent_setting() {
    let leaked: Vec<&str> = CONFIG_DOCS
        .iter()
        .map(|doc| doc.key)
        // `_TOKEN` as a suffix, not anywhere — `KAOS_MAX_TOKENS` is a budget,
        // not a credential, and a pattern that cannot tell them apart would
        // train people to ignore this.
        .filter(|key| {
            key.ends_with("_API_KEY") || key.ends_with("_TOKEN") || key.ends_with("_SECRET")
        })
        .collect();
    assert!(
        leaked.is_empty(),
        "these would be written to the config file in plain text: {leaked:?}"
    );
}

#[test]
fn every_documented_variable_says_what_it_is_for() {
    for doc in ENVIRONMENT_DOCS {
        assert!(
            doc.details.len() > 20,
            "{} needs a real description",
            doc.key
        );
    }
    for doc in CONFIG_DOCS {
        assert!(!doc.summary.is_empty(), "{} needs a summary", doc.key);
        assert!(doc.details.len() > 20, "{} needs details", doc.key);
    }
}

/// The config file's environment section must name every environment-only
/// variable.
///
/// It is a comment block, which is precisely the kind of thing that rots: a
/// variable gets added to the code and the note beside it does not. `/config`
/// in the terminal opens this file rather than a rendered pane, so for a
/// terminal user this comment IS the documentation.
#[test]
fn the_config_file_lists_every_environment_only_variable() {
    let template = kaos_core::config::DEFAULT_CONFIG;
    let missing: Vec<&str> = ENVIRONMENT_DOCS
        .iter()
        .map(|doc| doc.key)
        .filter(|key| !template.contains(key))
        .collect();
    assert!(
        missing.is_empty(),
        "the config file's environment section does not name: {missing:#?}"
    );
}

/// And the reverse: the file must not name something that is gone.
#[test]
fn the_config_file_names_nothing_that_no_longer_exists() {
    let template = kaos_core::config::DEFAULT_CONFIG;
    let known: BTreeSet<&str> = CONFIG_DOCS
        .iter()
        .map(|doc| doc.key)
        .chain(ENVIRONMENT_DOCS.iter().map(|doc| doc.key))
        .collect();
    let stale: Vec<String> = template
        .split_whitespace()
        .filter(|word| {
            word.starts_with("KAOS_")
                || word.starts_with("OLLAMA_")
                || word.starts_with("OPENAI_")
                || word.starts_with("OPENROUTER_")
                || word.starts_with("ANTHROPIC_")
                || word.starts_with("REBIS_")
        })
        .filter(|word| !known.contains(*word))
        .map(std::string::ToString::to_string)
        .collect();
    assert!(
        stale.is_empty(),
        "named in the file but not documented: {stale:#?}"
    );
}

/// `docs/CONFIGURATION.md` must name every environment-only variable too.
///
/// Three surfaces show this list — the visual Settings tab, the config file's
/// comment block, and the reference doc — and all three now read from one
/// inventory except the doc, which is prose. So the doc gets a test rather
/// than a promise.
#[test]
fn the_reference_documentation_lists_every_environment_only_variable() {
    let doc = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .join("docs/CONFIGURATION.md");
    let Ok(text) = std::fs::read_to_string(&doc) else {
        // A checkout without the docs still builds; there is nothing to check.
        return;
    };
    let missing: Vec<&str> = ENVIRONMENT_DOCS
        .iter()
        .map(|entry| entry.key)
        .filter(|key| !text.contains(key))
        .collect();
    assert!(
        missing.is_empty(),
        "docs/CONFIGURATION.md does not mention: {missing:#?}"
    );
}
