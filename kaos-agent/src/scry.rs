//! Scrying — divination as compute allocation; the quorum that adjourns itself.
//!
//! *Liber Kaos* binds the two halves of this module together:
//!
//! 1. **Divination is estimation under a probabilistic limit.** *"At any moment it
//!    may be possible to divine the etheric pattern of the future of that moment
//!    and pick the most probable future, but it is only a probability, not a
//!    certainty."* A scry is a cheap, imperfect read of P — never an oracle.
//!    Engineering reading: you can *estimate* whether more samples will change the
//!    outcome from the samples already drawn, and act on the estimate.
//!
//! 2. **Repetition past the point of information is waste.** *"There is very
//!    little point in repeating a conjuration unless there is a chance of doing it
//!    better."* And of the collective: *"the effects of a number of persons
//!    conjuring for a common objective never exceeds the best result that any one
//!    of them might achieve"* — a conclave is `max`, not `sum`. Once the vote is
//!    beyond overturning, further charges cannot move the decision. The quorum
//!    should **adjourn**.
//!
//! 3. **"Enchant Long and Divine Short."** Divine from the near evidence — the
//!    ballots already cast — not from a pre-committed plan to always draw k.
//!
//! The two mechanisms, both pure and std-only:
//!
//! - [`Quorum`] — sequential majority voting with **provably lossless early
//!   adjournment**: it stops the moment the leading answer cannot be overtaken by
//!   the ballots remaining, and its decision is *identical* to running all k
//!   ballots and taking the majority. This is not a heuristic; the equivalence is
//!   exhaustively tested below. Expected saving at k=5 with a unanimous model:
//!   adjournment after 3 — the same answer for 60% of the charge.
//!
//! - [`scry_then_convene`] — the two-tier allocation Carroll's mid-band law asks
//!   for (see [`kaos_pact::equation::probability_shift`]): probe with a small number
//!   of charges; if they **agree**, the task is likely at the ceiling where the
//!   conclave has nothing to rescue — ship the answer; if they **disagree**, the
//!   task has revealed itself mid-band, exactly where best-of-k pays — convene the
//!   full (adjourning) quorum. Unlike the pure quorum this *is* a trade: on tasks
//!   where two samples agree on a wrong answer, it ships the wrong answer cheaper.
//!   The benchmark measures that honestly rather than assuming it away.

use std::collections::BTreeMap;

/// The modal vote — ties broken by first appearance. The single canonical
/// majority used everywhere in kaos (the benchmarks import this one).
pub fn majority(votes: &[String]) -> Option<String> {
    let mut best: Option<(&str, usize)> = None;
    for v in votes {
        let count = votes.iter().filter(|x| *x == v).count();
        match best {
            Some((_, bc)) if bc >= count => {}
            _ => best = Some((v.as_str(), count)),
        }
    }
    best.map(|(v, _)| v.to_string())
}

/// A sequential quorum of `k` ballots that adjourns as soon as its decision is
/// mathematically settled.
///
/// Cast ballots one at a time with [`Quorum::cast`]; a ballot may be `None` (the
/// charge fizzled — no extractable answer), which spends the ballot without
/// voting. After each cast, [`Quorum::adjourned`] reports whether the leader is
/// strictly unbeatable — its count exceeds the best rival's count plus **all**
/// ballots yet uncast — at which point the decision is final and further charges
/// are pure waste.
#[derive(Clone, Debug)]
pub struct Quorum {
    k: usize,
    cast: usize,
    votes: Vec<String>,
}

impl Quorum {
    pub fn new(k: usize) -> Quorum {
        Quorum {
            k: k.max(1),
            cast: 0,
            votes: Vec::new(),
        }
    }

    /// Spend one ballot. Returns `true` if the quorum is now adjourned (settled
    /// early or all ballots spent) — the caller should stop drawing samples.
    pub fn cast(&mut self, vote: Option<String>) -> bool {
        if self.settled() {
            return true; // already adjourned; the ballot is refused, not spent
        }
        self.cast += 1;
        if let Some(v) = vote {
            self.votes.push(v);
        }
        self.settled()
    }

    /// Ballots actually spent (samples drawn).
    pub fn ballots_cast(&self) -> usize {
        self.cast
    }

    /// Ballots remaining in the writ of k.
    pub fn remaining(&self) -> usize {
        self.k - self.cast
    }

    /// The current decision — the modal vote among ballots cast so far.
    pub fn decision(&self) -> Option<String> {
        majority(&self.votes)
    }

    /// Is the quorum done — either settled beyond overturning, or out of ballots?
    pub fn adjourned(&self) -> bool {
        self.settled()
    }

    /// Settled = out of ballots, or the leader strictly unbeatable: even if every
    /// remaining ballot went to the best rival, the leader still wins outright.
    /// Strictness matters: a reachable *tie* could flip the first-appearance
    /// tie-break, so a tie must never be treated as settled.
    fn settled(&self) -> bool {
        if self.cast >= self.k {
            return true;
        }
        let counts = self.tally();
        let mut sorted: Vec<usize> = counts.values().copied().collect();
        sorted.sort_unstable_by(|a, b| b.cmp(a));
        let leader = sorted.first().copied().unwrap_or(0);
        let rival = sorted.get(1).copied().unwrap_or(0);
        leader > rival + self.remaining()
    }

    fn tally(&self) -> BTreeMap<&str, usize> {
        let mut t = BTreeMap::new();
        for v in &self.votes {
            *t.entry(v.as_str()).or_insert(0) += 1;
        }
        t
    }
}

