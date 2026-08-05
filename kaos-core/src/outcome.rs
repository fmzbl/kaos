//! What a run is willing to say it knows.
//!
//! The harness had two outcomes: it shipped an answer, or it failed.
//! `grep -rn "abstain"` returned nothing across the whole workspace. So a run
//! that could not verify its work shipped it anyway, and a run whose gate
//! refused every candidate reported the same thing as a run whose model never
//! answered at all.
//!
//! There are three outcomes, and the third is the point.
//!
//! # Why this is worth a type
//!
//! From the agi-testing work, and it is the measured result rather than a
//! preference: **the Lean gate's win was soundness — zero false ships, and
//! abstentions — not accuracy.** The gate did not make the model better at
//! mathematics. It made the harness unable to claim something it had not
//! checked.
//!
//! Folding [`Outcome::Abstained`] into either neighbour destroys the only
//! distinction that matters:
//!
//! - folded into **failure**, a verified-negative looks like a crash, and a run
//!   that correctly refused to ship a wrong answer is indistinguishable from one
//!   that fell over;
//! - folded into **success**, it is a **false ship** — an answer presented as
//!   though something had agreed with it, which is the outcome the whole thing
//!   exists to make impossible.
//!
//! So an abstention is surfaced as an abstention in the terminal, in the visual
//! run browser, and in the exit code. See `docs/EDGE.md`, which registers the
//! trade: every accuracy metric gets worse, deliberately.

/// The exit code a run that abstained leaves behind.
///
/// Public because a script checking for it should be able to name it rather
/// than write `4` and hope.
pub const ABSTAINED_EXIT: i32 = 4;

/// What agreed with an answer before it shipped.
///
/// The distinction a `Shipped` has to carry, because "an answer" and "an answer
/// a test suite accepted" are different claims and a harness that reported them
/// alike would be making the stronger one for free.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verified {
    /// A gate ran and accepted it — a test suite, a compiler, a proof kernel.
    /// The strong case, and the only one that earns the word *verified*.
    By(String),
    /// Branches agreed with each other. Self-consistency is evidence and is not
    /// a check: five branches can agree and all be wrong, which is exactly the
    /// case a sound gate catches and a vote cannot.
    Consensus { ballots: usize, of: usize },
    /// Nothing agreed with it. One model said it once.
    ///
    /// Not a failure — most useful work is unverifiable and shipping it is
    /// correct. It is a claim about evidence, and the honest one is *none*.
    Unchecked,
}

impl Verified {
    /// Whether something outside the answer itself accepted it.
    #[must_use]
    pub fn is_checked(&self) -> bool {
        matches!(self, Self::By(_))
    }

    /// One phrase for a status line.
    #[must_use]
    pub fn summary(&self) -> String {
        match self {
            Self::By(gate) => format!("verified by `{gate}`"),
            Self::Consensus { ballots, of } => format!("{ballots} of {of} branches agreed"),
            Self::Unchecked => "unchecked".to_string(),
        }
    }
}

/// How a run ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// An answer, and what agreed with it.
    Shipped {
        /// What the run produced.
        answer: String,
        /// What agreed with it, which may be nothing.
        gate: Verified,
    },
    /// The run could not do the work: a provider failed, a program did not
    /// parse, a budget ran out mid-way.
    Failed {
        /// What went wrong.
        error: String,
    },
    /// The work was done and could not be verified.
    ///
    /// Deliberately **not** a failure: the distinction is the whole point. The
    /// work is kept in `work` rather than discarded, because a person deciding
    /// what to do next needs to see what the run actually produced — an
    /// abstention that threw the work away would be a failure with extra steps.
    Abstained {
        /// What the run produced but would not stand behind.
        work: String,
        /// What refused it, in a sentence a reader can act on.
        why: String,
    },
}

impl Outcome {
    /// Whether this run put an answer forward.
    #[must_use]
    pub fn is_shipped(&self) -> bool {
        matches!(self, Self::Shipped { .. })
    }

    /// Whether this run did the work and declined to stand behind it.
    #[must_use]
    pub fn is_abstention(&self) -> bool {
        matches!(self, Self::Abstained { .. })
    }

