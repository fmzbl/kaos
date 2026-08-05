//! The myth syntax, and the Rebis it becomes.
//!
//! Kaos used to have two orchestration languages. `myth` was 518 lines with five
//! forms and four gates; Rebis is the model-interface language the rest of the
//! harness is written against, with 22 frozen operators and a tested cost model.
//! Two languages was the shape of an accident rather than a design.
//!
//! **The evaluator is gone.** What is left is a reader and a translator, so a
//! `KAOS_MYTH` a user wrote a year ago still runs — as the Rebis program it
//! always meant — and says what to write instead. `to_rebis` is the whole
//! remaining point of this file, and the file goes with the syntax a release
//! from now.
//!
//! The grammar it still reads:
//!
//! ```lisp
//! fire                 ; a model call
//! (ask "role")         ; a model call with an instruction  (a stage's job)
//! (spread N X)         ; diverge  — run X, N ways  (fan out)
//! (gather G X)         ; converge — collapse X's candidates through gate G
//! (pipe A B …)         ; sequence — each stage's answer feeds the next
//! ; G ::= vote | first | (check "shell-cmd") | (mirror P)
//! ```
//!
//! # What survived, and where it went
//!
//! - The **gates** are `kaos_agent::gate`: `[vote]`, `[first]` and `[check]` are
//!   host mediators Rebis resolves by name. Getting them there is what made
//!   retiring the evaluator possible — `tests/myth_equivalence.rs` found that
//!   three of the four had no Rebis spelling at all, not the one the plan
//!   predicted.
//! - The **[`Cast`] trait survived as the model seam.** It is what a leaf IS —
//!   one completion, or a whole tool session in an isolated copy of the tree —
//!   and `solve::RebisConclave` is generic over it. So the graph is Rebis and
//!   the leaf is still a trait, which is the split that was worth keeping.
//!
//! # What did not translate
//!
//! Recorded in `tests/myth_equivalence.rs` rather than here, because they are
//! assertions rather than prose: the bowtie (a nested square's mediator is shown
//! every leaf firing, not the collapsed inner results), `first`'s determinism,
//! and a vote with no majority.

/// The seam to the chat: "a chat you can fire". `Sync` so a `spread` fans out
/// concurrently. `check` gates a single candidate through a shell verifier.
pub trait Cast: Sync {
    fn fire(&self, task: &str, i: usize) -> Option<String>;
    /// Verify one candidate against `cmd`; default: no gate available (fails).
    fn check(&self, _task: &str, _candidate: &str, _cmd: &str) -> bool {
        false
    }
}

/// How a `gather` collapses candidates to one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Gate {
    /// Self-consistency: the modal candidate.
    Vote,
    /// The first non-empty candidate.
    First,
    /// Keep candidates that pass the shell verifier; take the first survivor.
    Check(String),
    /// Round-trip fidelity: reflect each candidate back to the question it
    /// answers (one extra model call per candidate) and keep those whose
    /// holonomy against the task is at most P percent — picking the lowest.
    /// An answer that cannot compress back to the question is drift, and this
    /// gate refuses it. See `docs/REBIS.md`.
    Mirror(u8),
}

/// The myth graph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node {
    /// One model call, generic.
    Fire,
    /// One model call with a role/instruction prepended (the workhorse of a
    /// multi-stage myth: "propose", "critique", "write the final code").
    Ask(String),
    /// Diverge: evaluate the subgraph `n` ways (concurrently).
    Spread(usize, Box<Node>),
    /// Converge: collapse the subgraph's candidates through the gate.
    Gather(Gate, Box<Node>),
    /// Sequence: run each stage in turn; the collapsed answer of one stage feeds
    /// the next as context. This is what makes a myth an agent *pipeline*.
    Pipe(Vec<Node>),
}

impl Node {
    /// How many leaf calls this myth issues when evaluated — `spread` multiplies,
    /// `pipe` sums, `gather` passes through. The cost exposure of a run: multiply by
    /// the per-session step budget (agentic) to bound the model calls a run can make.
    pub fn leaves(&self) -> usize {
        match self {
            Node::Fire | Node::Ask(_) => 1,
            Node::Spread(n, child) => n * child.leaves(),
            // the mirror gate reflects deterministically: no extra calls
            Node::Gather(Gate::Mirror(_), child) => child.leaves(),
            Node::Gather(_, child) => child.leaves(),
            Node::Pipe(stages) => stages.iter().map(|s| s.leaves()).sum(),
        }
    }
}

