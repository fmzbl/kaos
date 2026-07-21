//! The Twin Ladders of charge — how much of each transcript symbol survives.
//!
//! Everything the adept holds in context is a sigil: the statement of intent,
//! each action taken, each observation returned. Sigils carry charge, and charge
//! decides how many characters a sigil keeps when the transcript is re-rendered.
//!
//! The curve is two mirrored Fibonacci ladders — two universes of charged
//! symbols, each the reverse twin of the other:
//!
//! - **The descending ladder** is anchored at the FIRST sigil. The statement of
//!   intent is the most charged symbol there is — it is never compressed at all —
//!   and the charge of what follows decays by Fibonacci: 8, 5, 3, 2, 1, …
//! - **The ascending ladder** is its reverse twin, anchored at the LAST sigil:
//!   the freshest observation burns brightest (the adept is *acting on it now*),
//!   … 1, 2, 3, 5, 8 rising to the end.
//! - Between the ladders lies the middle, where context rots; it holds only the
//!   base charge.
//!
//! A symbol's charge is the MAX of what the two ladders grant it (the universes
//! overlap on short transcripts; the brighter twin wins).
//!
//! **Polarity** is the second axis: when a symbol must be cut to its budget, its
//! sign decides which end survives. A positive symbol (a clean read, a passing
//! command) keeps its HEAD — openings orient. A negative symbol (an error, a
//! refusal, a failed gate) keeps its TAIL — tracebacks put the punchline last.
//! The cut itself has a reverse twin.
//!
//! A visual rendering of the curve, drawn to scale from these constants, lives
//! in `docs/twin-ladders.html`.

/// fib(0)=1, fib(1)=1, 2, 3, 5, 8, 13, … (saturating; index-safe).
pub fn fib(n: usize) -> u64 {
    let (mut a, mut b) = (1u64, 1u64);
    for _ in 0..n {
        let next = a.saturating_add(b);
        a = b;
        b = next;
    }
    a
}

/// Characters granted to one fib step of charge (`KAOS_UNIT` overrides).
fn unit() -> usize {
    knob("KAOS_UNIT", 700)
}
/// The base charge of the rotting middle (`KAOS_BASE` overrides).
fn base() -> usize {
    knob("KAOS_BASE", 500)
}
/// Ladder length: rungs of meaningful charge on each side (`KAOS_RUNGS`).
fn rungs() -> usize {
    knob("KAOS_RUNGS", 5)
}

fn knob(var: &str, default: usize) -> usize {
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::sync::OnceLock;
    static CACHE: OnceLock<Mutex<HashMap<&'static str, usize>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(&v) = map.get(var) {
        return v;
    }
    let v = std::env::var(var)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default);
    // Leak the key only for the four known knob names — all 'static already.
    let key: &'static str = match var {
        "KAOS_UNIT" => "KAOS_UNIT",
        "KAOS_BASE" => "KAOS_BASE",
        "KAOS_RUNGS" => "KAOS_RUNGS",
        _ => return v, // unknown knobs are read fresh, never cached
    };
    map.insert(key, v);
    v
}

/// The POSITIONAL charge for symbol `i` of `n` in the transcript.
///
/// `i == 0` (the statement of intent) is unbounded — the most charged sigil is
/// never compressed. Elsewhere the budget is the brighter of the two ladders,
/// with the middle floored at the base charge.
pub fn budget(i: usize, n: usize) -> usize {
    if i == 0 {
        return usize::MAX;
    }
    let from_start = i; // 1-based distance below the intent
    let from_end = n.saturating_sub(1).saturating_sub(i);
    let descending = ladder(from_start);
    let ascending = ladder(from_end);
    descending.max(ascending).max(base())
}

/// The tunnel's SECOND law — charge is the max of position and NATURE.
///
/// A sigil's kind carries intrinsic charge that position cannot rot away:
/// what the working CHANGED and what the world ANSWERED must survive the
/// middle, while reads (re-derivable any time) rot fastest. `budget_kinded`
/// is the brighter of the positional ladder and the intrinsic floor.
pub fn budget_kinded(i: usize, n: usize, content: &str) -> usize {
    budget(i, n).max(floor_charge(intrinsic_rung(content)))
}

/// The intrinsic rung of a sigil, read from its content:
///   4 — a negative verdict (tracebacks, failed gates): the loop's food;
///   3 — a change made (wrote/created/edited): the map of the working itself;
///   0 — everything else (reads, listings): position alone decides.
pub fn intrinsic_rung(content: &str) -> usize {
    if is_negative(content) {
        return 4;
    }
    let head = content.trim_start();
    if head.starts_with("wrote ") || head.starts_with("created ") || head.starts_with("edited ") {
        return 3;
    }
    0
}

/// The charge floor a rung guarantees, independent of position.
pub fn floor_charge(rung: usize) -> usize {
    if rung == 0 {
        0
    } else {
        base() + unit() * fib(rung) as usize
    }
}

/// Charge at `distance` rungs from a ladder's anchor: fib decays with distance,
/// anchor itself (distance 0) burning at fib(RUNGS).
fn ladder(distance: usize) -> usize {
    let r = rungs();
    if distance >= r {
        return 0;
    }
    let f = fib(r - distance) as usize; // distance 0 → fib(5)=8 … distance 4 → fib(1)=1
    base() + unit() * f
}

