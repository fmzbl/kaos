//! The sigil library as a [`Toolset`], so a chat can work in it.
//!
//! [`kaos_core::scribe`] decides what an act means and whether it is allowed;
//! this is the thin layer that lets [`crate::familiar`]'s loop call it. Keeping
//! them apart matters: the rules about what a model may write to the library
//! are a property of the library, not of whichever agent happens to be asking,
//! and a second frontend must not be able to acquire different ones by
//! implementing its own toolset.
//!
//! So everything here is plumbing. The catalogue is prose for the system
//! prompt, `invoke` parses arguments and hands them on, and every refusal comes
//! back from the scribe unchanged.

use std::collections::BTreeMap;

use kaos_core::scribe::{self, Act};
use kaos_core::sigils::Library;

use crate::familiar::Toolset;

/// A familiar's access to the sigil library.
///
/// Holds the library rather than a path so a caller can point one at a
/// throwaway directory in a test, or at a second library, without this knowing
/// the difference.
pub struct Scriptorium {
    library: Library,
}

impl Scriptorium {
    #[must_use]
    pub fn new(library: Library) -> Self {
        Self { library }
    }

    /// The user's own library at `~/.kaos/sigils`.
    #[must_use]
    pub fn default_library() -> Self {
        Self::new(Library::default_library())
    }
}

impl Toolset for Scriptorium {
    fn catalogue(&self) -> String {
        // Deliberately states the two rules a model will otherwise discover by
        // being refused: what is writable, and that source must parse. A tool
        // description that omits its own constraints buys one wasted turn per
        // constraint.
        "- sigil_list: args query — sigils whose name contains the query; empty lists everything\n\
         - sigil_read: args name — the complete source of one sigil\n\
         - sigil_edit: args name, find, replace — replace one exact passage; `find` must appear exactly once\n\
         - sigil_write: args name, source — create or replace a sigil outright\n\
         Everything in the catalogue is readable, std/ and the collection included. Only personal \
         sigils are writable. A write or edit must parse as Rebis or it is refused with the parse \
         error. Read a sigil before editing it, and prefer sigil_edit to sigil_write — rewriting a \
         whole file is where parts get dropped.\n"
            .to_string()
    }

    fn invoke(&self, tool: &str, args: &BTreeMap<String, String>) -> String {
        let get = |key: &str| args.get(key).cloned().unwrap_or_default();
        let act = match tool {
            "sigil_list" | "sigils" => Act::List { query: get("query") },
            "sigil_read" | "sigil_open" => Act::Read { name: get("name") },
            "sigil_write" | "sigil_save" => Act::Write {
                name: get("name"),
                source: get("source"),
            },
            "sigil_edit" => Act::Edit {
                name: get("name"),
                find: get("find"),
                replace: get("replace"),
            },
            // The loop treats an `error:` observation as a negative one and
            // lets the model try again, which is the right response to a name
            // it invented.
            other => return format!("error: no tool named {other:?}"),
        };
        scribe::perform(&self.library, &act).report()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scriptorium() -> Scriptorium {
        let dir = std::env::temp_dir().join(format!(
            "kaos-scriptorium-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        Scriptorium::new(Library::new(dir))
    }

    fn args(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn a_write_then_a_read_round_trips_through_the_toolset() {
        let scriptorium = scriptorium();
        let wrote = scriptorium.invoke(
            "sigil_write",
            &args(&[("name", "team/reviews"), ("source", r#"(-> "one" "two")"#)]),
        );
        assert_eq!(wrote, "created team/reviews");
        let read = scriptorium.invoke("sigil_read", &args(&[("name", "team/reviews")]));
        assert!(read.contains(r#"(-> "one" "two")"#), "{read}");
    }

    #[test]
    fn the_scribe_s_refusals_reach_the_model_unchanged() {
        let scriptorium = scriptorium();
        let refused = scriptorium.invoke(
            "sigil_write",
            &args(&[("name", "std/flow"), ("source", r#""x""#)]),
        );
        assert!(refused.starts_with("refused"), "{refused}");
        assert!(refused.contains("read-only"), "{refused}");

        let unparseable = scriptorium.invoke(
            "sigil_write",
            &args(&[("name", "broken"), ("source", "(((")]),
        );
        assert!(unparseable.contains("does not parse"), "{unparseable}");
    }

    #[test]
    fn an_unknown_tool_is_an_error_the_loop_can_recover_from() {
        let scriptorium = scriptorium();
        let answer = scriptorium.invoke("sigil_delete", &args(&[("name", "x")]));
        assert!(answer.starts_with("error:"), "{answer}");
    }

    /// The whole path, through the real agent loop: the model reads a sigil,
    /// edits it, and answers — and the library on disk actually changed.
    ///
    /// The pieces are tested apart; this is the one that would catch them
    /// being wired together wrongly.
    #[test]
    fn a_familiar_can_read_a_sigil_edit_it_and_report() {
        use crate::conductor::ScriptedChat;
        use crate::familiar::converse;

        let scriptorium = scriptorium();
        scriptorium
            .library
            .save("team/reviews", "(-> \"draft it\" \"check it\")")
            .expect("seed the library");

        let chat = ScriptedChat::new(vec![
            "<act tool=\"sigil_read\"><arg name=\"name\">team/reviews</arg></act>",
            "<act tool=\"sigil_edit\"><arg name=\"name\">team/reviews</arg>\
             <arg name=\"find\">check it</arg><arg name=\"replace\">verify it</arg></act>",
            "<act tool=\"finish\"><arg name=\"message\">Tightened the second stage.</arg></act>",
        ]);

        let talk = converse("You tend the sigil library.", "Improve team/reviews.", &scriptorium, &chat, 6);

        assert!(talk.error.is_none(), "{:?}", talk.error);
        assert_eq!(talk.steps.len(), 3, "read, edit, finish: {:?}", talk.steps);
        assert!(talk.answer.contains("Tightened"), "{}", talk.answer);
        assert_eq!(
            scriptorium.library.load("team/reviews").unwrap(),
            "(-> \"draft it\" \"verify it\")",
            "the edit did not reach disk"
        );
    }

    #[test]
    fn the_catalogue_states_the_rules_a_model_would_otherwise_hit() {
        let catalogue = scriptorium().catalogue();
        assert!(catalogue.contains("Only personal sigils are writable"));
        assert!(catalogue.contains("parse"));
        // The loop adds `finish` itself; listing it here would double it.
        assert!(!catalogue.contains("finish"));
    }
}
