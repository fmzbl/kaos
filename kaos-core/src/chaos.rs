//! Chaos mode — the stance where the work is done by a program, not by an agent
//! that was told what to do.
//!
//! Until now `chaos` was a flag on one surface: `kaos rebis run --chaos` decided
//! whether a Rebis model node got the unscoped Kaos pipeline or a single scoped
//! node agent. That is a real distinction, but it is a *setting on runs*, and it
//! left the rest of the harness with no chaos at all. A chat had no chaos mode.
//! A sigil stack had no chaos mode. The one place the word appeared was the one
//! place a program already existed.
//!
//! # What chaos mode is
//!
//! **Normal mode: an agent is handed an intent and works it.**
//! **Chaos mode: the intent is first written as a Rebis program, and the program
//! is what runs.**
//!
//! One sentence, and it means the same thing on every surface:
//!
//! | surface        | normal                          | chaos                                        |
//! |----------------|---------------------------------|----------------------------------------------|
//! | chat           | one tool agent answers the turn | the turn is composed into a program, then run |
//! | Rebis run      | one scoped agent per node       | each node gets the whole Kaos pipeline        |
//! | sigil stack    | the generated program, scoped   | the generated program, unscoped              |
//!
//! The Rebis-run column is the behaviour that already shipped, unchanged. What
//! is new is that the same word now has a meaning for the surfaces that had
//! none, and one toggle reaches all of them.
//!
//! # Why compose first
//!
//! Because it is the only step that makes the work *countable* before it is
//! paid for. A chat agent's cost is discovered by running it. A Rebis program's
//! cost can be read off the page — that is the language's central claim, and the
//! `Cost:` tests in the standard library are what keep it true. Composing first
//! turns "answer this" into an artifact you can read, price, edit, save to the
//! wall, and run again. An agent's transcript is none of those things.
//!
//! It is also the honest reading of the book the project is named after. Carroll
//! (*Liber Null*, "Sigils"): the operation has three parts — the sigil is
//! **constructed**, it is **lost to the mind**, it is **charged**. The desire is
//! stated in words and then deliberately destroyed into a form that no longer
//! reads as the desire, because "the will becomes involved in a dialogue with
//! the mind" and that dialogue is what dilutes it. Composition is construction:
//! the sentence stops being a sentence and becomes structure. Running is
//! charging. Chaos mode is that operation; normal mode is talking to the mind.
//!
//! # Why it is off by default
//!
//! Composition costs a model call before any work happens, and for "what does
//! this function do" that call buys nothing. Chaos mode earns its price on work
//! with shape — several steps, a gate, a fan-out, something worth keeping. The
//! toggle is deliberately a stance the operator adopts, not a heuristic that
//! guesses.

/// The environment variable every Kaos surface reads and every child inherits.
///
/// A single name, because the toggle has a single meaning. The terminal app
/// exports it to the jobs it spawns, the visual editor exports it to runs, and
/// `kaos code --chaos` sets it for a one-shot invocation — so a chaos stance
/// adopted anywhere reaches the whole process tree without a second flag.
pub const ENABLE_ENV: &str = "KAOS_CHAOS";

/// Whether chaos mode is on for this process.
///
/// Reads the environment first so a `--chaos` flag or a parent's export wins,
/// then the config file, so the stance can be made durable in `/config` without
/// being unsettable for one run.
#[must_use]
pub fn enabled() -> bool {
    match std::env::var(ENABLE_ENV) {
        Ok(raw) => truthy(&raw),
        Err(_) => crate::config::value(ENABLE_ENV).is_some_and(|raw| truthy(&raw)),
    }
}

/// `1`, `true`, `yes`, `on` — anything else, including empty, is off.
///
/// Empty reads as OFF rather than as "unset". A surface that exports the
/// variable unconditionally (`.env(ENABLE_ENV, if on {"1"} else {"0"})`) is the
/// pattern that keeps a stale parent export from leaking into a child that was
/// launched with chaos off, and that pattern only works if `0` and `""` both
/// mean off.
#[must_use]
pub fn truthy(raw: &str) -> bool {
    matches!(
        raw.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// The value to export to a child for a given stance.
///
/// Always export, never conditionally set: see [`truthy`].
#[must_use]
pub const fn export(on: bool) -> &'static str {
    if on {
        "1"
    } else {
        "0"
    }
}

/// One word for the stance, for status lines and run records.
#[must_use]
pub const fn label(on: bool) -> &'static str {
    if on {
        "chaos"
    } else {
        "direct"
    }
}

/// One sentence explaining the stance, for a tooltip or a help line.
///
/// Kept here rather than in each frontend so the terminal app, the visual
/// editor and `--help` cannot drift into describing the same toggle three
/// different ways.
pub const CHAOS_HINT: &str = "Chaos mode: the intent is written as a Rebis program first, and the \
program is what runs — so its cost can be read before it is paid, and the result is something you \
can edit, save to the wall, and run again. Off, an agent is handed the intent and works it.";

