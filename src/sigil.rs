//! The Sigil Engine — sigilization as prompt engineering.
//!
//! This is the core conceit of kaos made mechanical. Austin Osman Spare's
//! method, as given in *Liber Null*:
//!
//! > *"There are three parts to the operation of a sigil. The sigil is
//! > constructed, the sigil is lost to the mind, the sigil is charged."*
//!
//! And the construction itself (the **word method**):
//!
//! > *"I wish to obtain the Necronomicon → (Eliminate repeated letters) →
//! > Letters Rearranged to give pictorial sigil."*
//!
//! We implement exactly that, and then read it as engineering:
//!
//! 1. **Construct** — take a *statement of intent* (a single, present-tense desire),
//!    strip it to letters, **eliminate repeated letters** (Spare's rule), and bind
//!    the survivors into a glyph. The fraction of the statement that survives is a
//!    direct measure of its redundancy. A verbose, hedged, repetitive intent
//!    compresses a lot; a sharp one barely compresses.
//! 2. **Lose / banish** — the statement of intent is dropped; only the compressed
//!    glyph and a terse *charged intent* remain. This is **context isolation**: the
//!    executor never sees the rambling original.
//! 3. **Charge** — the glyph is fired under gnosis (see [`crate::gnosis`]).
//!
//! The payoff is the **awareness factor A** of Carroll's equation. *Liber Kaos*:
//! *"Spell or ensigilization techniques should be used to depress conscious
//! awareness A to the 0.1 to 0.2 range."* So [`Sigil::awareness`] is computed from
//! the actual compression the algorithm achieved: more redundancy removed ⇒ lower A
//! ⇒ (by the equation) higher M. **The lore drives the number.** A sigil is not a
//! decoration here; it is a measured reduction of prompt bloat.

use crate::equation::clamp01;
use crate::rng::hash_str;

/// A constructed sigil: the compressed residue of a statement of intent.
#[derive(Clone, Debug)]
pub struct Sigil {
    /// The original statement of intent (kept only for the record/diary; never
    /// shown to the executor — it has been "lost to the mind").
    pub statement: String,
    /// The unique, repeated-letters-eliminated residue, in order of first
    /// appearance — Spare's raw material for the glyph.
    pub residue: Vec<char>,
    /// A terse, present-tense charged intent: what actually gets executed.
    pub charged_intent: String,
    /// A reproducible etheric signature of the statement (for seeding the charge).
    pub signature: u64,
}

impl Sigil {
    /// **Construct** a sigil from a statement of intent (Spare's word method).
    pub fn construct(statement: &str) -> Sigil {
        let letters: Vec<char> = statement
            .chars()
            .filter(|c| c.is_ascii_alphabetic())
            .map(|c| c.to_ascii_uppercase())
            .collect();

        // Eliminate repeated letters, preserving first-appearance order.
        let mut seen = [false; 26];
        let mut residue = Vec::new();
        for c in &letters {
            let idx = (*c as u8 - b'A') as usize;
            if !seen[idx] {
                seen[idx] = true;
                residue.push(*c);
            }
        }

        Sigil {
            statement: statement.to_string(),
            residue,
            charged_intent: charge_intent(statement),
            signature: hash_str(statement),
        }
    }

    /// The number of letters in the statement before elimination.
    pub fn raw_letters(&self) -> usize {
        self.statement
            .chars()
            .filter(|c| c.is_ascii_alphabetic())
            .count()
    }

    /// Compression ratio in [0, 1]: 1 − residue/raw. High ⇒ much redundancy
    /// removed ⇒ a tight intent.
    pub fn compression(&self) -> f64 {
        let raw = self.raw_letters();
        if raw == 0 {
            return 0.0;
        }
        1.0 - (self.residue.len() as f64 / raw as f64)
    }

    /// **The awareness factor A** that this sigil achieves, derived from the real
    /// compression. Carroll's target band is 0.1–0.2; an *uncharged*, un-sigilized
    /// intent sits near 0.8 (full conscious deliberation). A well-formed statement
    /// of intent compresses enough to drop A into Carroll's band.
    ///
    /// Mapping: A = 0.78 − 1.0 · compression, clamped to [0.08, 0.85]. An
    /// *un-sigilized* intent (no compression) sits near 0.78 — full conscious
    /// deliberation, the raw-prompt baseline. The English alphabet has 26 letters,
    /// so any real statement of intent compresses substantially, pulling A down
    /// into Carroll's 0.10–0.20 band. The number is the algorithm's, not a fudge.
    pub fn awareness(&self) -> f64 {
        clamp01(0.78 - 1.0 * self.compression()).clamp(0.08, 0.85)
    }