// ── the way out: a myth, written as Rebis ───────────────────────────────────

/// The name a translated myth binds the task to.
///
/// A myth is a *template* — `run(node, task, cast)` supplies the task from
/// outside. A Rebis program has no outside: the program is the prompt. So the
/// translation obtains the task once and names it, and every leaf composes it.
const TASK: &str = "task";

/// What a translated myth loses, if anything.
///
/// Returned beside the source rather than printed, because the caller decides
/// whether a lossy translation is a notice, a refusal, or acceptable.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Losses(pub Vec<String>);

impl Losses {
    /// Whether the translation is faithful.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Write a myth as the Rebis program that means the same thing.
///
/// This is the shim that lets `myth` retire. The mapping is the one
/// `tests/myth_equivalence.rs` established, and the two gates it could not
/// establish are reported in [`Losses`] rather than silently mistranslated:
///
/// | myth | Rebis |
/// |---|---|
/// | `fire` | `($ task)` — compose the task and fire it |
/// | `(ask "role")` | `(+ "role" ($ task))` — framing reaches the prompt |
/// | `(gather G (spread N X))` | `([g] X X … X)`, N branches written out |
/// | `(pipe A B …)` | `(-> A B …)` |
/// | `vote` `first` | `[vote]` `[first]` — host mediators, see [`crate::gate`] |
/// | `(check "cmd")` | `[check]` — **the command is lost**, see below |
/// | `(mirror P)` | `[mirror]` — **the threshold is lost** |
///
/// **The spread width becomes a written count.** `(spread 8 fire)` becomes eight
/// branches on the page. That is the point rather than a cost of the
/// translation: the price of a Rebis program is countable by reading it, and a
/// width hidden behind a variable is exactly what makes a myth's price not be.
///
/// # Errors
///
/// Returns the reason when the myth has no Rebis form at all.
pub fn to_rebis(node: &Node) -> Result<(String, Losses), String> {
    let mut losses = Losses::default();
    let body = write_node(node, &mut losses)?;
    Ok((format!("(= {TASK} (&) {body})"), losses))
}

/// One myth node as Rebis, without the task binding around it.
fn write_node(node: &Node, losses: &mut Losses) -> Result<String, String> {
    match node {
        Node::Fire => Ok(format!("($ {TASK})")),
        Node::Ask(instruction) => Ok(format!("(+ \"{}\" ($ {TASK}))", escape_prompt(instruction))),
        // A bare spread has no gate of its own; `run` applies an implicit final
        // vote, so the translation writes the vote that was always there.
        Node::Spread(width, child) => {
            let branch = write_node(child, losses)?;
            Ok(square("vote", &branch, *width))
        }
        Node::Gather(gate, child) => {
            let name = gate_name(gate, losses);
            match child.as_ref() {
                Node::Spread(width, inner) => {
                    let branch = write_node(inner, losses)?;
                    Ok(square(name, &branch, *width))
                }
                other => {
                    let branch = write_node(other, losses)?;
                    Ok(square(name, &branch, 1))
                }
            }
        }
        Node::Pipe(stages) => {
            if stages.is_empty() {
                return Err("a pipe with no stages has no Rebis form".to_string());
            }
            // A pipe of one stage IS that stage. Rebis refuses a one-operand
            // arrow — an arrow routes an answer from somewhere to somewhere, and
            // there is nowhere — so emitting `(-> X)` would produce a program
            // that does not parse. `myth` allowed the form and meant nothing by
            // it, which is how this was missed until a test ran the output.
            if let [only] = stages.as_slice() {
                return write_node(only, losses);
            }
            let written: Result<Vec<String>, String> = stages
                .iter()
                .map(|stage| write_node(stage, losses))
                .collect();
            Ok(format!("(-> {})", written?.join(" ")))
        }
    }
}

/// A mediator square with `width` copies of one branch.
///
/// A square of one branch is still a square: it mediates over the single answer
/// and yields it, which is what a `gather` over a non-spread already meant.
fn square(mediator: &str, branch: &str, width: usize) -> String {
    let branches = vec![branch.to_string(); width.max(1)];
    format!("([{mediator}] {})", branches.join(" "))
}

/// The mediator name a gate becomes, recording what the name cannot carry.
fn gate_name(gate: &Gate, losses: &mut Losses) -> &'static str {
    match gate {
        Gate::Vote => "vote",
        Gate::First => "first",
        Gate::Check(command) => {
            // A mediator head is one atom, so the command cannot travel with it.
            // That is a deliberate narrowing — a shared program should not be
            // able to name a subprocess — but it means this translation is not
            // complete on its own and the operator has to finish it.
            losses.0.push(format!(
                "the gate command `{command}` cannot be written in a Rebis mediator; \
                 set it as `KAOS_GATE` and run with `--allow-tools`"
            ));
            "check"
        }
        Gate::Mirror(percent) => {
            losses.0.push(format!(
                "`(mirror {percent})`'s threshold is lost: `[mirror]` is judged by \
                 the calculus against the mediator's own word, not against the task \
                 at {percent}%"
            ));
            "mirror"
        }
    }
}