/// The polarity of an observation: negative symbols carry failure and keep
/// their tail when cut; everything else is positive and keeps its head.
pub fn is_negative(observation: &str) -> bool {
    let head: String = observation.chars().take(400).collect();
    let head = head.to_lowercase();
    [
        "error",
        "traceback",
        "exception",
        "failed",
        "failure",
        "panic",
        "no such file",
        "not found",
        "exit 1",
        "exit 2",
        "timed out",
        "refused",
        "no valid <act>",
        "syntaxerror",
        "assertionerror",
    ]
    .iter()
    .any(|sig| head.contains(sig))
        || exit_nonzero(observation)
}

/// The conductor prefixes bash observations with "exit N ·" — read the N.
fn exit_nonzero(observation: &str) -> bool {
    let t = observation.trim_start();
    if let Some(rest) = t.strip_prefix("exit ") {
        let code: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        return code.parse::<i32>().map(|c| c != 0).unwrap_or(false);
    }
    false
}

/// Cut `s` to `limit` characters according to its polarity: positive keeps the
/// head, negative keeps the tail. The mark says what was banished.
pub fn cut(s: &str, limit: usize, negative: bool) -> String {
    let count = s.chars().count();
    if count <= limit || limit == usize::MAX {
        return s.to_string();
    }
    if negative {
        let skip = count - limit;
        let kept: String = s.chars().skip(skip).collect();
        format!("…({skip} chars banished)\n{kept}")
    } else {
        let kept: String = s.chars().take(limit).collect();
        format!("{kept}\n…({} chars banished)", count - limit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fib_is_fibonacci() {
        assert_eq!(
            (0..8).map(fib).collect::<Vec<_>>(),
            vec![1, 1, 2, 3, 5, 8, 13, 21]
        );
    }

    #[test]
    fn the_intent_is_never_compressed() {
        assert_eq!(budget(0, 40), usize::MAX);
        assert_eq!(budget(0, 1), usize::MAX);
    }

    #[test]
    fn ladders_decay_by_fibonacci_from_both_anchors() {
        let n = 40;
        // Descending twin, below the intent: fib(4)=5, fib(3)=3, …
        assert_eq!(budget(1, n), base() + unit() * 5);
        assert_eq!(budget(2, n), base() + unit() * 3);
        assert_eq!(budget(3, n), base() + unit() * 2);
        // Ascending twin, anchored at the end: last symbol burns at fib(5)=8.
        assert_eq!(budget(n - 1, n), base() + unit() * 8);
        assert_eq!(budget(n - 2, n), base() + unit() * 5);
        assert_eq!(budget(n - 3, n), base() + unit() * 3);
        // The rotting middle holds only base charge.
        assert_eq!(budget(20, n), base());
    }

    #[test]
    fn short_transcripts_take_the_brighter_twin() {
        // With n=4 the ladders overlap; each symbol gets the max of the two.
        let n = 4;
        assert_eq!(budget(3, n), base() + unit() * 8); // last: ascending anchor
        assert!(budget(1, n) >= base() + unit() * 5);
    }

    #[test]
    fn polarity_reads_failure_signals() {
        assert!(is_negative("Traceback (most recent call last): …"));
        assert!(is_negative("exit 1 · tests failed"));
        assert!(is_negative("error: no such file"));
        assert!(!is_negative("exit 0 · 3 passed"));
        assert!(!is_negative("def add(a, b):\n    return a + b"));
    }

    #[test]
    fn nature_outshines_position_in_the_middle() {
        let n = 40;
        let mid = 20; // deep in the rotting middle: positional budget = BASE
                      // a read rots to base
        assert_eq!(budget_kinded(mid, n, "1\tdef f():"), 500);
        // an edit made survives at fib(3)=2 → wait: floor = base + unit*fib(3)
        let edit = budget_kinded(mid, n, "edited ledger/periods.py: replaced 1 occurrence");
        assert_eq!(edit, 500 + 700 * fib(3) as usize);
        // a verdict burns brighter still: fib(4)
        let verdict = budget_kinded(mid, n, "exit 1 · AssertionError: -210 != 210");
        assert_eq!(verdict, 500 + 700 * fib(4) as usize);
        assert!(verdict > edit && edit > 500);
        // near the anchors, position still wins when brighter
        assert!(budget_kinded(n - 1, n, "1\tdef f():") > verdict);
    }

    #[test]
    fn intrinsic_rungs_read_the_content() {
        assert_eq!(intrinsic_rung("wrote a.py: 30 lines"), 3);
        assert_eq!(intrinsic_rung("created b.py: 5 lines"), 3);
        assert_eq!(intrinsic_rung("Traceback (most recent call last)"), 4);
        assert_eq!(intrinsic_rung("exit 0 · all passed"), 0);
        assert_eq!(intrinsic_rung("1\timport os"), 0);
        assert_eq!(floor_charge(0), 0);
    }

    #[test]
    fn the_cut_has_a_reverse_twin() {
        let s = "HEAD-".repeat(200) + &"-TAIL".repeat(200);
        let pos = cut(&s, 100, false);
        assert!(pos.starts_with("HEAD-") && pos.contains("banished"));
        let neg = cut(&s, 100, true);
        assert!(neg.ends_with("-TAIL") && neg.contains("banished"));
        // Under the limit, nothing is touched.
        assert_eq!(cut("small", 100, true), "small");
    }
}