    /// The awareness of an *un-sigilized* working — a verbose, hedged prompt in a
    /// long chat, with conscious deliberation left fully engaged. This is the
    /// baseline A that the raw configuration runs at.
    pub const UNSIGILIZED_AWARENESS: f64 = 0.78;

    /// Render the glyph: the residue letters overstruck into a single bound form.
    /// Purely cosmetic — but a sigil you cannot see is hard to charge. Returns a
    /// compact unicode mark plus the residue, the way a magician would sketch it.
    pub fn glyph(&self) -> String {
        if self.residue.is_empty() {
            return "\u{16B9}".to_string(); // a lone rune for the empty intent
        }
        // Bind the letters with combining marks so they read as one sigil-glyph.
        let mut g = String::new();
        for (i, c) in self.residue.iter().enumerate() {
            g.push(*c);
            if i + 1 < self.residue.len() {
                g.push('\u{0335}'); // combining short stroke overlay — the bind
            }
        }
        g
    }
}

/// Distil a statement of intent into a terse charged imperative. This is what the
/// executor receives after the original has been "lost to the mind" — the prompt,
/// stripped of hedging and preamble. Real, if simple, prompt compression.
fn charge_intent(statement: &str) -> String {
    const NOISE: &[&str] = &[
        "i", "wish", "to", "would", "like", "want", "please", "the", "a", "an", "obtain", "make",
        "kindly", "could", "you", "we", "should", "that", "this", "in", "of", "for", "and", "so",
        "just", "really", "very", "maybe", "perhaps", "it", "is", "be", "my", "our",
    ];
    let kept: Vec<String> = statement
        .split_whitespace()
        .map(|w| {
            w.trim_matches(|c: char| !c.is_ascii_alphanumeric())
                .to_string()
        })
        .filter(|w| !w.is_empty())
        .filter(|w| !NOISE.contains(&w.to_lowercase().as_str()))
        .collect();
    if kept.is_empty() {
        return statement.trim().to_string();
    }
    kept.join(" ").to_uppercase()
}

/// Forge a *statement of intent* from a raw task in the imperative present tense —
/// the magician's "This work…". Forcing the task into this shape is itself the
/// discipline: it demands a single, concrete, present desire.
pub fn statement_of_intent(task: &str) -> String {
    let t = task.trim().trim_end_matches('.');
    format!("THIS WORK {}", t.to_uppercase())
}

/// **Banish.** *Liber Null*: after charging, *"it is wise to banish it by evoking
/// laughter"* — and the statement of intent *"must be banished from normal waking
/// consciousness."* Engineering reading: a context reset. Banishing returns the
/// resistance factor R toward its floor — the next charge does not inherit the rot
/// of the last. Returns the post-banish R given the pre-banish R and a floor.
pub fn banish(resistance_before: f64, floor: f64) -> f64 {
    // Laughter does not erase everything — a trace remains — but it collapses most
    // of the accumulated resistance back toward the floor.
    clamp01(floor + 0.15 * (resistance_before - floor).max(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spares_necronomicon_example() {
        // Spare's own worked example. "I wish to obtain the Necronomicon" →
        // eliminate repeated letters, first-appearance order.
        let s = Sigil::construct("I wish to obtain the Necronomicon");
        let residue: String = s.residue.iter().collect();
        // I W S H T O B A N E C R M — each letter once, in order seen.
        assert_eq!(residue, "IWSHTOBANECRM");
        // No letter appears twice.
        let mut sorted = s.residue.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), s.residue.len());
    }

    #[test]
    fn sigil_depresses_awareness_into_carrolls_band() {
        // A realistic statement of intent should land A in/near 0.1–0.25.
        let s = Sigil::construct(&statement_of_intent(
            "fix the off by one error in the pagination cursor logic",
        ));
        let a = s.awareness();
        assert!(a < 0.30, "expected sigil to depress A below 0.30, got {a}");
        assert!(a >= 0.08);
    }

    #[test]
    fn more_redundant_intent_compresses_more() {
        let terse = Sigil::construct("FIX CRASH");
        let verbose = Sigil::construct(
            "please please could you kindly fix the crash that keeps crashing again",
        );
        assert!(verbose.compression() > terse.compression());
        assert!(verbose.awareness() < terse.awareness());
    }

    #[test]
    fn charged_intent_drops_noise() {
        let c = charge_intent("I wish to obtain the Necronomicon");
        assert!(c.contains("NECRONOMICON"));
        assert!(!c.to_lowercase().contains("wish"));
    }

    #[test]
    fn banishing_collapses_resistance_toward_floor() {
        let after = banish(0.7, 0.1);
        assert!(after < 0.25);
        assert!(after >= 0.1);
    }

    #[test]
    fn empty_intent_is_safe() {
        let s = Sigil::construct("!!! 123 ???");
        assert_eq!(s.compression(), 0.0);
        assert!(!s.glyph().is_empty());
    }
}
