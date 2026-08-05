//! The channel by which a run asks for another run beneath it.
//!
//! An agent inside a run can write a Rebis program and ask for it to be run —
//! but a run belongs to the frontend that owns the queue, not to the child
//! process doing the work. So the request travels the same way a delivered
//! input does: one per-run sidecar file, written by the child, drained by the
//! host. No process-control authority crosses into the child, and the child
//! never learns anything about the queue it is being added to.
//!
//! It sits in the core rather than beside the input inlet because both ends
//! need it and they are on opposite sides of the crate graph: the agent runtime
//! writes the request, and each frontend's run queue drains it.
//!
//! It is a file rather than a marker in the output stream on purpose. A run's
//! output is the program's own text, and a program that printed the marker —
//! quite easy, since these programs are *about* writing Rebis — would open runs
//! nobody asked for. A sidecar cannot be spoofed by anything a model says.
//!
//! Unlike the input inlet, which holds exactly one delivery, this file
//! accumulates: an agent may ask for several programs in one turn, and each is
//! a separate run. Draining takes them all and clears the file.

use std::path::{Path, PathBuf};

/// Environment variable carrying one run's nesting-request file.
pub const NEST_PATH_ENV: &str = "KAOS_REBIS_NEST";

/// Separates one request from the next in the sidecar.
///
/// A line that cannot occur in Rebis source: the language has no `\x1e`, so a
/// program cannot write a separator into its own request and split it in two.
const SEPARATOR: &str = "\x1e\n";

/// Separates a request's note from its program.
const FIELD: &str = "\x1f\n";

/// One program a run asked to have run beneath it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    /// What the asking agent said it is for; becomes the child run's label.
    pub note: String,
    /// The program, exactly as written.
    pub source: String,
}

/// Resolve the request-file path once when a hosted child is constructed.
#[must_use]
pub fn path_from_env() -> Option<PathBuf> {
    std::env::var_os(NEST_PATH_ENV).map(PathBuf::from)
}

/// Ask the host for a run beneath this one (called by the child).
///
/// Appends, so several requests in one turn all survive. A request with no
/// program is refused here rather than becoming an empty run the reader has to
/// work out the meaning of.
///
/// # Errors
///
/// Returns an I/O error when the request cannot be recorded.
pub fn request(path: &Path, note: &str, source: &str) -> std::io::Result<()> {
    let source = source.trim();
    if source.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "a nested run needs a program",
        ));
    }
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let entry = format!("{}{FIELD}{source}", note.trim());
    let joined = if existing.is_empty() {
        entry
    } else {
        format!("{existing}{SEPARATOR}{entry}")
    };
    std::fs::write(path, joined)
}

/// Take every request a run has made, clearing them (called by the host).
///
/// Returns them in the order they were asked for. A missing file means nothing
/// has been asked, which is the normal case on every poll.
#[must_use]
pub fn drain(path: &Path) -> Vec<Request> {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    // Cleared before parsing: a malformed file must not be re-read forever.
    let _ = std::fs::remove_file(path);
    contents
        .split(SEPARATOR)
        .filter_map(|entry| {
            let (note, source) = entry.split_once(FIELD)?;
            let source = source.trim();
            (!source.is_empty()).then(|| Request {
                note: note.trim().to_string(),
                source: source.to_string(),
            })
        })
        .collect()
}

/// The label a nested run is shown under.
///
/// The agent's own note when it gave one, and an honest stand-in when it did
/// not — an unlabelled run in a tree is worse than a dull label.
#[must_use]
pub fn label(request: &Request, parent: u64) -> String {
    if request.note.is_empty() {
        format!("opened by run #{parent}")
    } else {
        format!("#{parent}: {}", request.note)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kaos-nest-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(name);
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn requests_round_trip_in_the_order_they_were_asked() {
        let path = scratch("order.nest");
        assert!(drain(&path).is_empty(), "nothing asked for yet");

        request(&path, "answer the comments", "(\"one\")").unwrap();
        request(&path, "make the changes", "(\"two\")").unwrap();
        let taken = drain(&path);
        assert_eq!(
            taken,
            vec![
                Request {
                    note: "answer the comments".into(),
                    source: "(\"one\")".into()
                },
                Request {
                    note: "make the changes".into(),
                    source: "(\"two\")".into()
                },
            ]
        );
        // Draining clears: the same run is not opened twice on the next poll.
        assert!(drain(&path).is_empty());
    }

    #[test]
    fn a_program_cannot_forge_a_second_request_out_of_its_own_text() {
        let path = scratch("forge.nest");
        // A program containing what looks like a separator, and a note that
        // does too. One request in, one request out.
        let sneaky = "(\"a\\x1e\\nb\" \"c\\x1f\\nd\")";
        request(&path, "one\x1e\nfake", sneaky).unwrap();
        let taken = drain(&path);
        assert_eq!(taken.len(), 1, "{taken:?}");
        assert_eq!(taken[0].source, sneaky);
    }

    #[test]
    fn an_empty_program_is_refused_rather_than_queued() {
        let path = scratch("empty.nest");
        assert!(request(&path, "nothing", "   ").is_err());
        assert!(drain(&path).is_empty(), "an empty ask left no run behind");
    }

    #[test]
    fn a_malformed_file_is_cleared_rather_than_read_forever() {
        let path = scratch("junk.nest");
        std::fs::write(&path, "not a request at all").unwrap();
        assert!(drain(&path).is_empty());
        assert!(
            !path.exists(),
            "the bad file must not be re-read every poll"
        );
    }

    #[test]
    fn every_nested_run_is_labelled_even_when_the_agent_said_nothing() {
        let named = Request {
            note: "fix the retry queue".into(),
            source: "(\"x\")".into(),
        };
        assert_eq!(label(&named, 7), "#7: fix the retry queue");
        let bare = Request {
            note: String::new(),
            source: "(\"x\")".into(),
        };
        assert_eq!(label(&bare, 7), "opened by run #7");
    }
}
