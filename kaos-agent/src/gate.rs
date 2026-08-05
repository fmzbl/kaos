//! The judgements Kaos makes that Rebis cannot.
//!
//! A Rebis mediator square `([M] A B …)` whose head is pure symbols is judged by
//! the calculus: each branch answer is scored by round-trip holonomy onto the
//! mediator's own tokens and the lowest wins. It is free, deterministic, and
//! blind to anything outside the run — the right default, and not the only
//! judgement worth making.
//!
//! Proving that is what `tests/myth_equivalence.rs` was for, and the answer was
//! not the expected one. **Three of `myth`'s four gates have no Rebis spelling,
//! not one.** `[consensus]` does not count ballots; it prefers whichever branch
//! reads most like the word *consensus*. `[first]` is not positional. And there
//! was never a shell gate at all. Rebis has no table of mediator names — every
//! pure-symbol head runs the same scoring code — so these were one gate wearing
//! three words.
//!
//! This module is the other half of that finding. `Oracle::mediate` lets a host
//! claim a mediator by name, and Kaos claims exactly three:
//!
//! | written | what it means | costs |
//! |---|---|---|
//! | `[vote]` | the modal answer — self-consistency | nothing |
//! | `[first]` | the first branch that came back | nothing |
//! | `[check]` | the branch a verifier accepts | one subprocess per branch |
//!
//! **No operator was added.** To the parser, the formatter, the canvas and the
//! record these are symbol mediators like any other; only the judgement changes,
//! and only under a host that says it owns the word. Every other name — the
//! `[merger]` and `[judge]` a program already wrote — is declined and means what
//! it always meant. That is what makes this additive instead of a migration.
//!
//! # The authority question
//!
//! `[check]` runs a subprocess, so a written program can start one. Two things
//! bound that, and both are deliberate:
//!
//! - **The program cannot say what runs.** A square's head is one atom, so
//!   `[check]` names a gate and cannot carry a command into one. What runs is
//!   [`Check::command`], which comes from the operator's configuration.
//! - **The run must hold command authority.** Without `--allow-tools` the gate
//!   is [`Verdict::Failed`] — a diagnostic, not an answer. The dangerous
//!   outcome is the quiet one: a square answering as though it had been
//!   verified when nothing verified it.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use rebis_lang::Mediation;

/// What a host mediator decided about a square's branches.
///
/// The same four outcomes [`Mediation`] has, in Kaos's own vocabulary so a
/// mediator can be written and tested without a Rebis run around it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Not this mediator's name.
    Declined,
    /// This branch, by zero-based index.
    Chose(usize),
    /// The judgement ran and nothing survived it.
    Refused(String),
    /// The judgement could not run.
    Failed(String),
}

impl From<Verdict> for Mediation {
    fn from(verdict: Verdict) -> Self {
        match verdict {
            Verdict::Declined => Self::Declined,
            Verdict::Chose(index) => Self::Chose(index),
            Verdict::Refused(why) => Self::Refused { why },
            Verdict::Failed(message) => Self::Failed { message },
        }
    }
}

/// A mediator the host supplies, resolved by name when a program writes
/// `[name …]`.
pub trait HostMediator: Sync {
    /// The single symbol this mediator answers to.
    fn name(&self) -> &str;

    /// Judge the accepted branch answers, in source order.
    ///
    /// Called only when the square's head is exactly [`HostMediator::name`], so
    /// an implementation never has to check.
    fn pick(&self, branches: &[String]) -> Verdict;
}

/// The modal answer: self-consistency, which is the gate with a measured number
/// behind it (+23pts AIME2025 on a mid-band model, `solve.rs`).
///
/// Ties go to the earliest, so the result does not depend on iteration order.
/// Note what it does **not** do: it never invents a winner. Where every branch
/// is distinct there is no mode, and a square of all-different answers is
/// refused rather than silently resolved to the first — a vote among answers
/// that agree about nothing has decided nothing, and saying so is the point.
pub struct Vote;

impl HostMediator for Vote {
    fn name(&self) -> &str {
        "vote"
    }

    fn pick(&self, branches: &[String]) -> Verdict {
        let mut best: Option<(usize, usize)> = None;
        for (index, branch) in branches.iter().enumerate() {
            let ballots = branches.iter().filter(|other| *other == branch).count();
            if best.is_none_or(|(most, _)| ballots > most) {
                best = Some((ballots, index));
            }
        }
        match best {
            Some((ballots, index)) if ballots > 1 => Verdict::Chose(index),
            Some(_) => Verdict::Refused(format!(
                "{} branches and no two agreed, so there is no majority",
                branches.len()
            )),
            None => Verdict::Refused("no branch answered".to_string()),
        }
    }
}

/// The first branch that came back — any answer will do, you just want one that
/// did not fizzle.
///
/// Positional, which is exactly what a symbol mediator cannot be: the branches
/// arrive in source order and this takes the head of them.
pub struct First;

impl HostMediator for First {
    fn name(&self) -> &str {
        "first"
    }

