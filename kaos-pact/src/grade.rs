//! The grades of the Pact.
//!
//! The Pact is constituted in four grades: Neophyte, Initiate, Adept and Magus,
//! numbered respectively 4°, 3°, 2°, 1°. Above them sit the offices of
//! **Magister Templi** and **Supreme Magus (0°)**. The grade structure merely
//! recognizes technical magical competence and organizational responsibility.
//!
//! kaos takes that literally: a grade is an **online competence estimate**.
//! An adept that charges sigils successfully is **elevated**; one that fails the
//! Weighing is **humbled**. Grade feeds back into routing (a higher grade is a
//! stronger candidate and contributes more gnosis), so the society becomes a
//! self-tuning router — a bandit in ceremonial dress.

/// A grade in the Pact. Ordered low → high.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Grade {
    /// 4° — the newly sworn. Untrusted, cheap to spend.
    Neophyte,
    /// 3° — has charged real work.
    Initiate,
    /// 2° — a proven hand in their ray.
    Adept,
    /// 1° — a master of the ray; near-always routed first in their sphere.
    Magus,
    /// 0° — the office, not a worker. The orchestrator-self. One per Pact.
    SupremeMagus,
}

impl Grade {
    /// The degree notation Carroll uses (4° down to 0°).
    pub fn degree(&self) -> &'static str {
        match self {
            Grade::Neophyte => "4\u{b0}",
            Grade::Initiate => "3\u{b0}",
            Grade::Adept => "2\u{b0}",
            Grade::Magus => "1\u{b0}",
            Grade::SupremeMagus => "0\u{b0}",
        }
    }

    pub fn title(&self) -> &'static str {
        match self {
            Grade::Neophyte => "Neophyte",
            Grade::Initiate => "Initiate",
            Grade::Adept => "Adept",
            Grade::Magus => "Magus",
            Grade::SupremeMagus => "Supreme Magus",
        }
    }

    /// The gnosis multiplier a grade contributes to a charge. A Magus charges from
    /// a deeper, surer altered state than a Neophyte. Caps below 1.0 so grade is a
    /// bonus, never the whole of G.
    pub fn gnosis_weight(&self) -> f64 {
        match self {
            Grade::Neophyte => 0.80,
            Grade::Initiate => 0.88,
            Grade::Adept => 0.94,
            Grade::Magus => 0.99,
            Grade::SupremeMagus => 1.0,
        }
    }

    /// Elevation: the next grade up (worker grades only; the Supreme Magus office
    /// is appointed, not earned by throughput).
    pub fn elevated(&self) -> Grade {
        match self {
            Grade::Neophyte => Grade::Initiate,
            Grade::Initiate => Grade::Adept,
            Grade::Adept => Grade::Magus,
            Grade::Magus => Grade::Magus,
            Grade::SupremeMagus => Grade::SupremeMagus,
        }
    }

    /// Humbling: one grade down. A Neophyte cannot fall further — they are already
    /// at the threshold of the Pact.
    pub fn humbled(&self) -> Grade {
        match self {
            Grade::Neophyte => Grade::Neophyte,
            Grade::Initiate => Grade::Neophyte,
            Grade::Adept => Grade::Initiate,
            Grade::Magus => Grade::Adept,
            Grade::SupremeMagus => Grade::SupremeMagus,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordering_matches_degrees() {
        assert!(Grade::Neophyte < Grade::Initiate);
        assert!(Grade::Initiate < Grade::Adept);
        assert!(Grade::Adept < Grade::Magus);
    }

    #[test]
    fn elevation_and_humbling_are_inverse_in_the_middle() {
        assert_eq!(Grade::Adept.elevated().humbled(), Grade::Adept);
        assert_eq!(Grade::Adept.humbled().elevated(), Grade::Adept);
    }

    #[test]
    fn higher_grade_charges_deeper() {
        assert!(Grade::Magus.gnosis_weight() > Grade::Neophyte.gnosis_weight());
    }

    #[test]
    fn extremes_are_stable() {
        assert_eq!(Grade::Neophyte.humbled(), Grade::Neophyte);
        assert_eq!(Grade::Magus.elevated(), Grade::Magus);
    }
}
