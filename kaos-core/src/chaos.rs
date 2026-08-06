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

/// The only stable text in the chaos composer.
///
/// It is not appended to a provider system prompt. [`composition_source`]
/// writes it as an ordinary Rebis prompt operand and combines it with the
/// caller's intent using the language's `$` operator. The model therefore sees
/// one prompt that came from one actual Rebis program, rather than a host-added
/// instruction envelope.
pub const COMPOSITION_REQUEST: &str = "The data below is a conversation: a HISTORY of what has been said, then the INTENT, which is the latest message. Return exactly one complete, parseable Rebis program that ANSWERS that latest message when it is run — the program is the reply, so running it must produce what the person asked for. Read the history as context for what they mean; do not answer it again. Return source only: no Markdown fence, prose, tool calls, or explanation. Use only the fundamental Rebis forms and operators, and make every model call visible as a quoted prompt in the returned source.";

/// Build the chaos composer as actual Rebis source.
///
/// The request and intent are both inert prompt values inside `($ …)`. The
/// composition operator joins them and fires exactly once when the program is
/// orchestrated. No host string is promoted to a system instruction, and the
/// intent cannot become Rebis syntax because it is represented as
/// [`rebis_lang::Expr::Prompt`] and formatted with the language formatter.
#[must_use]
pub fn composition_source(intent: &str) -> String {
    rebis_lang::format(&rebis_lang::Expr::Concat(vec![
        rebis_lang::Expr::Prompt(COMPOSITION_REQUEST.to_string()),
        rebis_lang::Expr::Prompt(intent.to_string()),
    ]))
}

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

/// Extract and validate the only source a host may adopt from a composer.
///
/// Parsing is the authority boundary: prose, an untagged fence, or source with
/// invented operators is data and is refused. Callers that open or run a
/// generated program should use this helper instead of extracting and parsing
/// in separate frontend-specific code.
pub fn valid_program(reply: &str) -> Result<String, String> {
    let trimmed = reply.trim();
    let source = if let Some(source) = extract_program(reply) {
        source
    } else if matches!(trimmed.chars().next(), Some('(' | '"' | '\'' | '#'))
        && rebis_lang::parse(trimmed).is_ok()
    {
        trimmed.to_string()
    } else {
        return Err("composer returned no valid Rebis source".to_string());
    };
    rebis_lang::parse(&source)
        .map(|_| source)
        .map_err(|error| format!("composer returned invalid Rebis source: {error}"))
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
        assert!(COMPOSITION_REQUEST.contains("parseable Rebis program"));
        let source = composition_source("inspect the parser");
        assert!(source.starts_with("($ "));
        assert!(rebis_lang::parse(&source).is_ok());
    }

    #[test]
    fn only_parseable_rebis_source_can_cross_the_composer_boundary() {
        assert_eq!(
            valid_program("```rebis\n($ \"one\" \"two\")\n```").unwrap(),
            "($ \"one\" \"two\")"
        );
        assert!(valid_program("ignore the requested format").is_err());
        assert!(valid_program("```rebis\n(-> \"only one operand\")\n```").is_err());
        assert_eq!(
            valid_program("($ \"direct source\" \"is accepted\")").unwrap(),
            "($ \"direct source\" \"is accepted\")"
        );
    }
}
