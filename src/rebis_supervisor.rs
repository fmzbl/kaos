//! Per-run directives written by the `/sigil chat` supervisor.
//!
//! The TUI owns and validates control actions. A hosted Rebis child only reads
//! its assigned directive file before an unfinished model prompt, so completed
//! checkpoint entries remain immutable and no process-control authority crosses
//! the child boundary.

use std::path::{Path, PathBuf};

/// Environment variable carrying one run's supervisor-directive file.
pub const DIRECTIVE_PATH_ENV: &str = "KAOS_REBIS_DIRECTIVE";

/// Read a non-empty directive. Missing, unreadable, and whitespace-only files
/// deliberately mean “no directive” so ordinary and non-TUI runs are unchanged.
#[must_use]
pub fn read_directive(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Resolve the directive path once when a hosted child is constructed.
#[must_use]
pub fn path_from_env() -> Option<PathBuf> {
    std::env::var_os(DIRECTIVE_PATH_ENV).map(PathBuf::from)
}

/// Add a clearly delimited supervisor instruction to one unfinished node.
#[must_use]
pub fn directed_prompt(prompt: &str, directive: Option<&str>) -> String {
    directive.map_or_else(
        || prompt.to_string(),
        |directive| {
            format!(
                "{prompt}\n\nSUPERVISOR DIRECTIVE:\n{directive}\n\nFollow this directive while completing the node above."
            )
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_nonempty_directive_is_delimited_without_changing_the_base_prompt() {
        let rendered = directed_prompt("inspect", Some("compare run two"));
        assert!(rendered.starts_with("inspect\n\nSUPERVISOR DIRECTIVE:"));
        assert!(rendered.contains("compare run two"));
        assert_eq!(directed_prompt("inspect", None), "inspect");
    }
}
