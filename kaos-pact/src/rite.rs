//! The Great Work — the orchestration loop.
//!
//! A *rite* takes one task and drives it through the full ceremony:
//!
//! 1. **Scry the ray** — classify the task to one of the eight rays.
//! 2. **Forge the statement of intent** and **construct the sigil** — which
//!    depresses the awareness factor A (intent compression + isolation).
//! 3. **Route** the sigil to the fittest sworn adept (raises G — competence match).
//! 4. **Link** through the egregore — the Pact's shared memory (raises L).
//! 5. **Charge** under the matched gnosis current (sample M = G·L·(1−A)·(1−R)).
//! 6. On a failed charge, **banish** (reset context resistance R) and **re-route**
//!    to the next-fittest adept — a paradigm shift, a fresh life. Without
//!    banishing, the resistance of the failed attempt would rot the next one.
//! 7. **Weigh & ship** the first true charge; **record** verdicts so grades float
//!    and the egregore learns.
//!
//! The `Levers` flags exist so the benchmark can switch each mechanism off and
//! measure its individual contribution — proving the edge is real, not assumed.

use crate::gnosis::{charge, Charge};
use crate::order::Pact;
use crate::ray::Ray;
use crate::rng::Rng;
use crate::sigil::{banish, statement_of_intent, Sigil};

/// The floor that a fresh, banished context returns to. Never zero — some
/// resistance is intrinsic.
pub const RESISTANCE_FLOOR: f64 = 0.10;
/// How much a *failed* attempt rots the working context before banishing.
pub const ROT_PER_FAILURE: f64 = 0.28;
/// Maximum adepts the rite will route through before conceding.
pub const MAX_ATTEMPTS: usize = 4;

/// Which mechanisms are enabled. All on = full kaos. Turning one off is how
/// the benchmark isolates its effect.
#[derive(Clone, Copy, Debug)]
pub struct Levers {
    /// Construct a sigil to depress A. Off ⇒ raw verbose intent (A stays high).
    pub sigilize: bool,
    /// Route to the fittest ray specialist. Off ⇒ the first adept toils on
    /// everything (a generalist).
    pub route: bool,
    /// Banish (reset R) between attempts. Off ⇒ resistance rots and accumulates.
    pub banish: bool,
    /// Let the egregore lend its link bonus. Off ⇒ no cross-task memory.
    pub egregore: bool,
}

impl Levers {
    /// The full ceremony — every lever engaged.
    pub fn full() -> Levers {
        Levers {
            sigilize: true,
            route: true,
            banish: true,
            egregore: true,
        }
    }
    /// The raw baseline: one generalist, a verbose intent, a single accumulating
    /// context, no shared memory. The "just prompt a model in a long chat" agent.
    pub fn raw() -> Levers {
        Levers {
            sigilize: false,
            route: false,
            banish: false,
            egregore: false,
        }
    }
}

/// One attempt within a rite — for the diary and the TUI.
#[derive(Clone, Debug)]
pub struct Attempt {
    pub adept_name: String,
    pub adept_idx: usize,
    pub charge: Charge,
    pub resistance_in: f64,
}

/// The full record of a rite.
#[derive(Clone, Debug)]
pub struct Rite {
    pub task: String,
    pub ray: Ray,
    pub statement: String,
    pub sigil: Sigil,
    pub attempts: Vec<Attempt>,
    pub succeeded: bool,
}

impl Rite {
    pub fn final_attempt(&self) -> Option<&Attempt> {
        self.attempts.last()
    }
}

