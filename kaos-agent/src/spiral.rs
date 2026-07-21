//! The Spiral — Fibonacci restart scheduling for the agent loop.
//!
//! The benchmark data showed agent solve times are HEAVY-TAILED: the same
//! model on same-difficulty bugs solved in 8s or wandered for 600s, and the
//! wandering runs rarely recovered — they built on their own rot. Restart
//! theory (Luby et al. 1993; every modern SAT solver) answers exactly this:
//! against a heavy-tailed solve-time distribution, MANY SHORT ATTEMPTS WITH
//! FRESH STATE beat one long run, and the optimal budget schedule grows
//! geometrically with ratio between 1.5 and 2.
//!
//! φ = 1.618… sits in that window, and the Fibonacci numbers are its integer
//! schedule. So the spiral: attempt budgets of 5, 8, 13, 21 steps — each a
//! Fibonacci number, each φ longer than the last. A failed attempt is
//! BANISHED (Carroll: reset after a failed working) — its context is
//! discarded whole; only a distilled verdict crosses the gap.
//!
//! **Polarity** makes the restarts genuinely different draws, not the same
//! mistake retried: attempts alternate between two universes of sampling —
//! SOLAR (cold, precise, temperature 0.35) and LUNAR (hot, exploratory,
//! 0.85), each the other's reverse twin around the 0.7 default (the
//! reflection of x about 0.6: solar 0.35 ↔ lunar 0.85). Restart theory needs
//! independent draws for the tail argument to hold; polarity is what makes
//! them independent when a hosted endpoint honours temperature but not seed.

/// The Fibonacci step budgets of the spiral: 8, 13, 21, 34, …
/// `total` caps the SUM — the spiral never spends more steps than one long
/// run would have (the schedule redistributes the budget, it does not grow it).
///
/// The first rung is fib(5) = 8: the smoke data showed the minimal honest
/// attempt — read, read, edit, test, fix, test, finish — needs 6-7 steps, so
/// 5 lands fixes it cannot verify. Eight is the first rung that can close.
pub fn budgets(total: usize) -> Vec<usize> {
    let mut out: Vec<usize> = Vec::new();
    let (mut a, mut b) = (8usize, 13usize);
    let mut spent = 0usize;
    while spent < total {
        let remaining = total - spent;
        // A remainder short of the full next rung is no attempt at all —
        // fold it into the previous rung (the deep dive gets the leftovers).
        if remaining < a {
            match out.last_mut() {
                Some(last) => *last += remaining,
                None => out.push(remaining.max(1)),
            }
            break;
        }
        out.push(a);
        spent += a;
        let next = a + b;
        a = b;
        b = next;
    }
    out
}

/// The two universes of sampling. Each attempt belongs to one; consecutive
/// attempts alternate, so a banished working is never retried under the same
/// stars.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Polarity {
    /// Cold, precise, convergent — the first attempt's universe.
    Solar,
    /// Hot, exploratory, divergent — the reverse twin.
    Lunar,
}

impl Polarity {
    pub fn of_attempt(i: usize) -> Polarity {
        if i.is_multiple_of(2) {
            Polarity::Solar
        } else {
            Polarity::Lunar
        }
    }

    /// The universe's sampling temperature. Reverse twins around the 0.6
    /// midpoint: 0.6 − 0.25 and 0.6 + 0.25.
    pub fn temperature(&self) -> f32 {
        match self {
            Polarity::Solar => 0.35,
            Polarity::Lunar => 0.85,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Polarity::Solar => "solar",
            Polarity::Lunar => "lunar",
        }
    }
}

/// Did this session FIZZLE — end without either finishing or changing the
/// world? A fizzled working is banished and the spiral turns; a session that
/// edited files earns judgement by the gate (or the user) instead.
pub fn fizzled(session: &crate::conductor::Session) -> bool {
    if session.error.is_some() {
        return true;
    }
    let changed = session.steps.iter().any(|s| {
        matches!(
            s.tool,
            crate::conductor::Tool::WriteFile { .. } | crate::conductor::Tool::EditFile { .. }
        ) && (s.observation.starts_with("wrote ")
            || s.observation.starts_with("created ")
            || s.observation.starts_with("edited "))
    });
    // Finished without touching anything: on a code task that is a fizzle too —
    // "the Work is done" with no work done.
    !changed
}

