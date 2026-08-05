//! The shared wire format for collapsible model-work sections.
//!
//! The terminal and visual front ends render the same child-process stream. A
//! record-separator marker starts a section and a matching close marker ends
//! it; ordinary model/tool text remains ordinary text. Keeping classification
//! here prevents either front end from drifting away from the protocol.

const RS: char = '\u{1e}';
#[cfg(test)]
const US: char = '\u{1f}';
#[cfg(test)]
const OPEN_TAG: &str = "FOLD_OPEN";
const CLOSE_TAG: &str = "FOLD_CLOSE";

/// One line from a child stream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Marker<'a> {
    /// Begin a section with this visible summary.
    Open(&'a str),
    /// End the innermost section.
    Close,
    /// Ordinary output, including ANSI-bearing output.
    Line(&'a str),
}

/// Classify one raw stream line. Only an exact control-led marker is special;
/// a model mentioning the marker words in normal prose remains content.
#[must_use]
pub fn classify(line: &str) -> Marker<'_> {
    if let Some(rest) = line.strip_prefix(RS) {
        if let Some(summary) = rest.strip_prefix("FOLD_OPEN\u{1f}") {
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
    fn classifies_the_wire_markers() {
        assert_eq!(
            classify(&format!("{RS}{OPEN_TAG}{US}model turn 1")),
            Marker::Open("model turn 1")
        );
        assert_eq!(classify(&format!("{RS}{CLOSE_TAG}")), Marker::Close);
    }

    #[test]
    fn leaves_plain_text_alone() {
        assert_eq!(
            classify("FOLD_OPEN not a marker"),
            Marker::Line("FOLD_OPEN not a marker")
        );
    }
}
