//! An Adept — a sworn member of the Pact, and the unit of work.
//!
//! Each adept takes a **magical name** (chaos magicians take a motto — e.g. Carroll
//! wrote as *Frater Stokastikos*), is sworn to a **home ray** (their sphere of
//! mastery), and holds a **grade** that floats with their record. An adept is, in
//! engineering terms, a specialist worker with an online competence estimate.

use crate::grade::Grade;
use crate::ray::{competence, Ray};
use crate::rng::{hash_str, Rng};

/// A member of the Pact.
#[derive(Clone, Debug)]
pub struct Adept {
    /// The magical motto (e.g. "Frater Stokastikos").
    pub name: String,
    /// The ray this adept is sworn to — their sphere of greatest gnosis.
    pub home: Ray,
    /// Current grade in the Pact; rises and falls with the Weighing.
    pub grade: Grade,
    /// A fixed temperament in [0, 1], derived from the name — innate skill that
    /// the grade modulates. Two adepts of the same ray and grade still differ.
    pub temperament: f64,
    /// Lifetime sigils charged true.
    pub victories: u32,
    /// Lifetime sigils that failed the Weighing.
    pub defeats: u32,
    /// Consecutive victories since the last defeat — the elevation run. Reaching
    /// three elevates and the run begins anew; any defeat breaks it.
    pub streak: u32,
}

impl Adept {
    pub fn sworn(name: &str, home: Ray, grade: Grade) -> Adept {
        // Temperament in [0.82, 1.00], stable per name.
        let t = 0.82 + (hash_str(name) % 1000) as f64 / 1000.0 * 0.18;
        Adept {
            name: name.to_string(),
            home,
            grade,
            temperament: t,
            victories: 0,
            defeats: 0,
            streak: 0,
        }
    }

    /// The **gnosis** this adept can muster against a task on `task_ray`. This is
    /// the G factor of [`crate::equation`]: competence-for-the-ray × grade depth ×
    /// temperament, with a little reproducible variance for the moment of charging.
    pub fn gnosis(&self, task_ray: Ray, rng: &mut Rng) -> f64 {
        let comp = competence(self.home, task_ray);
        let base = comp * self.grade.gnosis_weight() * self.temperament;
        // The altered state is never perfectly repeatable: small charge-time jitter.
        (base + rng.jitter(0.04)).clamp(0.0, 1.0)
    }

    /// How strong a candidate this adept is to be *routed* a task of `task_ray`.
    /// Higher grade and higher home-competence win the assignment. Deterministic
    /// (no jitter) so routing is explainable.
    pub fn fitness(&self, task_ray: Ray) -> f64 {
        competence(self.home, task_ray) * self.grade.gnosis_weight() * self.temperament
    }

    /// Record a verdict and let the grade float (Carroll: the grade "merely
    /// recognizes technical magical competence"). Three clean victories since the
    /// last defeat elevate; a defeat humbles immediately and breaks the run. The
    /// asymmetry is deliberate — the Pact is quicker to demote than to promote.
    pub fn record(&mut self, charged_true: bool) {
        if charged_true {
            self.victories += 1;
            self.streak += 1;
            // Elevate on a run of three since the last defeat, capped at Magus. The
            // run resets either way: the next elevation must be earned in full.
            if self.streak >= 3 {
                if self.grade < Grade::Magus {
                    self.grade = self.grade.elevated();
                }
                self.streak = 0;
            }
        } else {
            self.defeats += 1;
            self.grade = self.grade.humbled();
            self.streak = 0;
        }
    }

    /// A one-line roster entry for the TUI.
    pub fn epithet(&self) -> String {
        format!(
            "{} {:>4}  {:<8} {} \u{2014} {}",
            self.grade.degree(),
            self.grade.title(),
            self.home.name(),
            self.name,
            self.home.sphere(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_determines_temperament() {
        let a = Adept::sworn("Frater Stokastikos", Ray::Red, Grade::Adept);
        let b = Adept::sworn("Frater Stokastikos", Ray::Red, Grade::Adept);
        assert_eq!(a.temperament, b.temperament);
    }

    #[test]
    fn defeat_humbles_immediately() {
        let mut a = Adept::sworn("Soror Test", Ray::Red, Grade::Adept);
        a.record(false);
        assert_eq!(a.grade, Grade::Initiate);
    }

    #[test]
    fn three_victories_elevate() {
        let mut a = Adept::sworn("Soror Test", Ray::Red, Grade::Initiate);
        a.record(true);
        a.record(true);
        a.record(true);
        assert_eq!(a.grade, Grade::Adept);
    }

    #[test]
    fn a_defeat_breaks_the_elevation_run() {
        // Five wins, one loss, one win: the loss broke the run, so that single win
        // after it must NOT elevate — lifetime totals are not the run.
        let mut a = Adept::sworn("Soror Test", Ray::Red, Grade::Initiate);
        for _ in 0..5 {
            a.record(true); // three wins → Adept; the run stands at 2 after five
        }
        assert_eq!(a.grade, Grade::Adept);
        a.record(false); // humbled back to Initiate; the run is dead
        assert_eq!(a.grade, Grade::Initiate);
        a.record(true);
        assert_eq!(
            a.grade,
            Grade::Initiate,
            "one win after a defeat must not elevate"
        );
    }

    #[test]
    fn three_straight_wins_after_a_loss_elevate() {
        let mut a = Adept::sworn("Soror Test", Ray::Red, Grade::Adept);
        a.record(true); // a stray prior win, so the run and lifetime totals diverge
        a.record(false); // humbled to Initiate; the run resets
        assert_eq!(a.grade, Grade::Initiate);
        a.record(true);
        a.record(true);
        assert_eq!(
            a.grade,
            Grade::Initiate,
            "two wins are not yet a run of three"
        );
        a.record(true);
        assert_eq!(
            a.grade,
            Grade::Adept,
            "three straight wins since the defeat elevate"
        );
    }

    #[test]
    fn home_ray_gives_more_gnosis() {
        let a = Adept::sworn("Frater X", Ray::Orange, Grade::Adept);
        assert!(a.fitness(Ray::Orange) > a.fitness(Ray::Black));
    }
}