    /// The word a status line shows.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Shipped { .. } => "SHIPPED",
            Self::Failed { .. } => "FAILED",
            Self::Abstained { .. } => "ABSTAINED",
        }
    }

    /// The process exit code.
    ///
    /// Three codes for three outcomes, because a script has the same right to
    /// tell them apart that a person does. `0` shipped, `1` failed, and **`4`
    /// abstained** — distinct from failure, so a caller that treats non-zero as
    /// "something broke" is not silently told a sound refusal was a crash.
    ///
    /// `4` rather than the obvious `3` because `kaos rebis run` already exits
    /// `3` when a run reports diagnostics, and quietly reusing it would make an
    /// abstention indistinguishable from the thing it is most often confused
    /// with. A number is cheap; the confusion is what this type costs money to
    /// prevent.
    #[must_use]
    pub const fn exit_code(&self) -> i32 {
        match self {
            Self::Shipped { .. } => 0,
            Self::Failed { .. } => 1,
            Self::Abstained { .. } => ABSTAINED_EXIT,
        }
    }

    /// The answer, if this run is willing to stand behind one.
    ///
    /// `None` for an abstention **even though the work exists**, and that is the
    /// method rather than an oversight: a caller reaching for "the answer" is
    /// asking what to use, and an unverified attempt is not that. Reaching the
    /// work requires naming the abstention, which is one deliberate keystroke
    /// between a caller and a false ship.
    #[must_use]
    pub fn answer(&self) -> Option<&str> {
        match self {
            Self::Shipped { answer, .. } => Some(answer),
            Self::Failed { .. } | Self::Abstained { .. } => None,
        }
    }

    /// Everything the run produced, shipped or not — for a person reading the
    /// result rather than a caller consuming it.
    #[must_use]
    pub fn work(&self) -> Option<&str> {
        match self {
            Self::Shipped { answer, .. } => Some(answer),
            Self::Abstained { work, .. } => Some(work),
            Self::Failed { .. } => None,
        }
    }

    /// One line: what happened, and on what evidence.
    #[must_use]
    pub fn summary(&self) -> String {
        match self {
            Self::Shipped { gate, .. } => format!("shipped · {}", gate.summary()),
            Self::Failed { error } => format!("failed · {error}"),
            Self::Abstained { why, .. } => format!("abstained · {why}"),
        }
    }
}