/// A myth instruction as the body of a Rebis prompt.
fn escape_prompt(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

// ── the reader: a tiny S-expression parser ──

/// Parse an S-expression into a [`Node`]. The whole grammar:
/// `fire` · `(ask "…")` · `(spread N X)` · `(gather G X)` · `(pipe A B …)` ·
/// gates `vote`/`first`/`(check "…")`.
pub fn parse(src: &str) -> Result<Node, String> {
    let toks = tokenize(src);
    let mut pos = 0;
    let node = parse_node(&toks, &mut pos)?;
    if pos != toks.len() {
        return Err("trailing tokens after the myth".into());
    }
    Ok(node)
}

fn tokenize(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = src.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            '(' | ')' => {
                out.push(c.to_string());
                chars.next();
            }
            '"' => {
                chars.next();
                let mut s = String::from("\"");
                // read until the closing quote; `\"` and `\\` escape through so a
                // check command can carry its own quotes (e.g. "grep \"$CANDIDATE\"").
                while let Some(c) = chars.next() {
                    match c {
                        '\\' => {
                            if let Some(e) = chars.next() {
                                s.push(e);
                            }
                        }
                        '"' => break,
                        _ => s.push(c),
                    }
                }
                out.push(s); // a quoted atom, prefixed with " to mark it a string
            }
            c if c.is_whitespace() => {
                chars.next();
            }
            _ => {
                let mut s = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_whitespace() || c == '(' || c == ')' {
                        break;
                    }
                    s.push(c);
                    chars.next();
                }
                out.push(s);
            }
        }
    }
    out
}

fn parse_node(toks: &[String], pos: &mut usize) -> Result<Node, String> {
    let t = toks.get(*pos).ok_or("unexpected end of myth")?.clone();
    *pos += 1;
    match t.as_str() {
        "fire" => Ok(Node::Fire),
        "(" => {
            let head = toks.get(*pos).ok_or("empty form")?.clone();
            *pos += 1;
            let node = match head.as_str() {
                "ask" => {
                    let s = toks
                        .get(*pos)
                        .ok_or("(ask \"…\"): missing instruction")?
                        .clone();
                    *pos += 1;
                    Node::Ask(s.strip_prefix('"').unwrap_or(&s).to_string())
                }
                "pipe" => {
                    let mut stages = Vec::new();
                    while toks.get(*pos).map(|t| t != ")").unwrap_or(false) {
                        stages.push(parse_node(toks, pos)?);
                    }
                    if stages.is_empty() {
                        return Err("(pipe …): needs at least one stage".into());
                    }
                    Node::Pipe(stages)
                }
                "spread" => {
                    let n: usize = toks
                        .get(*pos)
                        .and_then(|s| s.parse().ok())
                        .ok_or("(spread N X): N must be a number")?;
                    *pos += 1;
                    let child = parse_node(toks, pos)?;
                    Node::Spread(n.max(1), Box::new(child))
                }
                "gather" => {
                    let gate = parse_gate(toks, pos)?;
                    let child = parse_node(toks, pos)?;
                    Node::Gather(gate, Box::new(child))
                }
                other => {
                    return Err(format!(
                        "unknown form '{other}' (want ask/spread/gather/pipe)"
                    ))
                }
            };
            expect(toks, pos, ")")?;
            Ok(node)
        }
        other => Err(format!("expected `fire` or `(`, got '{other}'")),
    }
}