    fn pick(&self, branches: &[String]) -> Verdict {
        branches
            .iter()
            .position(|branch| !branch.trim().is_empty())
            .map_or_else(
                || Verdict::Refused("every branch was empty".to_string()),
                Verdict::Chose,
            )
    }
}

/// The branch a real verifier accepts: a test suite, a compiler, a proof kernel.
///
/// This is the gate whose value is *soundness* rather than accuracy. A vote can
/// be confidently wrong when the branches agree and are all wrong; a sound gate
/// cannot be fooled, and its win is that it refuses rather than that it picks
/// well. Which is why [`Verdict::Refused`] is a first-class outcome here and not
/// an error.
pub struct Check {
    /// What runs. From the host's configuration — never from the program, which
    /// has no way to say.
    pub command: String,
    /// Wall cap for one candidate. A gate may run a whole suite.
    pub timeout: Duration,
    /// Whether this run may start a subprocess at all.
    pub authorised: bool,
}

impl Check {
    /// Run the verifier over one candidate.
    ///
    /// The candidate is handed over twice — on `$CANDIDATE` and on stdin — so a
    /// one-line `grep` and a `python3 verify.py` both work without the program
    /// having to know which the gate wants.
    fn accepts(&self, candidate: &str) -> bool {
        use std::io::Write;

        let child = Command::new("sh")
            .arg("-c")
            .arg(&self.command)
            .env("CANDIDATE", candidate)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        let Ok(mut child) = child else {
            return false;
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(candidate.as_bytes());
        }
        // Poll rather than block: a gate that hangs would hang the run, and a
        // verifier that never returns is the same outcome as one that refuses.
        let deadline = Instant::now() + self.timeout;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => return status.success(),
                Ok(None) if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return false;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(_) => return false,
            }
        }
    }
}

impl HostMediator for Check {
    fn name(&self) -> &str {
        "check"
    }

    fn pick(&self, branches: &[String]) -> Verdict {
        if !self.authorised {
            return Verdict::Failed(
                "`[check]` runs a verifier and this run holds no command authority; \
                 re-run with `--allow-tools`"
                    .to_string(),
            );
        }
        if self.command.trim().is_empty() {
            return Verdict::Failed(
                "`[check]` has no verifier configured; set `KAOS_GATE`".to_string(),
            );
        }
        branches
            .iter()
            .position(|branch| self.accepts(branch))
            .map_or_else(
                || {
                    Verdict::Refused(format!(
                        "none of {} candidates passed `{}`",
                        branches.len(),
                        self.command
                    ))
                },
                Verdict::Chose,
            )
    }
}

/// Every mediator this host claims.
///
/// A short, closed list on purpose. A host that claimed every name it was handed
/// would silently redefine `[merger]` and `[judge]` for every program it ran.
/// The lifetime is what lets a mediator borrow the run it belongs to — an
/// agentic gate holds the [`crate::myth::Cast`] whose workspaces it re-applies
/// candidates into, and that cast borrows the session's model.
pub struct Mediators<'a>(Vec<Box<dyn HostMediator + 'a>>);

impl<'a> Mediators<'a> {
    /// The three Kaos supplies.
    ///
    /// `gate` is the verifier `[check]` runs and `authorised` is whether this
    /// run may start one. `[vote]` and `[first]` are pure and are always
    /// available: neither reaches outside the run, so neither needs authority.
    #[must_use]
    pub fn standard(gate: String, gate_timeout: Duration, authorised: bool) -> Self {
        Self(vec![
            Box::new(Vote),
            Box::new(First),
            Box::new(Check {
                command: gate,
                timeout: gate_timeout,
                authorised,
            }),
        ])
    }

    /// Claim nothing — the posture of a run with no gates configured, and the
    /// one that leaves every program meaning exactly what the language says.
    #[must_use]
    pub fn none() -> Self {
        Self(Vec::new())
    }

    /// Add a mediator. A second registration of a name shadows the first, so a
    /// frontend can replace `[check]` without rebuilding the list.
    pub fn claim(&mut self, mediator: Box<dyn HostMediator + 'a>) {
        self.0.insert(0, mediator);
    }

    /// The names this host owns, for a diagnostic or a settings screen.
    #[must_use]
    pub fn names(&self) -> Vec<&str> {
        self.0.iter().map(|mediator| mediator.name()).collect()
    }

    /// Answer Rebis's `Oracle::mediate`.
    ///
    /// The head arrives as written. Anything that is not exactly one of the
    /// claimed names is [`Mediation::Declined`] — including a multi-word head,
    /// which cannot be one of ours and must keep meaning what it means.
    #[must_use]
    pub fn judge(&self, mediator: &str, branches: &[String]) -> Mediation {
        let head = mediator.trim();
        self.0
            .iter()
            .find(|claimed| claimed.name() == head)
            .map_or(Mediation::Declined, |claimed| claimed.pick(branches).into())
    }
}