/// Read a finished Rebis run as one outcome of three.
///
/// The three signals, in the order they are trusted:
///
/// 1. **A host mediator refused.** `MediatorHosted { result: None }` means a
///    gate ran and nothing survived it. That is an abstention and it outranks
///    everything else — a run whose gate said no has been verified, negatively,
///    and reporting it as a failure would throw away the one fact it
///    established.
/// 2. **An answer.** Whatever agreed with it is read off the same events:
///    `[check]` accepting a branch is [`Verified::By`], `[vote]` is consensus,
///    and anything else is unchecked.
/// 3. **Neither.** No answer and no refusal is a failure — the model never
///    answered, a budget ran out, a program did not parse.
///
/// `gate` is what `[check]` was configured to run, and is used only to name the
/// verifier in a `Verified::By`.
#[must_use]
pub fn of_run(result: &rebis_lang::Orchestration, gate: &str) -> Outcome {
    use rebis_lang::ExecutionEvent;

    let mut refusal: Option<(String, String)> = None;
    let mut judged: Option<(String, usize)> = None;
    let mut pending_branches = 0;
    for event in &result.events {
        match event {
            ExecutionEvent::MediatorStarted { branches: count } => pending_branches = *count,
            ExecutionEvent::MediatorHosted { name, result, why } => match result {
                Some(_) => {
                    judged = Some((name.clone(), pending_branches));
                    pending_branches = 0;
                }
                None => {
                    refusal = Some((name.clone(), why.clone()));
                    pending_branches = 0;
                }
            },
            _ => {}
        }
    }

    // A refusal wins even where a later square produced an answer: the run
    // reached a gate that said no, and a harness that shipped past it would be
    // doing the exact thing this type exists to prevent.
    if let Some((name, why)) = refusal {
        return Outcome::Abstained {
            // What it produced, so a person can look at it. `output` may be
            // `None` when the refused square WAS the program, in which case the
            // trace's last answer is the nearest thing to the work.
            work: result
                .output
                .clone()
                .or_else(|| {
                    result
                        .firings
                        .iter()
                        .rev()
                        .find_map(|firing| firing.answer.clone())
                })
                .unwrap_or_default(),
            why: if why.is_empty() {
                format!("`[{name}]` accepted no candidate")
            } else {
                why
            },
        };
    }

    match &result.output {
        Some(answer) => Outcome::Shipped {
            answer: answer.clone(),
            gate: match judged.as_ref().map(|(name, _)| name.as_str()) {
                Some("check") => Verified::By(if gate.trim().is_empty() {
                    "the configured gate".to_string()
                } else {
                    gate.trim().to_string()
                }),
                // A vote is evidence, not a check. Which is why it gets its own
                // variant rather than borrowing `By`.
                Some("vote") => {
                    let branches = judged.as_ref().map_or(0, |(_, count)| *count);
                    Verified::Consensus {
                        ballots: branches,
                        of: branches,
                    }
                }
                _ => Verified::Unchecked,
            },
        },
        None => Outcome::Failed {
            error: if result.diagnostics.is_empty() {
                "the run produced no answer".to_string()
            } else {
                result
                    .diagnostics
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(" · ")
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn abstention() -> Outcome {
        Outcome::Abstained {
            work: "a patch that does not compile".to_string(),
            why: "no candidate passed `cargo test`".to_string(),
        }
    }

    #[test]
    fn an_abstention_is_not_a_failure_and_not_a_ship() {
        let outcome = abstention();
        assert!(outcome.is_abstention());
        assert!(!outcome.is_shipped());
        assert_ne!(outcome.exit_code(), 0, "it did not ship");
        assert_ne!(
            outcome.exit_code(),
            Outcome::Failed {
                error: "provider down".to_string()
            }
            .exit_code(),
            "and it is not the same event as falling over"
        );
    }

    #[test]
    fn an_abstention_keeps_the_work_and_withholds_the_answer() {
        // The distinction the type exists to enforce: a caller asking for "the
        // answer" gets nothing, and a person asking what happened gets
        // everything. One deliberate keystroke between a caller and a false ship.
        let outcome = abstention();
        assert_eq!(outcome.answer(), None);
        assert_eq!(outcome.work(), Some("a patch that does not compile"));
    }

    #[test]
    fn every_outcome_has_its_own_exit_code() {
        let codes = [
            Outcome::Shipped {
                answer: "a".to_string(),
                gate: Verified::Unchecked,
            }
            .exit_code(),
            Outcome::Failed {
                error: "b".to_string(),
            }
            .exit_code(),
            abstention().exit_code(),
        ];
        let mut sorted = codes.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 3, "{codes:?} collapses two outcomes into one");
        assert_eq!(codes[0], 0, "only a ship is success");
    }

    #[test]
    fn a_vote_is_evidence_and_not_a_check() {
        // Five branches can agree and all be wrong. Reporting consensus as
        // verification is the false ship this distinction exists to prevent.
        assert!(!Verified::Consensus { ballots: 5, of: 5 }.is_checked());
        assert!(!Verified::Unchecked.is_checked());
        assert!(Verified::By("cargo test".to_string()).is_checked());
    }

    #[test]
    fn the_summary_says_what_agreed() {
        assert_eq!(
            Outcome::Shipped {
                answer: "42".to_string(),
                gate: Verified::By("pytest -q".to_string()),
            }
            .summary(),
            "shipped · verified by `pytest -q`"
        );
        assert_eq!(
            Outcome::Shipped {
                answer: "42".to_string(),
                gate: Verified::Consensus { ballots: 4, of: 5 },
            }
            .summary(),
            "shipped · 4 of 5 branches agreed"
        );
        assert!(abstention().summary().starts_with("abstained · "));
    }

    #[test]
    fn the_label_never_reads_as_its_neighbour() {
        let labels = [
            Outcome::Shipped {
                answer: String::new(),
                gate: Verified::Unchecked,
            }
            .label(),
            Outcome::Failed {
                error: String::new(),
            }
            .label(),
            abstention().label(),
        ];
        let mut sorted = labels.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 3, "{labels:?}");
    }
}