fn parse_gate(toks: &[String], pos: &mut usize) -> Result<Gate, String> {
    let t = toks.get(*pos).ok_or("missing gate")?.clone();
    *pos += 1;
    match t.as_str() {
        "vote" => Ok(Gate::Vote),
        "first" => Ok(Gate::First),
        "(" => {
            let head = toks.get(*pos).ok_or("empty gate form")?.clone();
            *pos += 1;
            match head.as_str() {
                "check" => {
                    let cmd = toks
                        .get(*pos)
                        .ok_or("(check \"cmd\"): missing command")?
                        .clone();
                    *pos += 1;
                    expect(toks, pos, ")")?;
                    Ok(Gate::Check(
                        cmd.strip_prefix('"').unwrap_or(&cmd).to_string(),
                    ))
                }
                "mirror" => {
                    let percent: u8 = toks
                        .get(*pos)
                        .and_then(|s| s.parse().ok())
                        .filter(|p| *p <= 100)
                        .ok_or("(mirror P): P must be a percentage 0..=100")?;
                    *pos += 1;
                    expect(toks, pos, ")")?;
                    Ok(Gate::Mirror(percent))
                }
                other => Err(format!("unknown gate form '{other}' (want check/mirror)")),
            }
        }
        other => Err(format!(
            "unknown gate '{other}' (want vote/first/(check …)/(mirror P))"
        )),
    }
}

