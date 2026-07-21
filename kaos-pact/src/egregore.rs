//! The Egregore — the shared mind of the Pact.
//!
//! In magical practice an *egregore* is a thought-form sustained by a group: a
//! collective intelligence that outlives any single working. kaos uses it as
//! the society's **shared memory** — distilled lessons, keyed by ray, accreted
//! across every rite the Pact has performed.
//!
//! Engineering reading: this is **retrieval that raises the magical link L**. When
//! a sigil is cast on a ray the Pact has worked before, the egregore lends its
//! accumulated potency to the link — the new working is better grounded because the
//! society remembers. This is what makes kaos *improve over a session* rather
//! than treating every task as the first: the edge compounds.

use crate::equation::clamp01;
use crate::ray::Ray;

/// The collective memory: a potency per ray that grows with successful workings.
#[derive(Clone, Debug, Default)]
pub struct Egregore {
    /// Accumulated potency per worker ray, in [0, ∞) before saturation.
    potency: [f64; 8],
    /// A short ledger of the most recent distilled lessons (for the TUI/diary).
    pub ledger: Vec<String>,
}

impl Egregore {
    pub fn new() -> Egregore {
        Egregore {
            potency: [0.0; 8],
            ledger: Vec::new(),
        }
    }

    fn idx(ray: Ray) -> usize {
        ray.index()
    }

    /// The link bonus the egregore lends to a fresh working on `ray`. Saturating:
    /// the first few successes help a lot, then diminishing returns. Caps at +0.30.
    pub fn link_bonus(&self, ray: Ray) -> f64 {
        let p = self.potency[Self::idx(ray)];
        // 1 − e^{-p/2}, scaled. Smooth, saturating, monotonic in p.
        0.30 * (1.0 - (-p / 2.0).exp())
    }

    /// Feed a verdict back into the collective mind. A true charge strengthens the
    /// egregore for its ray and distils a lesson; a failure leaves only a faint
    /// cautionary trace.
    pub fn reinforce(&mut self, ray: Ray, charged_true: bool, lesson: &str) {
        let i = Self::idx(ray);
        if charged_true {
            self.potency[i] += 1.0;
            self.remember(format!("[{}] {}", ray.name(), lesson));
        } else {
            self.potency[i] += 0.10; // even failure teaches a little
        }
    }

    fn remember(&mut self, line: String) {
        self.ledger.push(line);
        // Keep the ledger from growing without bound — banishing applies here too.
        if self.ledger.len() > 32 {
            let overflow = self.ledger.len() - 32;
            self.ledger.drain(0..overflow);
        }
    }

    /// Total potency across all rays — a single number for "how awake is the
    /// Pact's collective mind."
    pub fn awakeness(&self) -> f64 {
        clamp01(self.potency.iter().sum::<f64>() / 24.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_bonus_grows_and_saturates() {
        let mut e = Egregore::new();
        let before = e.link_bonus(Ray::Red);
        for _ in 0..3 {
            e.reinforce(Ray::Red, true, "warmed the link");
        }
        let after = e.link_bonus(Ray::Red);
        assert!(after > before);
        for _ in 0..100 {
            e.reinforce(Ray::Red, true, "x");
        }
        assert!(e.link_bonus(Ray::Red) <= 0.30 + 1e-9);
    }

    #[test]
    fn rays_are_independent() {
        let mut e = Egregore::new();
        e.reinforce(Ray::Orange, true, "x");
        assert!(e.link_bonus(Ray::Orange) > 0.0);
        assert_eq!(e.link_bonus(Ray::Black), 0.0);
    }
}
