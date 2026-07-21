//! Running a Rebis program deterministically, with no model and no subprocess.
//!
//! `rebis_lang::run` evaluates the concept calculus rather than firing prompts:
//! a prompt is a term, the arrows and squares combine terms, and the record
//! supplies evidence to the operations that consult it. Nothing is sent to a
//! model, so this is offline, instant and repeatable — which makes it the run
//! any front-end can offer without a provider, a network, or a child process.
//!
//! A model-backed run is a different thing and belongs with the agent stack;
//! this is the one that always works.

use std::fmt;

/// What a run produced.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Outcome {
    /// The surviving terms, in the language's own order.
    pub terms: Vec<String>,
    /// How much survived, in `0..=1`.
    pub score: f32,
    /// Which record lines the answer rests on.
    pub evidence: Vec<usize>,
}

impl Outcome {
    /// Whether anything survived. An empty concept is a real result — the
    /// program ran and nothing held — not a failure. Note that a bare prompt
    /// survives as itself, so this is only empty for programs that genuinely
    /// eliminate every term.
    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.terms.is_empty() {
            return write!(f, "nothing survived  (score {:.2})", self.score);
        }
        write!(
            f,
            "{}  (score {:.2}, evidence {:?})",
            self.terms.join(", "),
            self.score,
            self.evidence
        )
    }
}

/// Evaluate `source` against `record`, one line per record entry.
///
/// The only failure is source that does not parse; an empty answer is an
/// outcome, not an error.
pub fn evaluate(source: &str, record: &[String]) -> Result<Outcome, String> {
    let record = rebis_lang::Record::from_texts(record);
    let concept = rebis_lang::run(source, &record).map_err(|e| e.to_string())?;
    Ok(Outcome {
        terms: concept.terms.into_iter().collect(),
        score: concept.score,
        evidence: concept.evidence.into_iter().collect(),
    })
}

/// Split a pasted record into lines, dropping blank ones — what a text box
/// full of evidence means.
pub fn record_from_text(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_prompt_survives_as_itself() {
        // The calculus does not filter a prompt against the record — a prompt
        // is a term, and on its own it holds with a full score.
        let record = record_from_text("the parser has three stages");
        let out = evaluate("\"parser stages\"", &record).unwrap();
        assert_eq!(out.terms, vec!["parser stages".to_string()]);
        assert_eq!(out.score, 1.0);
    }

    #[test]
    fn an_empty_outcome_displays_as_a_result_not_a_failure() {
        let out = Outcome::default();
        assert!(out.is_empty());
        assert!(out.to_string().contains("nothing survived"));
    }

    #[test]
    fn unparseable_source_is_the_only_failure() {
        assert!(evaluate("(-> \"unclosed", &[]).is_err());
    }

    #[test]
    fn running_without_a_record_is_allowed() {
        // No evidence at all is a legitimate run.
        let out = evaluate("\"anything\"", &[]).unwrap();
        assert_eq!(out.terms, vec!["anything".to_string()]);
        assert!(out.evidence.is_empty());
    }

    #[test]
    fn record_text_drops_blank_lines() {
        let r = record_from_text("  one  \n\n   \n two\n");
        assert_eq!(r, vec!["one".to_string(), "two".to_string()]);
    }

    #[test]
    fn the_same_program_and_record_always_give_the_same_answer() {
        // Determinism is the point: no model is fired, so a run is repeatable.
        let record = record_from_text("alpha beta\ngamma delta");
        let a = evaluate("([\"combine\"] \"alpha\" \"gamma\")", &record).unwrap();
        let b = evaluate("([\"combine\"] \"alpha\" \"gamma\")", &record).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn every_documented_form_can_be_run() {
        let record = record_from_text("a b c\nd e f");
        for src in [
            "\"a\"",
            "(-> \"a\" \"b\")",
            "(<- \"a\" \"b\")",
            "([\"m\"] \"a\" \"b\")",
            "($ \"a\" \"b\")",
            "((~ f (x) x) (f \"a\"))",
        ] {
            assert!(evaluate(src, &record).is_ok(), "{src} failed to run");
        }
    }
}
