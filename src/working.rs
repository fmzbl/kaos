//! The Working — decomposition as a lever: turn one impossible conjuration into a
//! chain of mid-band operations.
//!
//! *Liber Kaos* names both halves of the idea:
//!
//! - **No k rescues the floor.** The second equation leaves P ≈ 0 untouched by any
//!   M < 1: *"the bottom line of the graph remains at zero until M = 1."* A task a
//!   model virtually never one-shots is not a sampling problem.
//! - **Raise P by ordinary means first.** *"The magician should take all possible
//!   ordinary steps to increase the probability of the desired result occurring by
//!   chance alone, before and after using magic."* And enchantment works on
//!   *sequences*: *"enchantments cast well in advance have a greater chance to
//!   modify sequences of probability in one's favor."*
//!
//! The ordinary step that raises P is **decomposition**: a working of N operations,
//! each with its own gate, each individually mid-band — exactly where the second
//! equation says the conclave's lift is maximal. The arithmetic is the whole
//! mechanism: if a whole task has p ≈ 0.2 one-shot, nine attempts give
//! `1−0.8⁹ ≈ 0.87` *only if a gate can tell success* — but the same task as three
//! steps of p ≈ 0.6 with three verified attempts each gives
//! `(1−0.4³)³ ≈ 0.82` **while shipping partial progress and failing legibly at the
//! step that broke**, and each step's gate is a far sharper Weighing than one
//! end-to-end oracle. Where whole-task p is truly floor-bound (0.05), no budget of
//! whole-task retries clears 40%, and the chain is the only path that pays.
//!
//! Each operation is performed the way the conclave already works: isolated copy,
//! attempt, Weighing; the first *verified* attempt ships back and the chain moves
//! on ("there is very little point in repeating a conjuration unless there is a
//! chance of doing it better" — a passed gate is that point). A step that exhausts
//! its charges halts the working honestly: the caller learns *which* operation is
//! beyond the adept, instead of receiving one opaque end-to-end failure.

use std::io;
use std::path::Path;

use crate::agent::{write_files_into, AdeptAgent, Workspace};
use crate::rng::Rng;

/// One operation of a working: a narrow statement of intent and its own gate.
#[derive(Clone, Debug)]
pub struct Operation {
    pub intent: String,
    /// The Weighing for this step (exit 0 == the operation is done). Cumulative
    /// gates (step N's gate re-asserts steps 1..N) keep a later op from silently
    /// undoing an earlier one.
    pub gate: String,
}

/// The record of one operation's performance.
#[derive(Clone, Debug)]
pub struct OpOutcome {
    pub intent: String,
    pub attempts: usize,
    pub verified: bool,
}

/// The record of a whole working.
#[derive(Clone, Debug)]
pub struct WorkingOutcome {
    pub ops: Vec<OpOutcome>,
    /// True iff every operation shipped a verified diff.
    pub completed: bool,
    /// Total model attempts spent across all operations.
    pub samples: usize,
}

