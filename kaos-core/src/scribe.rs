//! Letting a chat read and write the sigil library.
//!
//! Chat could already explain Rebis and could already see one bound run's
//! source. What it could not do is reach the library: open a sigil it was
//! asked about, or change one it had just diagnosed. Every such conversation
//! ended with the model describing an edit and a person retyping it.
//!
//! This is the seam that closes that. It is deliberately a *scribe* rather than
//! an editor: it parses what a reply asked for, checks it, performs it, and
//! reports what happened in words the model reads next turn. Nothing here
//! decides anything — the model proposes edits, this performs the ones that are
//! legal and refuses the rest out loud.
//!
//! Three rules hold, and they are the whole safety story:
//!
//! 1. **Everything in the catalog is readable.** The embedded `std/` modules
//!    and the vendored collection included — a model asked to improve a sigil
//!    should be able to read the library it imports.
//! 2. **Only personal sigils are writable.** `std/` and the collection are the
//!    language's own, and a model editing them would change what every other
//!    program means. [`Library::save`] already refuses; this reports the refusal
//!    rather than letting it look like a failed write.
//! 3. **A write must parse first.** This is the rule with teeth. Kaos will
//!    happily save unparseable source for a person — a half-finished edit is a
//!    legitimate thing to keep — but a model writing a sigil nobody is watching
//!    is different: an unparseable sigil is one that can never be imported, and
//!    it would fail later, somewhere else, in a program that merely mentions
//!    it. The parse error goes back to the model instead.
//!
//! The reply protocol is the `<act>` block the rest of Kaos already uses, so a
//! model that can drive the Rebis conductor can drive this without learning a
//! second syntax.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::sigils::{Library, SigilError};

/// How many acts one reply may perform.
///
/// A bound on a runaway turn, not on ambition: read a few, write one, report.
/// A reply asking for more than this has usually misunderstood the task rather
/// than found a lot of work to do.
pub const MAX_ACTS: usize = 12;

/// One thing a reply asked to do to the library.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Act {
    /// Names in the catalog, optionally filtered.
    List { query: String },
    /// The complete source of one sigil.
    Read { name: String },
    /// Create or replace a sigil outright.
    Write { name: String, source: String },
    /// Replace one exact passage inside a sigil.
    ///
    /// Separate from [`Act::Write`] because a model that has just read a long
    /// sigil should not have to reproduce all of it to change four lines — and
    /// because reproducing it is where a model quietly drops the parts it did
    /// not think were interesting.
    Edit {
        name: String,
        find: String,
        replace: String,
    },
}

/// What became of one act.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scribed {
    Listed { query: String, names: Vec<String> },
    Read { name: String, source: String },
    Wrote { name: String, path: PathBuf, created: bool },
    Edited { name: String, path: PathBuf },
    /// The act was legal to ask for and could not be done. Always says why:
    /// a refusal a model cannot read is a refusal it will repeat.
    Refused { name: String, why: String },
}

impl Scribed {
    /// Whether the library changed.
    #[must_use]
    pub fn changed(&self) -> bool {
        matches!(self, Scribed::Wrote { .. } | Scribed::Edited { .. })
    }

    /// One line for the transcript, and for the model's next turn.
    #[must_use]
    pub fn report(&self) -> String {
        match self {
            Scribed::Listed { query, names } if names.is_empty() => {
                format!("no sigil matches {query:?}")
            }
            Scribed::Listed { names, .. } => format!("sigils: {}", names.join(", ")),
            Scribed::Read { name, source } => format!("{name}:\n{source}"),
            Scribed::Wrote {
                name,
                created: true,
                ..
            } => format!("created {name}"),
            Scribed::Wrote { name, .. } => format!("rewrote {name}"),
            Scribed::Edited { name, .. } => format!("edited {name}"),
            Scribed::Refused { name, why } => format!("refused {name}: {why}"),
        }
    }
}

/// Parse every `<act>` block in a reply, in order.
///
/// Unknown tools are skipped rather than reported: this parser runs over an
/// ordinary chat reply, and a model writing about `<act>` blocks in prose
/// should not be treated as having cast one.
#[must_use]
pub fn parse_acts(reply: &str) -> Vec<Act> {
    let mut acts = Vec::new();
    let mut rest = reply;
    while acts.len() < MAX_ACTS {
        let Some(start) = rest.find("<act") else {
            break;
        };
        let after = &rest[start..];
        let end = after
            .find("</act>")
            .map_or(rest.len(), |i| start + i + "</act>".len());
        if let Some(act) = parse_act(&rest[start..end]) {
            acts.push(act);
        }
        rest = &rest[end..];
    }
    acts
}