impl Default for Mediators<'_> {
    fn default() -> Self {
        Self::none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn branches(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_string()).collect()
    }

    #[test]
    fn vote_takes_the_modal_answer() {
        assert_eq!(
            Vote.pick(&branches(&["42", "7", "42", "42"])),
            Verdict::Chose(0)
        );
        // Ties resolve to the earliest, so the answer does not depend on order
        // of iteration over equal counts.
        assert_eq!(
            Vote.pick(&branches(&["a", "b", "a", "b"])),
            Verdict::Chose(0)
        );
    }

    #[test]
    fn a_vote_among_answers_that_agree_about_nothing_refuses() {
        // The distinction that makes a vote worth having. Five different answers
        // is not "the first one wins"; it is a conclave that found no signal, and
        // resolving it anyway would launder a guess as a consensus.
        let verdict = Vote.pick(&branches(&["a", "b", "c", "d", "e"]));
        assert!(
            matches!(&verdict, Verdict::Refused(why) if why.contains("no two agreed")),
            "{verdict:?}"
        );
        assert_eq!(
            Vote.pick(&[]),
            Verdict::Refused("no branch answered".into())
        );
    }

    #[test]
    fn first_is_positional_and_skips_the_empty() {
        assert_eq!(First.pick(&branches(&["one", "two"])), Verdict::Chose(0));
        assert_eq!(First.pick(&branches(&["  ", "two"])), Verdict::Chose(1));
        assert!(matches!(
            First.pick(&branches(&["", " "])),
            Verdict::Refused(_)
        ));
    }

    #[test]
    fn check_without_authority_fails_rather_than_answering() {
        // R7. The quiet failure is the dangerous one.
        let gate = Check {
            command: "true".to_string(),
            timeout: Duration::from_secs(1),
            authorised: false,
        };
        let verdict = gate.pick(&branches(&["a", "b"]));
        assert!(
            matches!(&verdict, Verdict::Failed(why) if why.contains("--allow-tools")),
            "{verdict:?}"
        );
    }

    #[test]
    fn check_keeps_the_first_candidate_the_verifier_accepts() {
        let gate = Check {
            // The candidate arrives on `$CANDIDATE` and on stdin; this reads the
            // variable, and `check_reads_the_candidate_on_stdin` reads the pipe.
            command: r#"[ "$CANDIDATE" = "good" ]"#.to_string(),
            timeout: Duration::from_secs(10),
            authorised: true,
        };
        assert_eq!(
            gate.pick(&branches(&["bad", "good", "good"])),
            Verdict::Chose(1)
        );
    }

    #[test]
    fn check_reads_the_candidate_on_stdin() {
        let gate = Check {
            command: "grep -q good".to_string(),
            timeout: Duration::from_secs(10),
            authorised: true,
        };
        assert_eq!(
            gate.pick(&branches(&["bad", "very good"])),
            Verdict::Chose(1)
        );
    }

    #[test]
    fn a_gate_nothing_passes_refuses_and_says_what_it_ran() {
        let gate = Check {
            command: "false".to_string(),
            timeout: Duration::from_secs(10),
            authorised: true,
        };
        let verdict = gate.pick(&branches(&["a", "b"]));
        assert!(
            matches!(&verdict, Verdict::Refused(why) if why.contains("false") && why.contains('2')),
            "{verdict:?}"
        );
    }

    #[test]
    fn a_gate_that_hangs_is_bounded() {
        let gate = Check {
            command: "sleep 30".to_string(),
            timeout: Duration::from_millis(200),
            authorised: true,
        };
        let started = Instant::now();
        assert!(matches!(gate.pick(&branches(&["a"])), Verdict::Refused(_)));
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the timeout must bound the gate, not the sleep"
        );
    }

    #[test]
    fn an_unclaimed_name_is_declined() {
        let mediators = Mediators::standard("true".into(), Duration::from_secs(1), true);
        assert_eq!(
            mediators.judge("merger", &branches(&["a", "b"])),
            Mediation::Declined
        );
        // A multi-word head cannot be one of ours, and must keep its meaning.
        assert_eq!(
            mediators.judge("check the work", &branches(&["a"])),
            Mediation::Declined
        );
        assert_eq!(mediators.names(), vec!["vote", "first", "check"]);
    }

    #[test]
    fn claiming_nothing_declines_everything() {
        let mediators = Mediators::none();
        for name in ["vote", "first", "check", "merger"] {
            assert_eq!(
                mediators.judge(name, &branches(&["a", "a"])),
                Mediation::Declined,
                "{name}"
            );
        }
    }

    #[test]
    fn a_later_claim_shadows_an_earlier_one() {
        struct AlwaysSecond;
        impl HostMediator for AlwaysSecond {
            fn name(&self) -> &str {
                "vote"
            }
            fn pick(&self, _branches: &[String]) -> Verdict {
                Verdict::Chose(1)
            }
        }
        let mut mediators = Mediators::standard("true".into(), Duration::from_secs(1), true);
        mediators.claim(Box::new(AlwaysSecond));
        assert_eq!(
            mediators.judge("vote", &branches(&["a", "b", "a"])),
            Mediation::Chose(1)
        );
    }
}