fn expect(toks: &[String], pos: &mut usize, want: &str) -> Result<(), String> {
    match toks.get(*pos) {
        Some(t) if t == want => {
            *pos += 1;
            Ok(())
        }
        other => Err(format!("expected `{want}`, got {other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── the reader ──────────────────────────────────────────────────────────

    #[test]
    fn the_grammar_is_still_read() {
        assert_eq!(
            parse("(gather vote (spread 5 fire))").unwrap(),
            Node::Gather(Gate::Vote, Box::new(Node::Spread(5, Box::new(Node::Fire))))
        );
        assert_eq!(
            parse("(gather first (spread 3 fire))").unwrap(),
            Node::Gather(Gate::First, Box::new(Node::Spread(3, Box::new(Node::Fire))))
        );
        assert_eq!(
            parse(r#"(pipe (ask "Propose") (ask "Refine"))"#).unwrap(),
            Node::Pipe(vec![
                Node::Ask("Propose".into()),
                Node::Ask("Refine".into()),
            ])
        );
        assert!(parse("(bogus 3 fire)").is_err());
        assert!(parse("(spread x fire)").is_err());
        assert!(parse("fire fire").is_err());
    }

    #[test]
    fn a_check_command_keeps_its_own_quotes() {
        // A command that carries quotes must survive the reader, or the loss
        // reported by `to_rebis` would name the wrong command.
        let node =
            parse(r#"(gather (check "grep -qxF \"$CANDIDATE\" answers.txt") (spread 8 fire))"#)
                .unwrap();
        match node {
            Node::Gather(Gate::Check(command), _) => {
                assert_eq!(command, r#"grep -qxF "$CANDIDATE" answers.txt"#);
            }
            other => panic!("expected a check gather, got {other:?}"),
        }
    }

    #[test]
    fn the_mirror_gate_is_still_read_and_still_bounded() {
        assert_eq!(
            parse("(gather (mirror 40) (spread 5 fire))").unwrap(),
            Node::Gather(
                Gate::Mirror(40),
                Box::new(Node::Spread(5, Box::new(Node::Fire)))
            )
        );
        assert!(parse("(gather (mirror 101) fire)").is_err());
        assert!(parse("(gather (mirror x) fire)").is_err());
    }

    #[test]
    fn leaves_counts_cost_exposure() {
        // The surviving cost model, and the oracle the translation is checked
        // against: a myth's price and its Rebis translation's price must agree.
        assert_eq!(parse("fire").unwrap().leaves(), 1);
        assert_eq!(parse("(gather vote (spread 8 fire))").unwrap().leaves(), 8);
        assert_eq!(
            parse("(gather (mirror 40) (spread 5 fire))")
                .unwrap()
                .leaves(),
            5
        );
        let piped = parse(
            r#"(pipe (gather vote (spread 4 fire)) (ask "x") (gather first (spread 3 fire)))"#,
        )
        .unwrap();
        assert_eq!(piped.leaves(), 4 + 1 + 3);
    }

    // ── the translation ─────────────────────────────────────────────────────

    /// Translate, and assert the result is a Rebis program that parses.
    fn rebis_of(source: &str) -> (String, Losses) {
        let node = parse(source).expect("the myth parses");
        let (written, losses) = to_rebis(&node).expect("it translates");
        rebis_lang::parse(&written).unwrap_or_else(|error| {
            panic!("{source}\n  became {written}\n  which does not parse: {error}")
        });
        (written, losses)
    }

    #[test]
    fn the_conclave_becomes_a_written_square() {
        let (written, losses) = rebis_of("(gather vote (spread 3 fire))");
        assert_eq!(written, "(= task (&) ([vote] ($ task) ($ task) ($ task)))");
        assert!(losses.is_empty());
    }

    #[test]
    fn a_role_becomes_a_framing() {
        let (written, _) = rebis_of(r#"(ask "Propose an approach")"#);
        assert_eq!(written, "(= task (&) (+ \"Propose an approach\" ($ task)))");
    }

    #[test]
    fn a_pipe_becomes_an_arrow() {
        let (written, _) = rebis_of(r#"(pipe (ask "Draft") (ask "Refine"))"#);
        assert_eq!(
            written,
            "(= task (&) (-> (+ \"Draft\" ($ task)) (+ \"Refine\" ($ task))))"
        );
    }

    #[test]
    fn a_pipe_of_one_stage_is_that_stage_not_a_one_armed_arrow() {
        // Rebis refuses `(-> X)`, so this is a correctness fix rather than a
        // tidying: the naive translation produced a program that did not parse.
        let (written, _) = rebis_of(r#"(pipe (ask "Summarise"))"#);
        assert_eq!(written, "(= task (&) (+ \"Summarise\" ($ task)))");
        assert!(!written.contains("->"));
    }

    #[test]
    fn a_bare_spread_writes_the_implicit_final_vote() {
        // `run` votes when the top node leaves several candidates, so the
        // translation must write that vote or it would change the answer.
        let (written, _) = rebis_of("(spread 2 fire)");
        assert_eq!(written, "(= task (&) ([vote] ($ task) ($ task)))");
    }

    #[test]
    fn the_spread_width_becomes_a_countable_number_of_branches() {
        // The payoff, and the reason the width is not kept as a variable: the
        // price of the program is now on the page.
        for width in [1_usize, 4, 8] {
            let (written, _) = rebis_of(&format!("(gather vote (spread {width} fire))"));
            assert_eq!(written.matches("($ task)").count(), width);
        }
    }

    #[test]
    fn a_translated_myth_costs_what_the_myth_cost() {
        // The invariant that makes the shim safe to run: the same number of
        // firings, counted two ways.
        for source in [
            "fire",
            "(gather vote (spread 8 fire))",
            r#"(pipe (gather vote (spread 4 fire)) (ask "x") (gather first (spread 3 fire)))"#,
        ] {
            let node = parse(source).expect("parses");
            let (written, _) = to_rebis(&node).expect("translates");
            // Every leaf is one `($ task)`, and nothing else composes the task.
            assert_eq!(
                written.matches("($ task)").count(),
                node.leaves(),
                "{source} became {written}"
            );
        }
    }

    #[test]
    fn a_shell_gate_says_what_the_operator_must_finish() {
        let (written, losses) = rebis_of(r#"(gather (check "pytest -q") (spread 2 fire))"#);
        assert!(written.contains("[check]"));
        assert!(
            !written.contains("pytest"),
            "the command cannot travel: {written}"
        );
        assert_eq!(losses.0.len(), 1);
        assert!(
            losses.0[0].contains("KAOS_GATE") && losses.0[0].contains("pytest -q"),
            "{:?}",
            losses.0
        );
    }

    #[test]
    fn a_mirror_gate_says_what_it_loses() {
        let (written, losses) = rebis_of("(gather (mirror 40) (spread 2 fire))");
        assert!(written.contains("[mirror]"));
        assert_eq!(losses.0.len(), 1);
        assert!(losses.0[0].contains("40"), "{:?}", losses.0);
    }

    #[test]
    fn an_instruction_containing_a_quote_survives_the_round_trip() {
        let (written, _) = rebis_of(r#"(ask "say \"hello\" twice")"#);
        let parsed = rebis_lang::parse(&written).expect("parses");
        assert!(rebis_lang::format(&parsed).contains("hello"), "{written}");
    }
}