/// Perform a working against `target`: each operation in sequence, each as
/// verified best-of-`k` with early exit — attempts run in a fresh isolated copy of
/// the *current* target (which carries all previously shipped steps), and the
/// first attempt to pass the operation's gate ships back immediately. A step that
/// exhausts its k charges halts the working; everything shipped so far stays (the
/// gates that passed still pass).
///
/// `edit_paths` are the files shown to the adept each attempt.
pub fn perform_working(
    target: &Path,
    ops: &[Operation],
    agent: &dyn AdeptAgent,
    edit_paths: &[&str],
    k: usize,
    rng: &mut Rng,
    mut progress: impl FnMut(&str),
) -> io::Result<WorkingOutcome> {
    let k = k.max(1);
    let mut outcomes = Vec::with_capacity(ops.len());
    let mut samples = 0usize;
    let mut completed = true;

    for (oi, op) in ops.iter().enumerate() {
        let mut verified = false;
        let mut attempts = 0usize;
        for a in 0..k {
            attempts = a + 1;
            samples += 1;
            let ws = Workspace::isolate(target)?;
            let files = ws.read_files(edit_paths);
            let patch = agent.attempt(&op.intent, &files, rng);
            let applied = !patch.is_empty() && ws.apply(&patch).is_ok();
            let passed = applied && ws.verify(&op.gate).0;
            progress(&format!(
                "op {}/{} attempt {}/{} {}",
                oi + 1,
                ops.len(),
                attempts,
                k,
                if passed {
                    "\u{2713} weighed true"
                } else {
                    "\u{2717}"
                },
            ));
            if passed {
                // Ship this step's verified diff into the target; the next
                // operation builds on it.
                let changes = ws.changed_files(target)?;
                write_files_into(target, &changes)?;
                verified = true;
                break;
            }
        }
        outcomes.push(OpOutcome {
            intent: op.intent.clone(),
            attempts,
            verified,
        });
        if !verified {
            completed = false;
            break; // halt honestly at the operation that broke
        }
    }

    Ok(WorkingOutcome {
        ops: outcomes,
        completed,
        samples,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{Edit, Patch};
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    /// A deterministic adept that answers each operation by keyword: intent
    /// containing "alpha" fixes ALPHA, "beta" fixes BETA, anything else flails.
    struct StepAgent;
    impl AdeptAgent for StepAgent {
        fn name(&self) -> &str {
            "Soror Gradatim"
        }
        fn attempt(&self, task: &str, files: &BTreeMap<String, String>, _rng: &mut Rng) -> Patch {
            let cur = files.get("sol.txt").cloned().unwrap_or_default();
            let fixed = if task.contains("alpha") {
                cur.replace("ALPHA_WRONG", "ALPHA_RIGHT")
            } else if task.contains("beta") {
                cur.replace("BETA_WRONG", "BETA_RIGHT")
            } else {
                cur.replace("nothing", "still nothing")
            };
            Patch {
                edits: vec![Edit::Write {
                    path: "sol.txt".into(),
                    contents: fixed,
                }],
            }
        }
    }

    fn arena(contents: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let d = std::env::temp_dir().join(format!(
            "kaos-working-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("sol.txt"), contents).unwrap();
        d
    }

    fn op(intent: &str, gate: &str) -> Operation {
        Operation {
            intent: intent.into(),
            gate: gate.into(),
        }
    }

    #[test]
    fn a_working_chains_verified_steps_into_the_target() {
        let dir = arena("ALPHA_WRONG\nBETA_WRONG\n");
        let ops = [
            op("fix alpha", "grep -q ALPHA_RIGHT sol.txt"),
            // Cumulative gate: beta's Weighing re-asserts alpha survived.
            op(
                "fix beta",
                "grep -q ALPHA_RIGHT sol.txt && grep -q BETA_RIGHT sol.txt",
            ),
        ];
        let mut rng = Rng::new(7);
        let out =
            perform_working(&dir, &ops, &StepAgent, &["sol.txt"], 3, &mut rng, |_| {}).unwrap();
        assert!(out.completed, "both operations should weigh true");
        assert_eq!(out.samples, 2, "each mid-band step lands first try here");
        let final_text = std::fs::read_to_string(dir.join("sol.txt")).unwrap();
        assert_eq!(final_text, "ALPHA_RIGHT\nBETA_RIGHT\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_working_halts_at_the_operation_that_breaks_and_keeps_shipped_steps() {
        let dir = arena("ALPHA_WRONG\nGAMMA_WRONG\n");
        let ops = [
            op("fix alpha", "grep -q ALPHA_RIGHT sol.txt"),
            op("fix gamma", "grep -q GAMMA_RIGHT sol.txt"), // StepAgent cannot do this
            op("fix beta", "grep -q BETA_RIGHT sol.txt"),   // must never be attempted
        ];
        let mut rng = Rng::new(7);
        let out =
            perform_working(&dir, &ops, &StepAgent, &["sol.txt"], 2, &mut rng, |_| {}).unwrap();
        assert!(!out.completed);
        assert_eq!(
            out.ops.len(),
            2,
            "the chain halts at gamma; beta is never charged"
        );
        assert!(out.ops[0].verified);
        assert!(!out.ops[1].verified);
        assert_eq!(out.ops[1].attempts, 2, "gamma exhausted its k charges");
        assert_eq!(out.samples, 3);
        // Alpha's verified progress SHIPPED and survives the halt.
        let text = std::fs::read_to_string(dir.join("sol.txt")).unwrap();
        assert!(text.contains("ALPHA_RIGHT"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_arithmetic_of_decomposition() {
        // The claim the mechanism rests on, as pure math: with a per-attempt gate,
        // a floor-bound whole task loses to the same budget spent on mid-band steps.
        let whole_p: f64 = 0.05; // the model almost never one-shots the whole task
        let step_p: f64 = 0.6; //  ...but each narrow step is mid-band
        let budget = 9.0_f64;
        let whole = 1.0 - (1.0 - whole_p).powf(budget); // best-of-9 on the whole
        let steps = 3.0_f64;
        let chain = (1.0 - (1.0 - step_p).powf(budget / steps)).powf(steps);
        assert!(
            chain > whole + 0.2,
            "chain {chain:.2} must dominate whole {whole:.2}"
        );
        // And Carroll's floor: at p = 0, no budget rescues the whole task.
        assert!((1.0 - (1.0_f64 - 0.0).powf(1e6)).abs() < 1e-12);
    }
}