/// Replay a recorded sequence of up to `k` vote-keys through an adjourning
/// [`Quorum`], returning `(decision, ballots_spent)`. Because adjournment is
/// lossless, `decision` always equals the full-k majority of the same sequence —
/// this is how the benchmark measures the saving at zero extra inference.
pub fn adjourned_vote(votes: &[Option<String>], k: usize) -> (Option<String>, usize) {
    let mut q = Quorum::new(k);
    for v in votes.iter().take(k) {
        if q.cast(v.clone()) {
            break;
        }
    }
    (q.decision(), q.ballots_cast())
}

/// The two-tier scry: probe with `probe` charges; if every probe ballot names the
/// same answer, ship it (the task looks to be at the ceiling — nothing for a
/// conclave to rescue). On any disagreement or fizzle, convene the full adjourning
/// quorum over all k. Returns `(decision, ballots_spent)`.
///
/// This is the aggressive tier: unlike [`adjourned_vote`] it can differ from the
/// full majority (two agreeing probes can both be wrong). Carroll's second
/// equation says that regime is rare when the model is near its ceiling — and the
/// benchmark measures the actual cost instead of trusting the theory.
pub fn scry_then_convene(
    votes: &[Option<String>],
    probe: usize,
    k: usize,
) -> (Option<String>, usize) {
    let probe = probe.max(1).min(k);
    let head = &votes[..probe.min(votes.len())];
    let first = head.first().and_then(|v| v.clone());
    let unanimous = first.is_some()
        && head.len() == probe
        && head.iter().all(|v| v.as_deref() == first.as_deref());
    if unanimous {
        return (first, probe);
    }
    adjourned_vote(votes, k)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(x: &str) -> Option<String> {
        Some(x.to_string())
    }

    #[test]
    fn unanimous_quorum_adjourns_at_majority() {
        // k=5, three agreeing ballots: leader 3 > rival 0 + remaining 2 → adjourn.
        let (decision, spent) = adjourned_vote(&[s("88"), s("88"), s("88"), s("67"), s("88")], 5);
        assert_eq!(decision.as_deref(), Some("88"));
        assert_eq!(spent, 3, "the last two charges are never fired");
    }

    #[test]
    fn split_vote_goes_the_distance() {
        let (decision, spent) = adjourned_vote(&[s("a"), s("b"), s("a"), s("b"), s("a")], 5);
        assert_eq!(decision.as_deref(), Some("a"));
        assert_eq!(spent, 5, "a contested vote spends every ballot");
    }

    #[test]
    fn a_reachable_tie_is_never_settled() {
        // After a,a,b with 2 remaining, b can reach 3-3; the tie-break could then
        // matter, so the quorum must keep sitting.
        let mut q = Quorum::new(5);
        assert!(!q.cast(s("a")));
        assert!(!q.cast(s("a")));
        assert!(!q.cast(s("b")));
        assert!(!q.adjourned());
    }

    #[test]
    fn fizzled_ballots_spend_without_voting() {
        let (decision, spent) = adjourned_vote(&[None, s("x"), None, s("x"), s("x")], 5);
        assert_eq!(decision.as_deref(), Some("x"));
        assert_eq!(spent, 4); // after N,x,N,x the lone rival ballot cannot catch x
        let (d2, sp2) = adjourned_vote(&[s("x"), s("x"), s("x"), None, None], 5);
        assert_eq!(d2.as_deref(), Some("x"));
        assert_eq!(sp2, 3);
    }

    #[test]
    fn adjournment_is_lossless_exhaustively() {
        // THE property: over every possible sequence of 5 ballots drawn from
        // {a, b, fizzle} (3^5 = 243 sequences), the adjourned decision equals the
        // full-k majority, and never spends more than k ballots.
        let alphabet = [s("a"), s("b"), None];
        for code in 0..3usize.pow(5) {
            let mut seq = Vec::with_capacity(5);
            let mut c = code;
            for _ in 0..5 {
                seq.push(alphabet[c % 3].clone());
                c /= 3;
            }
            let full: Vec<String> = seq.iter().filter_map(|v| v.clone()).collect();
            let want = majority(&full);
            let (got, spent) = adjourned_vote(&seq, 5);
            assert_eq!(got, want, "sequence {seq:?} diverged");
            assert!(spent <= 5);
        }
    }

    #[test]
    fn majority_breaks_ties_by_first_appearance() {
        assert_eq!(
            majority(&["b".into(), "a".into(), "a".into(), "b".into()]).as_deref(),
            Some("b")
        );
        assert_eq!(majority(&[]), None);
    }

    #[test]
    fn scry_ships_on_agreeing_probes() {
        let (d, spent) = scry_then_convene(&[s("42"), s("42"), s("7"), s("7"), s("7")], 2, 5);
        assert_eq!(
            d.as_deref(),
            Some("42"),
            "agreeing probes ship without a conclave"
        );
        assert_eq!(spent, 2);
    }

    #[test]
    fn scry_escalates_on_disagreement() {
        let (d, spent) = scry_then_convene(&[s("41"), s("42"), s("42"), s("42"), s("7")], 2, 5);
        assert_eq!(d.as_deref(), Some("42"));
        assert_eq!(spent, 4, "escalates, then the quorum adjourns when settled");
    }

    #[test]
    fn scry_escalates_on_a_fizzled_probe() {
        let (d, spent) = scry_then_convene(&[None, None, s("9"), s("9"), s("9")], 2, 5);
        assert_eq!(d.as_deref(), Some("9"));
        assert_eq!(
            spent, 4,
            "fizzles are disagreement; the quorum convenes, then adjourns"
        );
    }
}
