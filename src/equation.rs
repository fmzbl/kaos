//! Carroll's First Equation of Magic — the objective function of kaos.
//!
//! From *Liber Kaos* (Peter J. Carroll), the magic factor of any operation is:
//!
//! ```text
//!     M = G · L · (1 − A) · (1 − R)
//! ```
//!
//! where each factor lies in [0, 1]:
//!   - **G** — *gnosis*: the intensity/quality of the altered state the work is
//!     charged under. Engineering reading: how well-matched the executing agent's
//!     competence and sampling regime are to the task. Carroll wants G ≈ 0.8–0.9.
//!   - **L** — *magical link*: the quality of the connection to the target.
//!     Engineering reading: grounding — how well the prompt/spec/context actually
//!     links to the real problem (retrieval, examples, a sharp statement of intent).
//!   - **A** — *conscious awareness*: deliberation, noise, prompt bloat. It works
//!     *against* the result. Carroll: *"Spell or ensigilization techniques should
//!     be used to depress conscious awareness A to the 0.1 to 0.2 range."* This is
//!     exactly the job of a sigil — see [`crate::sigil`].
//!   - **R** — *subconscious resistance*: accumulated contradiction, hedging,
//!     context rot. Also works against. Banishing (a context reset) depresses R.
//!
//! Carroll's own remark — *"the overall magic factor M can never exceed the value
//! of the gnosis employed or the quality of the magical link"* — falls straight out
//! of the product form, and his worked example (all factors at 0.5 ⇒ M = 0.0625) is
//! reproduced exactly by [`Equation::magic_factor`]. This is not decoration: it is a
//! literal, testable model in which **sigils lower A and banishing lowers R, so both
//! raise M.** That is the edge, stated by the source.
//!
//! ## The second and third equations — where magic *pays*
//!
//! *Liber Kaos* does not stop at M. The **second equation of magic** combines M with
//! P, the probability of the desired result occurring by chance, to give Pm, the
//! probability of bringing it about by magic:
//!
//! ```text
//!     Pm = P + (1 − P) · M^(1/P)                 — Liber Kaos, Principia Magica
//! ```
//!
//! and the **third** describes conjurations launched *against* an event:
//!
//! ```text
//!     Pm = P − P · M^(1/(1−P))
//! ```
//!
//! Carroll's own reading of the second equation is the load-bearing one:
//!
//! > *"Moderate acts of magic in the M = 0.5 to 0.7 range will have a
//! > proportionally greater effect on events whose probability lies in similar
//! > range, while such acts only marginally improve the probabilities of events
//! > which are fairly improbable, i.e. P = 0.2 or below, or fairly probable,
//! > P = 0.8 or above."*
//!
//! That is the **mid-band law**, and it is *exactly* what kaos measured empirically
//! in `agentbench` before anyone re-read the chapter: the conclave (verified
//! best-of-k) lifted a weak model (base pass rate ≈ 0.5) by +32.5 points and a
//! strong one (≈ 1.0) by nothing. The lift of best-of-k over one shot,
//! `1−(1−p)^k − p`, has the same shape as Carroll's `(1−P)·M^(1/P)`: zero at both
//! ends, maximal in the middle. The second equation is a **compute-allocation
//! policy**: spend the conclave where P is mid-band; a single charge suffices at
//! the ceiling, and no amount of sampling rescues P ≈ 0. See [`crate::scry`] for
//! the mechanism that acts on this.

/// The four factors of a single magical act, each clamped to [0, 1].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Equation {
    /// G — gnosis (execution-state quality / competence match).
    pub gnosis: f64,
    /// L — magical link (grounding / spec quality).
    pub link: f64,
    /// A — conscious awareness (bloat / deliberation). Lower is better.
    pub awareness: f64,
    /// R — subconscious resistance (context rot / hedging). Lower is better.
    pub resistance: f64,
}

impl Equation {
    pub fn new(gnosis: f64, link: f64, awareness: f64, resistance: f64) -> Self {
        Equation {
            gnosis: clamp01(gnosis),
            link: clamp01(link),
            awareness: clamp01(awareness),
            resistance: clamp01(resistance),
        }
    }

    /// M = G · L · (1 − A) · (1 − R). The probability that this charge succeeds.
    pub fn magic_factor(&self) -> f64 {
        self.gnosis * self.link * (1.0 - self.awareness) * (1.0 - self.resistance)
    }

    /// The ceiling Carroll names: M can never exceed min(G, L).
    pub fn ceiling(&self) -> f64 {
        self.gnosis.min(self.link)
    }

    /// The second equation applied to this act: the probability the desired result
    /// is brought about, given it would occur by chance with probability `p`.
    pub fn probability_shift(&self, p: f64) -> f64 {
        probability_shift(p, self.magic_factor())
    }
}

/// **The second equation of magic**: `Pm = P + (1 − P) · M^(1/P)`.
///
/// Both arguments are clamped to [0, 1]. The limit cases follow Carroll's table:
/// P = 0 stays 0 for any M < 1 and jumps to 1 only at M = 1 (*"any M = 1 act of
/// magic will raise any probability to a certainty"*); P = 1 stays 1.
pub fn probability_shift(p: f64, m: f64) -> f64 {
    let p = clamp01(p);
    let m = clamp01(m);
    if p <= 0.0 {
        return if m >= 1.0 { 1.0 } else { 0.0 };
    }
    p + (1.0 - p) * m.powf(1.0 / p)
}