/// The Gnosis Crossing — what survives a banishment.
///
/// Banishment discards the fizzled CONTEXT, not the fizzled WORK: edits stand
/// in the tree, and the map the last self drew of the territory was real even
/// if its fix was not. Blind restarts pay a re-exploration tax for throwing
/// that map away (measured: two v1 instances got SLOWER under the spiral).
/// So both polarities cross the gap, fib-bounded:
///
/// - **positive sigils**: files already modified (the edits stand), files
///   examined — the map;
/// - **the negative sigil**: the tail of the last failing observation — the
///   verdict.
pub fn gnosis(session: &crate::conductor::Session) -> String {
    use crate::conductor::Tool;
    let mut modified: Vec<&str> = Vec::new();
    let mut examined: Vec<&str> = Vec::new();
    let mut last_negative: Option<&str> = None;
    for s in &session.steps {
        match &s.tool {
            Tool::WriteFile { path, .. } | Tool::EditFile { path, .. } => {
                if (s.observation.starts_with("wrote ")
                    || s.observation.starts_with("created ")
                    || s.observation.starts_with("edited "))
                    && !modified.contains(&path.as_str())
                {
                    modified.push(path);
                }
            }
            Tool::ReadFile { path } => {
                if !s.observation.starts_with("error")
                    && !s.observation.starts_with("(unchanged")
                    && !examined.contains(&path.as_str())
                {
                    examined.push(path);
                }
            }
            _ => {}
        }
        if kaos_pact::charge::is_negative(&s.observation) {
            last_negative = Some(&s.observation);
        }
    }
    // Fib bounds: 5 modified, 8 examined, one UNIT of verdict.
    modified.truncate(5);
    examined.retain(|p| !modified.contains(p));
    examined.truncate(8);
    let mut out = String::new();
    if !modified.is_empty() {
        out.push_str(&format!(
            "Files you already modified (those edits STAND in the tree): {}.\n",
            modified.join(", ")
        ));
    }
    if !examined.is_empty() {
        out.push_str(&format!(
            "Files already examined (the map is drawn — do not re-read them all): {}.\n",
            examined.join(", ")
        ));
    }
    match (last_negative, &session.error) {
        (_, Some(e)) => out.push_str(&format!(
            "It ended in error: {}\n",
            kaos_pact::charge::cut(e, 700, true)
        )),
        (Some(neg), None) => out.push_str(&format!(
            "The last failure seen:\n{}\n",
            kaos_pact::charge::cut(neg, 700, true)
        )),
        (None, None) => out.push_str("It ended without changing any file.\n"),
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conductor::{Session, Step, Tool};

    #[test]
    fn the_spiral_is_fibonacci_and_respects_the_total() {
        assert_eq!(budgets(42), vec![8, 13, 21]);
        assert_eq!(budgets(47), vec![8, 13, 26]); // remainder folds into the deep dive
        assert_eq!(budgets(21), vec![8, 13]);
        assert_eq!(budgets(8), vec![8]);
        assert_eq!(budgets(5), vec![5]); // below the first rung: one attempt, all of it
        for total in [5usize, 8, 21, 40, 47, 100] {
            assert_eq!(budgets(total).iter().sum::<usize>(), total);
        }
    }

    #[test]
    fn polarity_alternates_and_twins_mirror() {
        assert_eq!(Polarity::of_attempt(0), Polarity::Solar);
        assert_eq!(Polarity::of_attempt(1), Polarity::Lunar);
        assert_eq!(Polarity::of_attempt(2), Polarity::Solar);
        // Reverse twins around 0.6: equidistant, opposite sides.
        let (s, l) = (Polarity::Solar.temperature(), Polarity::Lunar.temperature());
        assert!((0.6 - s - (l - 0.6)).abs() < 1e-6);
        assert!(s < 0.6 && l > 0.6);
    }

    fn session(steps: Vec<Step>, finished: bool, error: Option<String>) -> Session {
        Session {
            steps,
            finished,
            final_message: String::new(),
            error,
        }
    }

    #[test]
    fn the_gnosis_carries_both_polarities() {
        let steps = vec![
            Step {
                tool: Tool::ReadFile {
                    path: "ledger/models.py".into(),
                },
                observation: "1\tTYPES = (...)".into(),
                thought: String::new(),
            },
            Step {
                tool: Tool::ReadFile {
                    path: "ledger/periods.py".into(),
                },
                observation: "1\timport ...".into(),
                thought: String::new(),
            },
            Step {
                tool: Tool::EditFile {
                    path: "ledger/periods.py".into(),
                    find: "a".into(),
                    replace: "b".into(),
                },
                observation: "edited ledger/periods.py: replaced 1 occurrence".into(),
                thought: String::new(),
            },
            Step {
                tool: Tool::Bash {
                    cmd: "python3 tests.py".into(),
                },
                observation: "exit 1 · AssertionError: -210 != 210".into(),
                thought: String::new(),
            },
        ];
        let g = gnosis(&Session {
            steps,
            finished: false,
            final_message: String::new(),
            error: None,
        });
        assert!(g.contains("ledger/periods.py") && g.contains("STAND"));
        assert!(g.contains("ledger/models.py") && g.contains("examined"));
        assert!(g.contains("AssertionError"));
        // A modified file is not repeated in the examined list.
        assert_eq!(g.matches("ledger/periods.py").count(), 1);
    }

    #[test]
    fn fizzle_reads_the_session_honestly() {
        let edit_ok = Step {
            tool: Tool::EditFile {
                path: "a.py".into(),
                find: "x".into(),
                replace: "y".into(),
            },
            observation: "edited a.py: replaced 1 occurrence".into(),
            thought: String::new(),
        };
        let edit_refused = Step {
            tool: Tool::EditFile {
                path: "a.py".into(),
                find: "n".into(),
                replace: "y".into(),
            },
            observation: "error: `find` text not found in a.py".into(),
            thought: String::new(),
        };
        // A finished session that edited something: not a fizzle.
        assert!(!fizzled(&session(vec![edit_ok.clone()], true, None)));
        // Step budget exhausted with only refused edits: fizzle.
        assert!(fizzled(&session(vec![edit_refused], false, None)));
        // Finished but changed nothing: a fizzle with a bow on it.
        assert!(fizzled(&session(vec![], true, None)));
        // A transport error is always a fizzle.
        assert!(fizzled(&session(vec![edit_ok], true, Some("boom".into()))));
    }
}
