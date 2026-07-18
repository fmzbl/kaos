//! Charging a sigil — assembling Carroll's equation and firing it.
//!
//! This is where the four factors are gathered from the parts of the Pact and the
//! work, and the charge is fired. *Liber Null* lists the states under which a sigil
//! is charged — *"during magical trance; at the moment of orgasm or great elation;
//! at times of great fear, anger…"* — all of them moments where conscious
//! awareness is suppressed. We model the *outcome*, not the trance: given the
//! equation, the magic factor M is the success probability, and we sample it.

use crate::adept::Adept;
use crate::equation::Equation;
use crate::ray::Ray;
use crate::rng::Rng;

/// The two gnosis currents of *Liber Null* — paths to the same suppression of A.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Current {
    /// Inhibitory: stillness, trance, meditation. Low temperature — careful, exact.
    /// Suits surgical rays (Black entropy, Orange intellect, Yellow contracts).
    Inhibitory,
    /// Excitatory: ecstasy, fury, elation. High temperature — bold, exploratory.
    /// Suits forceful rays (Red war, Purple creation, Blue expansion).
    Excitatory,
}

impl Current {
    /// The current that best suits a ray's temperament.
    pub fn for_ray(ray: Ray) -> Current {
        match ray {
            Ray::Black | Ray::Orange | Ray::Yellow | Ray::Octarine => Current::Inhibitory,
            Ray::Red | Ray::Purple | Ray::Blue | Ray::Green => Current::Excitatory,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Current::Inhibitory => "inhibitory (still)",
            Current::Excitatory => "excitatory (ecstatic)",
        }
    }
}

/// The full account of one charge — kept for the diary and the TUI.
#[derive(Clone, Debug)]
pub struct Charge {
    pub eq: Equation,
    pub current: Current,
    /// M = magic factor = the success probability that was sampled.
    pub magic_factor: f64,
    /// Whether the charge fired true (sampled against M).
    pub fired: bool,
}

/// Assemble the equation for an adept charging `sigil` against a task on
/// `task_ray`, with the egregore lending `link_bonus`, under current `resistance`.
///
/// - G = adept gnosis (competence × grade × temperament × charge-time variance),
///   plus a small bonus when the gnosis current matches the ray.
/// - L = base link 0.55 + egregore bonus (retrieval grounding).
/// - A = the sigil's measured awareness (compression-derived).
/// - R = the resistance carried into this charge (rot; reset by banishing).
pub fn assemble(
    adept: &Adept,
    awareness: f64,
    task_ray: Ray,
    link_bonus: f64,
    resistance: f64,
    rng: &mut Rng,
) -> (Equation, Current) {
    let current = Current::for_ray(task_ray);
    let mut g = adept.gnosis(task_ray, rng);
    // A current matched to the ray deepens the gnosis slightly.
    if Current::for_ray(adept.home) == current {
        g = (g + 0.05).min(1.0);
    }
    // The base link of a well-formed statement of intent, plus what the egregore
    // lends. Caps just shy of 1.0 — the magical link is never perfect.
    let l = (0.68 + link_bonus).min(0.96);
    (Equation::new(g, l, awareness, resistance), current)
}

/// **Charge** the sigil: sample success against M = G·L·(1−A)·(1−R). `awareness`
/// is the A the working achieves — low if a sigil was cast, high (≈0.78) if the
/// intent was left raw and verbose.
pub fn charge(
    adept: &Adept,
    awareness: f64,
    task_ray: Ray,
    link_bonus: f64,
    resistance: f64,
    rng: &mut Rng,
) -> Charge {
    let (eq, current) = assemble(adept, awareness, task_ray, link_bonus, resistance, rng);
    let m = eq.magic_factor();
    let fired = rng.chance(m);
    Charge {
        eq,
        current,
        magic_factor: m,
        fired,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grade::Grade;
    use crate::sigil::{statement_of_intent, Sigil};

    #[test]
    fn matched_current_per_ray() {
        assert_eq!(Current::for_ray(Ray::Red), Current::Excitatory);
        assert_eq!(Current::for_ray(Ray::Black), Current::Inhibitory);
    }

    #[test]
    fn a_well_sigilized_charge_by_an_adept_mostly_fires() {
        // Adept on home ray, tight sigil (low A), some egregore, low R → M high.
        let adept = Adept::sworn("Frater Stokastikos", Ray::Red, Grade::Magus);
        let a = Sigil::construct(&statement_of_intent(
            "fix the deadlock in the worker pool under load",
        ))
        .awareness();
        let mut rng = Rng::new(2026);
        let mut fired = 0;
        for _ in 0..1000 {
            let c = charge(&adept, a, Ray::Red, 0.2, 0.12, &mut rng);
            if c.fired {
                fired += 1;
            }
        }
        // Comfortable majority — the equation is working in our favour.
        assert!(
            fired > 500,
            "expected a charged Magus to mostly fire, got {fired}/1000"
        );
    }

    #[test]
    fn bloated_high_resistance_charge_mostly_fizzles() {
        let adept = Adept::sworn("Neo", Ray::Green, Grade::Neophyte);
        // Off-ray task, no egregore, high resistance, raw (un-sigilized) high A.
        let mut rng = Rng::new(7);
        let mut fired = 0;
        for _ in 0..1000 {
            let c = charge(
                &adept,
                Sigil::UNSIGILIZED_AWARENESS,
                Ray::Black,
                0.0,
                0.75,
                &mut rng,
            );
            if c.fired {
                fired += 1;
            }
        }
        assert!(
            fired < 200,
            "expected a poor charge to mostly fizzle, got {fired}/1000"
        );
    }
}