/// The contract appended to a chat that is being run in chaos mode.
///
/// It asks for one thing — a program — and states the two rules that make the
/// result worth having: the program must parse, and its cost must be readable.
/// Everything else is deliberately absent. A longer contract would start
/// specifying the *shape* of the program, and the point of composing is that the
/// shape is the model's to find.
///
/// The fenced block is required, not encouraged, because the host has to
/// extract it: [`extract_program`] reads the fence and nothing else.
pub const COMPOSE_CONTRACT: &str = "\
CHAOS MODE

Do not do the work yet. First write the work as a Rebis program.

Return a single ```rebis fenced block containing one complete, parseable \
program, and nothing else outside it except at most two sentences saying what \
the program costs. Every model call the program makes must be visible as a \
written prompt in the source, because the reader prices the program by counting \
them. Prefer the standard library over inventing a macro. If the request needs \
no model call at all, say so and write the program anyway — a program that \
costs nothing is the best possible answer.";

/// The header a chaos chat carries instead of the ordinary conversational one.
///
/// Kept separate from [`COMPOSE_CONTRACT`] so a host that also puts the
/// contract in a system prompt is not writing it twice in one request.
pub const COMPOSE_TURN_HEADER: &str = "\
You are the Kaos composer. The user has stated an intent below. Write it as a \
Rebis program under the chaos-mode contract that follows.";

/// The single fenced Rebis program in a composer's reply, if there is one.
///
/// Returns the block's contents with no trailing newline fuss, or `None` when
/// the reply has no ```rebis fence. Deliberately strict about the language tag:
/// a model that returns a bare ``` block has not followed the contract, and
/// guessing that its prose is a program is how a host ends up running an
/// apology.
#[must_use]
pub fn extract_program(reply: &str) -> Option<String> {
    let start = reply.find("```rebis")?;
    let after = &reply[start + "```rebis".len()..];
    // Skip the rest of the fence line (a language tag may be followed by
    // attributes, and the newline is not part of the program).
    let body = after.split_once('\n')?.1;
    let end = body.find("```")?;
    let program = body[..end].trim_end();
    (!program.trim().is_empty()).then(|| program.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The stance is read from one name, and `0` means off.
    ///
    /// The unconditional-export pattern depends on this: a child launched with
    /// chaos off is launched with `KAOS_CHAOS=0`, not with the variable absent,
    /// so a parent that had it on cannot leak the stance downward.
    #[test]
    fn zero_and_empty_are_off_so_an_export_can_always_be_unconditional() {
        assert!(truthy("1"));
        assert!(truthy("true"));
        assert!(truthy("YES"));
        assert!(truthy(" on "));
        assert!(!truthy("0"));
        assert!(!truthy(""));
        assert!(!truthy("off"));
        assert!(!truthy("maybe"));
        assert_eq!(export(true), "1");
        assert_eq!(export(false), "0");
        assert!(truthy(export(true)));
        assert!(!truthy(export(false)));
    }

    #[test]
    fn the_stance_has_a_word_for_each_side() {
        assert_eq!(label(true), "chaos");
        assert_eq!(label(false), "direct");
    }

    /// The host runs what is inside the fence, so the fence has to be found
    /// exactly — including when the model wraps it in the explanation the
    /// contract asks for.
    #[test]
    fn the_program_is_taken_from_the_fence_and_only_from_the_fence() {
        let reply =
            "Here is the plan.\n\n```rebis\n(? \"do the thing\")\n```\n\nIt costs one call.";
        assert_eq!(
            extract_program(reply).as_deref(),
            Some("(? \"do the thing\")")
        );
    }

    /// A reply that did not follow the contract is not guessed at.
    ///
    /// The failure this prevents is specific and bad: a model that refuses, or
    /// apologises, or answers in prose gets its prose handed to the parser, and
    /// the user sees a syntax error instead of the refusal. Returning `None`
    /// lets the host say what actually happened.
    #[test]
    fn prose_is_never_mistaken_for_a_program() {
        assert!(extract_program("I cannot help with that.").is_none());
        assert!(extract_program("```\n(? \"untagged\")\n```").is_none());
        assert!(extract_program("```rebis\n\n```").is_none());
        assert!(extract_program("```rebis\n(? \"unclosed\")").is_none());
    }

    /// The contract must say the two things the host depends on, or the
    /// extraction above has nothing to extract.
    #[test]
    fn the_contract_states_the_fence_and_the_cost_rule() {
        assert!(COMPOSE_CONTRACT.contains("```rebis"));
        assert!(COMPOSE_CONTRACT.contains("cost"));
        assert!(COMPOSE_TURN_HEADER.contains("Rebis program"));
    }
}