/// **The third equation of magic**: `Pm = P − P · M^(1/(1−P))` — a conjuration
/// launched to *prevent* an event. At P = 1 the event is suppressed only by a
/// perfect act (M = 1 → 0); any M < 1 leaves certainty untouched.
pub fn probability_suppression(p: f64, m: f64) -> f64 {
    let p = clamp01(p);
    let m = clamp01(m);
    if p >= 1.0 {
        return if m >= 1.0 { 0.0 } else { 1.0 };
    }
    p - p * m.powf(1.0 / (1.0 - p))
}

/// The *lift* the second equation grants over chance: `Pm − P = (1−P)·M^(1/P)`.
/// This is the quantity Carroll's mid-band law is about — zero at both ends of P,
/// maximal in the middle band for moderate M.
pub fn lift(p: f64, m: f64) -> f64 {
    probability_shift(p, m) - clamp01(p)
}

/// The conclave's version of the same curve: the lift of best-of-k over one shot,
/// `(1 − (1−p)^k) − p`. It obeys the same mid-band law as [`lift`] — which is why
/// `agentbench` measured +32.5 points at p ≈ 0.5 and +0.0 at p ≈ 1.0.
pub fn conclave_lift(p: f64, k: usize) -> f64 {
    let p = clamp01(p);
    (1.0 - (1.0 - p).powi(k as i32)) - p
}

#[inline]
pub fn clamp01(x: f64) -> f64 {
    x.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carrolls_worked_example() {
        // Liber Kaos: "If all factors are at half, 0.5, then the overall magic
        // factor M is a very poor 0.0625."
        let eq = Equation::new(0.5, 0.5, 0.5, 0.5);
        assert!((eq.magic_factor() - 0.0625).abs() < 1e-9);
    }

    #[test]
    fn sigil_lowering_awareness_raises_m() {
        // The central claim, mechanically: depress A and M rises, all else equal.
        let bloated = Equation::new(0.85, 0.85, 0.80, 0.20);
        let sigilized = Equation::new(0.85, 0.85, 0.15, 0.20); // A: 0.80 -> 0.15
        assert!(sigilized.magic_factor() > bloated.magic_factor());
    }

    #[test]
    fn banishing_lowering_resistance_raises_m() {
        let rotten = Equation::new(0.85, 0.85, 0.15, 0.70);
        let banished = Equation::new(0.85, 0.85, 0.15, 0.15); // R reset by banishing
        assert!(banished.magic_factor() > rotten.magic_factor());
    }

    #[test]
    fn m_never_exceeds_the_ceiling() {
        let eq = Equation::new(0.6, 0.4, 0.0, 0.0);
        assert!(eq.magic_factor() <= eq.ceiling() + 1e-12);
    }

    #[test]
    fn carrolls_table_two_p_half_row() {
        // Liber Kaos, Table 2 ("The effects of magic on probability"), the P = 0.5
        // row is printed legibly: M=0.5 → 0.625, M=0.7 → 0.745, M=0.8 → 0.820,
        // M=0.9 → 0.905, M=1.0 → 1.000. The equation reproduces it exactly.
        for (m, want) in [
            (0.5, 0.625),
            (0.7, 0.745),
            (0.8, 0.820),
            (0.9, 0.905),
            (1.0, 1.0),
        ] {
            assert!(
                (probability_shift(0.5, m) - want).abs() < 5e-4,
                "P=0.5 M={m}: got {}, table says {want}",
                probability_shift(0.5, m)
            );
        }
    }

    #[test]
    fn second_equation_limit_cases() {
        // "The bottom line of the graph remains at zero until M = 1, when the Pm
        // value moves suddenly from zero to one."
        assert_eq!(probability_shift(0.0, 0.99), 0.0);
        assert_eq!(probability_shift(0.0, 1.0), 1.0);
        assert_eq!(probability_shift(1.0, 0.0), 1.0);
        // Any act of magic, if not totally hopeless, improves a non-zero probability.
        assert!(probability_shift(0.3, 0.5) > 0.3);
    }

    #[test]
    fn third_equation_suppresses() {
        assert!(probability_suppression(0.5, 0.7) < 0.5);
        assert_eq!(probability_suppression(1.0, 0.5), 1.0); // certainty resists M < 1
        assert_eq!(probability_suppression(1.0, 1.0), 0.0); // ...but not a miracle
        assert_eq!(probability_suppression(0.0, 0.9), 0.0);
    }

    #[test]
    fn carrolls_midband_law() {
        // "Moderate acts of magic ... have a proportionally greater effect on events
        // whose probability lies in similar range, while such acts only marginally
        // improve" the extremes. Lift at mid-P beats both tails for moderate M.
        for m in [0.5, 0.6, 0.7] {
            assert!(lift(0.5, m) > lift(0.1, m), "mid vs low tail at M={m}");
            assert!(lift(0.5, m) > lift(0.9, m), "mid vs high tail at M={m}");
        }
    }

    #[test]
    fn the_conclaves_lift_obeys_the_same_midband_law() {
        // The bridge from Carroll to agentbench: best-of-k's lift over one shot has
        // the same shape — nothing to rescue at the ceiling, nothing to amplify at
        // the floor, the payoff in the middle. This is why the conclave moved a
        // weak model +32.5 points and a strong one +0.0.
        for k in [3usize, 5] {
            assert!(conclave_lift(0.5, k) > conclave_lift(0.05, k));
            assert!(conclave_lift(0.5, k) > conclave_lift(0.99, k));
        }
        assert!(conclave_lift(1.0, 5).abs() < 1e-12); // the strong-model null result
        assert!(conclave_lift(0.0, 5).abs() < 1e-12); // magic cannot rescue P = 0
    }
}