/// Perform the Great Work on a single task and mutate the Pact's state (grades,
/// egregore) accordingly. Deterministic given `rng`.
pub fn perform(pact: &mut Pact, task: &str, levers: Levers, rng: &mut Rng) -> Rite {
    let ray = Ray::classify(task);

    // Construct the sigil. With sigilization on, the awareness factor A is the
    // compression-derived value (low). With it off, there is no sigil discipline:
    // A stays at the raw, fully-deliberated level — a verbose prompt in a long
    // chat. We still build a sigil object for display, but A comes from the lever.
    let sigil = if levers.sigilize {
        Sigil::construct(&statement_of_intent(task))
    } else {
        Sigil::construct(task)
    };
    let awareness = if levers.sigilize {
        sigil.awareness()
    } else {
        Sigil::UNSIGILIZED_AWARENESS
    };
    let statement = if levers.sigilize {
        statement_of_intent(task)
    } else {
        task.to_string()
    };

    let mut resistance = RESISTANCE_FLOOR;
    let mut attempts = Vec::new();
    let mut tried: Vec<usize> = Vec::new();
    let mut succeeded = false;

    for _ in 0..MAX_ATTEMPTS {
        // Route (or fall back to the first member — the generalist).
        let idx = if levers.route {
            next_fittest(pact, ray, &tried)
        } else {
            0
        };
        tried.push(idx);

        let link_bonus = if levers.egregore {
            pact.egregore.link_bonus(ray)
        } else {
            0.0
        };

        let c = charge(
            &pact.members[idx],
            awareness,
            ray,
            link_bonus,
            resistance,
            rng,
        );
        let resistance_in = resistance;
        let adept_name = pact.members[idx].name.clone();

        // Weigh the heart: record verdict into grade + egregore.
        if c.fired {
            pact.members[idx].record(true);
            if levers.egregore {
                pact.egregore.reinforce(ray, true, &short_lesson(task));
            }
            succeeded = true;
        } else {
            pact.members[idx].record(false);
            if levers.egregore {
                pact.egregore.reinforce(ray, false, "");
            }
            // The failed attempt rots the context. Banishing collapses that rot
            // back toward the floor; without it, the rot persists into the next.
            let rotted = (resistance + ROT_PER_FAILURE).min(0.95);
            resistance = if levers.banish {
                banish(rotted, RESISTANCE_FLOOR)
            } else {
                rotted
            };
        }

        attempts.push(Attempt {
            adept_name,
            adept_idx: idx,
            charge: c,
            resistance_in,
        });

        if succeeded {
            break;
        }
        // A generalist with no re-routing and no banishing just keeps grinding the
        // same rotting context; allow the loop so its decline is visible.
        if !levers.route && !levers.banish && attempts.len() >= MAX_ATTEMPTS {
            break;
        }
    }

    Rite {
        task: task.to_string(),
        ray,
        statement,
        sigil,
        attempts,
        succeeded,
    }
}

/// The fittest adept for `ray` not yet tried in this rite. Falls back to the
/// global fittest if all have been tried (the rite is about to end anyway).
fn next_fittest(pact: &Pact, ray: Ray, tried: &[usize]) -> usize {
    let mut best = None;
    let mut best_fit = f64::MIN;
    for (i, m) in pact.members.iter().enumerate() {
        if tried.contains(&i) {
            continue;
        }
        let f = m.fitness(ray);
        if f > best_fit {
            best_fit = f;
            best = Some(i);
        }
    }
    best.unwrap_or_else(|| pact.route(ray))
}

/// A one-line distilled lesson for the egregore ledger.
fn short_lesson(task: &str) -> String {
    let t: String = task
        .split_whitespace()
        .take(8)
        .collect::<Vec<_>>()
        .join(" ");
    format!("charged true: {t}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_ceremony_beats_raw_on_average() {
        // The headline: full kaos solves materially more than the raw baseline
        // over a batch of tasks, same seeds. (The benchmark elaborates; this guards
        // the direction of the effect.)
        let tasks = sample_tasks();
        let full = run_batch(&tasks, Levers::full(), 2026);
        let raw = run_batch(&tasks, Levers::raw(), 2026);
        assert!(
            full > raw + 10,
            "full ({full}) should clear raw ({raw}) by a wide margin"
        );
    }

    #[test]
    fn banishing_alone_helps() {
        let tasks = sample_tasks();
        let mut with = Levers::raw();
        with.banish = true;
        let a = run_batch(&tasks, with, 99);
        let b = run_batch(&tasks, Levers::raw(), 99);
        assert!(a >= b, "banishing should not hurt ({a} vs {b})");
    }

    fn run_batch(tasks: &[String], levers: Levers, seed: u64) -> usize {
        let mut pact = Pact::convene();
        let mut rng = Rng::new(seed);
        let mut solved = 0;
        for t in tasks {
            if perform(&mut pact, t, levers, &mut rng).succeeded {
                solved += 1;
            }
        }
        solved
    }

    fn sample_tasks() -> Vec<String> {
        let verbs = [
            "fix the crash in",
            "delete",
            "optimize",
            "scaffold a new",
            "bump deps for",
            "redesign the api of",
            "integrate",
        ];
        let nouns = [
            "parser",
            "scheduler",
            "worker pool",
            "cache",
            "router",
            "auth flow",
            "billing module",
            "render loop",
        ];
        let mut v = Vec::new();
        for (i, verb) in verbs.iter().enumerate() {
            for (j, noun) in nouns.iter().enumerate() {
                if (i + j) % 2 == 0 {
                    v.push(format!("{verb} {noun}"));
                }
            }
        }
        v
    }
}