fn parse_act(text: &str) -> Option<Act> {
    let start = text.find("<act")?;
    let open_end = text[start..].find('>')? + start;
    let tool = attribute(&text[start..open_end], "tool")?;
    let body_end = text[open_end..]
        .find("</act>")
        .map_or(text.len(), |i| i + open_end);
    let args = parse_args(&text[open_end + 1..body_end]);
    let get = |key: &str| args.get(key).cloned().unwrap_or_default();
    match tool.as_str() {
        "sigil_list" | "sigils" => Some(Act::List { query: get("query") }),
        "sigil_read" | "sigil_open" => Some(Act::Read { name: get("name") }),
        "sigil_write" | "sigil_save" => Some(Act::Write {
            name: get("name"),
            source: get("source"),
        }),
        "sigil_edit" => Some(Act::Edit {
            name: get("name"),
            find: get("find"),
            replace: get("replace"),
        }),
        _ => None,
    }
}

fn attribute(header: &str, key: &str) -> Option<String> {
    let needle = format!("{key}=\"");
    let start = header.find(&needle)? + needle.len();
    let end = header[start..].find('"')? + start;
    Some(header[start..end].to_string())
}

fn parse_args(body: &str) -> BTreeMap<String, String> {
    let mut args = BTreeMap::new();
    let mut rest = body;
    while let Some(open) = rest.find("<arg name=\"") {
        let name_start = open + "<arg name=\"".len();
        let Some(quote) = rest[name_start..].find('"') else {
            break;
        };
        let name = rest[name_start..name_start + quote].to_string();
        let Some(gt) = rest[name_start + quote..].find('>') else {
            break;
        };
        let value_start = name_start + quote + gt + 1;
        let Some(close) = rest[value_start..].find("</arg>") else {
            break;
        };
        // Trim only newlines: a sigil's leading indentation is source, and an
        // argument that trimmed it would silently reformat what it saved.
        args.insert(
            name,
            rest[value_start..value_start + close]
                .trim_matches('\n')
                .to_string(),
        );
        rest = &rest[value_start + close + "</arg>".len()..];
    }
    args
}

/// Perform one act against the library.
///
/// Never panics and never propagates an error: every outcome, including every
/// refusal, comes back as a [`Scribed`] the model can read. A chat that failed
/// silently would have the model assume its edit landed.
pub fn perform(library: &Library, act: &Act) -> Scribed {
    match act {
        Act::List { query } => Scribed::Listed {
            query: query.clone(),
            names: library
                .search_catalog(query)
                .into_iter()
                .map(|entry| entry.name)
                .collect(),
        },
        Act::Read { name } => match library.load_catalog(name) {
            Ok(source) => Scribed::Read {
                name: name.clone(),
                source,
            },
            Err(error) => refuse(name, &error),
        },
        Act::Write { name, source } => {
            if let Err(why) = readable_as_rebis(source) {
                return Scribed::Refused {
                    name: name.clone(),
                    why,
                };
            }
            let created = !library.exists(name);
            match library.save(name, source) {
                Ok(path) => Scribed::Wrote {
                    name: name.clone(),
                    path,
                    created,
                },
                Err(error) => refuse(name, &error),
            }
        }
        Act::Edit {
            name,
            find,
            replace,
        } => {
            let current = match library.load(name) {
                Ok(source) => source,
                Err(error) => return refuse(name, &error),
            };
            // Exactly once, or not at all. A find that matches twice is a
            // model that has not read carefully enough to know which one it
            // meant, and picking either for it is how the wrong line changes.
            let hits = current.matches(find.as_str()).count();
            let why = match hits {
                0 => Some("that passage is not in the sigil".to_string()),
                1 => None,
                n => Some(format!("that passage appears {n} times; make it unique")),
            };
            if let Some(why) = why {
                return Scribed::Refused {
                    name: name.clone(),
                    why,
                };
            }
            let edited = current.replacen(find.as_str(), replace, 1);
            if let Err(why) = readable_as_rebis(&edited) {
                return Scribed::Refused {
                    name: name.clone(),
                    why,
                };
            }
            match library.save(name, &edited) {
                Ok(path) => Scribed::Edited {
                    name: name.clone(),
                    path,
                },
                Err(error) => refuse(name, &error),
            }
        }
    }
}

