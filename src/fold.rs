//! Foldable trace protocol — collapsible "what the system is doing" sections.
//!
//! A coding conclave runs *k* full agent sessions, each with many tool steps. Dumped
//! flat into the transcript that is an unreadable wall. So a command emits its detail
//! wrapped in **fold markers**: a one-line summary the reader always sees, and a body
//! they can expand or collapse. The fullscreen TUI ([`crate::tui`]) parses the markers
//! and renders real collapsible groups (collapsed by default, toggled with the
//! keyboard). Anywhere else — a pipe, CI, `--no-default-features` — the same calls
//! degrade to plain indented text, so the output is always legible.
//!
//! The markers are single lines led by an ASCII control char (`RS`, `0x1e`) that never
//! occurs in normal model or tool output, so they cannot collide with real content:
//!
//! ```text
//!   \x1e{OPEN}\x1f<summary>     begin a fold titled <summary>
//!   \x1e{CLOSE}                 end the innermost fold
//! ```
//!
//! Whether markers are emitted is gated by `KAOS_FOLD=1`, which the TUI sets on the
//! subprocesses it spawns. The child asks [`enabled`] once and picks its rendering.

use std::io::Write;

/// Record-separator lead-in for a marker line (never appears in real output).
const RS: char = '\u{1e}';
/// Unit-separator between a marker's tag and its payload.
const US: char = '\u{1f}';

const OPEN_TAG: &str = "FOLD_OPEN";
const CLOSE_TAG: &str = "FOLD_CLOSE";

/// Is fold-marker emission on? True when the parent (the TUI) set `KAOS_FOLD=1`.
pub fn enabled() -> bool {
    std::env::var("KAOS_FOLD")
        .map(|v| v == "1")
        .unwrap_or(false)
}

/// Begin a fold titled `summary`. In marker mode this emits an OPEN line the TUI
/// captures; in plain mode it prints the summary as an ordinary header so the text
/// is still readable. Depth-indenting in plain mode keeps nested folds legible.
pub fn open(summary: &str) {
    if enabled() {
        println!("{RS}{OPEN_TAG}{US}{summary}");
    } else {
        println!("{summary}");
    }
    let _ = std::io::stdout().flush();
}

/// End the innermost open fold. A no-op line in plain mode (the header already
/// stands on its own), a CLOSE marker in marker mode.
pub fn close() {
    if enabled() {
        println!("{RS}{CLOSE_TAG}");
        let _ = std::io::stdout().flush();
    }
}

/// Parse a line the TUI streamed from a child. Returns what kind of marker it is (if
/// any) so the renderer can build the fold tree. Kept here so the wire format lives
/// in exactly one place.
pub enum Marker<'a> {
    Open(&'a str),
    Close,
    /// An ordinary content line (the common case).
    Line(&'a str),
}

/// Classify one raw line (ANSI-bearing content is passed through untouched as
/// [`Marker::Line`]). Only the exact control-led marker lines are special.
pub fn classify(line: &str) -> Marker<'_> {
    if let Some(rest) = line.strip_prefix(RS) {
        if let Some(summary) = rest.strip_prefix(&format!("{OPEN_TAG}{US}")) {
            return Marker::Open(summary);
        }
        if rest == CLOSE_TAG {
            return Marker::Close;
        }
    }
    Marker::Line(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_classify() {
        let open_line = format!("{RS}{OPEN_TAG}{US}adept 1 — verified");
        match classify(&open_line) {
            Marker::Open(s) => assert_eq!(s, "adept 1 — verified"),
            _ => panic!("expected open"),
        }
        let close_line = format!("{RS}{CLOSE_TAG}");
        assert!(matches!(classify(&close_line), Marker::Close));
    }

    #[test]
    fn ordinary_lines_are_content() {
        // Coloured/ANSI output and normal text are never mistaken for markers.
        assert!(matches!(classify("  ± edit sol.py"), Marker::Line(_)));
        assert!(matches!(classify("\x1b[31mred\x1b[0m"), Marker::Line(_)));
        // A bare marker tag WITHOUT the control lead-in is just content.
        assert!(matches!(classify("FOLD_OPEN foo"), Marker::Line(_)));
    }
}