/// Perform a whole reply's worth of acts, in order.
#[must_use]
pub fn perform_all(library: &Library, acts: &[Act]) -> Vec<Scribed> {
    acts.iter().map(|act| perform(library, act)).collect()
}

fn refuse(name: &str, error: &SigilError) -> Scribed {
    Scribed::Refused {
        name: name.to_string(),
        why: match error {
            SigilError::Reserved => {
                "that name belongs to the standard library or the collection, \
                 which are read-only — save it under a personal name instead"
                    .to_string()
            }
            other => other.to_string(),
        },
    }
}

/// A sigil that does not parse is one that can never be imported.
///
/// Kaos lets a PERSON save unparseable source — a half-finished edit is a
/// legitimate thing to keep, and the editor shows the error. A model writing
/// into the library unattended is a different case: nothing is watching, and
/// the failure would surface later in whichever program imported it. So the
/// parse error goes back to the model, which is the one that can fix it.
fn readable_as_rebis(source: &str) -> Result<(), String> {
    if source.trim().is_empty() {
        return Err("a sigil needs source; an empty one would import nothing".to_string());
    }
    rebis_lang::parse(source)
        .map(|_| ())
        .map_err(|error| format!("that source does not parse: {error}"))
}

/// What the model is told it can do, for the system contract.
///
/// Written as the acts themselves rather than as prose about them: a model
/// shown the exact shape it must emit gets it right far more often than one
/// told about it.
#[must_use]
pub fn contract() -> String {
    "\nYou can read and edit the sigil library. Emit an <act> block to do it; \
     the result comes back to you on the next turn.\n\
     \n\
     <act tool=\"sigil_list\"><arg name=\"query\">repair</arg></act>\n\
     <act tool=\"sigil_read\"><arg name=\"name\">team/reviews</arg></act>\n\
     <act tool=\"sigil_edit\"><arg name=\"name\">team/reviews</arg>\
     <arg name=\"find\">exact text</arg><arg name=\"replace\">new text</arg></act>\n\
     <act tool=\"sigil_write\"><arg name=\"name\">team/reviews</arg>\
     <arg name=\"source\">(complete Rebis program)</arg></act>\n\
     \n\
     Everything in the catalog is readable, including std/ and the collection. \
     Only personal sigils are writable — std/ and the collection are read-only, \
     and a write there is refused. A write must parse as Rebis or it is refused \
     with the parse error. Prefer sigil_edit over sigil_write when changing part \
     of a sigil: rewriting a whole file is where parts get dropped. Read a sigil \
     before editing it.\n"
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway library, the way `sigils`' own tests make one — this crate
    /// carries no temp-dir dependency and one test module should not add it.
    fn library() -> Library {
        let dir = std::env::temp_dir().join(format!(
            "kaos-scribe-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        Library::new(dir)
    }

    #[test]
    fn a_reply_can_ask_for_several_things_in_order() {
        let acts = parse_acts(
            r#"Let me look first.
            <act tool="sigil_read"><arg name="name">team/reviews</arg></act>
            Then change it:
            <act tool="sigil_edit"><arg name="name">team/reviews</arg>
            <arg name="find">old</arg><arg name="replace">new</arg></act>"#,
        );
        assert_eq!(
            acts,
            vec![
                Act::Read {
                    name: "team/reviews".into()
                },
                Act::Edit {
                    name: "team/reviews".into(),
                    find: "old".into(),
                    replace: "new".into(),
                },
            ]
        );
    }

    #[test]
    fn prose_about_acts_is_not_an_act() {
        // This parser runs over ordinary chat replies. A model explaining the
        // protocol must not be treated as having used it.
        assert!(parse_acts("You would write an <act tool=\"nonsense\"> block.").is_empty());
        assert!(parse_acts("no blocks here at all").is_empty());
    }

    #[test]
    fn a_write_that_does_not_parse_is_refused_with_the_parse_error() {
        // The rule with teeth. A person may save a half-finished edit; a model
        // writing unattended may not, because the failure would surface later
        // in whichever program imported it.
        let library = library();
        let outcome = perform(
            &library,
            &Act::Write {
                name: "broken".into(),
                source: "((( not rebis".into(),
            },
        );
        match &outcome {
            Scribed::Refused { why, .. } => assert!(why.contains("does not parse"), "{why}"),
            other => panic!("expected a refusal, got {other:?}"),
        }
        assert!(!library.exists("broken"), "nothing should have been saved");
    }

    #[test]
    fn a_write_that_parses_lands_and_says_whether_it_created() {
        let library = library();
        let source = r#"(-> "one" "two")"#;
        let first = perform(
            &library,
            &Act::Write {
                name: "team/reviews".into(),
                source: source.into(),
            },
        );
        assert!(matches!(first, Scribed::Wrote { created: true, .. }), "{first:?}");
        assert_eq!(library.load("team/reviews").unwrap(), source);

        let again = perform(
            &library,
            &Act::Write {
                name: "team/reviews".into(),
                source: r#"(-> "one" "three")"#.into(),
            },
        );
        assert!(
            matches!(again, Scribed::Wrote { created: false, .. }),
            "a replacement should not claim to have created: {again:?}"
        );
    }

    #[test]
    fn std_is_readable_and_never_writable() {
        let library = library();
        // Readable: a model improving a sigil should see what it imports.
        match perform(&library, &Act::Read { name: "std/flow".into() }) {
            Scribed::Read { source, .. } => assert!(source.contains("std-twice"), "{source}"),
            other => panic!("std/ should be readable: {other:?}"),
        }
        // Writable: never.
        let outcome = perform(
            &library,
            &Act::Write {
                name: "std/flow".into(),
                source: r#"(-> "a" "b")"#.into(),
            },
        );
        match &outcome {
            Scribed::Refused { why, .. } => assert!(why.contains("read-only"), "{why}"),
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn an_edit_must_match_exactly_once() {
        let library = library();
        library
            .save("dup", "(\n  \"same\"\n  \"same\"\n  \"other\")")
            .expect("save");

        // Twice is ambiguous, and picking one is how the wrong line changes.
        let ambiguous = perform(
            &library,
            &Act::Edit {
                name: "dup".into(),
                find: "\"same\"".into(),
                replace: "\"changed\"".into(),
            },
        );
        match &ambiguous {
            Scribed::Refused { why, .. } => assert!(why.contains("appears 2 times"), "{why}"),
            other => panic!("expected a refusal, got {other:?}"),
        }

        // Absent is reported too, rather than silently doing nothing.
        let missing = perform(
            &library,
            &Act::Edit {
                name: "dup".into(),
                find: "\"absent\"".into(),
                replace: "x".into(),
            },
        );
        assert!(matches!(missing, Scribed::Refused { .. }), "{missing:?}");

        // Unique lands.
        let ok = perform(
            &library,
            &Act::Edit {
                name: "dup".into(),
                find: "\"other\"".into(),
                replace: "\"changed\"".into(),
            },
        );
        assert!(matches!(ok, Scribed::Edited { .. }), "{ok:?}");
        assert!(library.load("dup").unwrap().contains("changed"));
    }

    #[test]
    fn an_edit_that_would_break_the_sigil_is_refused_and_changes_nothing() {
        let library = library();
        let original = r#"(-> "one" "two")"#;
        library.save("fine", original).expect("save");
        let outcome = perform(
            &library,
            &Act::Edit {
                name: "fine".into(),
                find: r#""two")"#.into(),
                replace: r#""two""#.into(), // drops the closing paren
            },
        );
        match &outcome {
            Scribed::Refused { why, .. } => assert!(why.contains("does not parse"), "{why}"),
            other => panic!("expected a refusal, got {other:?}"),
        }
        assert_eq!(
            library.load("fine").unwrap(),
            original,
            "a refused edit must leave the sigil untouched"
        );
    }

    #[test]
    fn a_name_that_climbs_out_of_the_library_is_refused() {
        // Names are data. A sigil called `../../.ssh/id_rsa` must not reach out.
        let library = library();
        let outcome = perform(
            &library,
            &Act::Write {
                name: "../escaped".into(),
                source: r#""x""#.into(),
            },
        );
        assert!(matches!(outcome, Scribed::Refused { .. }), "{outcome:?}");
    }

    #[test]
    fn every_outcome_reports_something_a_model_can_act_on() {
        let library = library();
        let refusal = perform(
            &library,
            &Act::Read {
                name: "nothing/here".into(),
            },
        );
        let report = refusal.report();
        assert!(report.starts_with("refused"), "{report}");
        assert!(report.len() > "refused nothing/here: ".len(), "{report}");
        assert!(!refusal.changed());
    }
}
