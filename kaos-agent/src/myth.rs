//! The myth — a composition LAYER over kaos, written as an S-expression graph
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
//! `spread`/`gather` diverge and converge; `pipe`/`ask` sequence and role — so a
//! myth is a real agent-workflow language, not just a voter:
//!   conclave         (gather vote (spread 8 fire))
//!   mirror gate      (gather (mirror 40) (spread 5 fire))   ; answers must round-trip
//!   gated best-of-k  (gather (check "lake build") (spread 8 fire))
//!   a pipeline       (pipe (gather vote (spread 5 (ask "Propose a fix")))
//!                          (ask "Critique it; list the flaws")
//!                          (gather (check "pytest -q") (spread 3 (ask "Write the final code"))))
//!
//! A node evaluates to a LIST of candidates; `spread` grows the list
//! (concurrently), `gather` shrinks it to one, `pipe` threads one stage's answer
//! into the next. Minimal surface — five forms, three gates, one `run`.
use crate::scry::majority;
use rebis_lang as mirror;
use std::sync::atomic::{AtomicUsize, Ordering};

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

impl Gate {
    // `ctr` numbers gate-issued model calls; since the mirror gate went
    // deterministic no gate fires the cast, but the seam stays for gates
    // that may need it (the check gate takes the shell instead).
    fn pick<C: Cast>(
        &self,
        task: &str,
        cands: &[Option<String>],
        cast: &C,
        _ctr: &AtomicUsize,
    ) -> Option<String> {
        match self {
            Gate::Vote => {
                let votes: Vec<String> = cands.iter().flatten().cloned().collect();
                majority(&votes)
            }
            Gate::First => cands.iter().flatten().next().cloned(),
            Gate::Check(cmd) => cands
                .iter()
                .flatten()
                .find(|c| cast.check(task, c, cmd))
                .cloned(),
            Gate::Mirror(percent) => {
                // Reflect every candidate deterministically — Rebis's own
                // tokenizer is the compressor, no model and no prompt — and
                // score its holonomy against the task. Evaluation follows the
                // abraxas collider semantics: the candidates' own lines are the
                // record; atoms resolve to evidence there and broaden one hop
                // through its co-occurrence graph. An answer with no content
                // scores as maximal holonomy (it cannot round-trip).
                let threshold = f32::from(*percent) / 100.0;
                let texts: Vec<&String> = cands.iter().flatten().collect();
                let record = mirror::Record::from_texts(&texts);
                let mut best: Option<(f32, String)> = None;
                for cand in cands.iter().flatten() {
                    let h = mirror::holonomy_reflected(task, cand, &record);
                    if h <= threshold && best.as_ref().is_none_or(|(b, _)| h < *b) {
                        best = Some((h, cand.clone()));
                    }
                }
                best.map(|(_, cand)| cand)
            }
        }
    }
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
    /// Evaluate to a list of candidates. `ctr` hands each `fire` a unique index.
    fn eval<C: Cast>(&self, task: &str, cast: &C, ctr: &AtomicUsize) -> Vec<Option<String>> {
        match self {
            Node::Fire => vec![cast.fire(task, ctr.fetch_add(1, Ordering::Relaxed))],
            Node::Ask(instr) => {
                let prompt = format!("{instr}\n\n{task}");
                vec![cast.fire(&prompt, ctr.fetch_add(1, Ordering::Relaxed))]
            }
            Node::Spread(n, child) => std::thread::scope(|s| {
                let handles: Vec<_> = (0..*n)
                    .map(|_| s.spawn(|| child.eval(task, cast, ctr)))
                    .collect();
                handles
                    .into_iter()
                    .flat_map(|h| h.join().unwrap_or_default())
                    .collect()
            }),
            Node::Gather(gate, child) => {
                let cands = child.eval(task, cast, ctr);
                vec![gate.pick(task, &cands, cast, ctr)]
            }
            Node::Pipe(stages) => {
                let mut t = task.to_string();
                let mut cands = vec![None];
                for (i, stage) in stages.iter().enumerate() {
                    cands = stage.eval(&t, cast, ctr);
                    if i + 1 < stages.len() {
                        // collapse this stage to one answer; feed it to the next
                        let one = if cands.len() == 1 {
                            cands[0].clone()
                        } else {
                            Gate::Vote.pick(&t, &cands, cast, ctr)
                        };
                        if let Some(answer) = one {
                            t = format!("{task}\n\nWork so far:\n{answer}");
                        }
                    }
                }
                cands
            }
        }
    }
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

/// Run a myth on a task; the single collapsed answer (an implicit final
/// vote if the top node left more than one candidate).
pub fn run<C: Cast>(node: &Node, task: &str, cast: &C) -> Option<String> {
    let ctr = AtomicUsize::new(0);
    let cands = node.eval(task, cast, &ctr);
    if cands.len() == 1 {
        cands.into_iter().next().flatten()
    } else {
        Gate::Vote.pick(task, &cands, cast, &ctr)
    }
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

    struct Mock(Vec<Option<String>>);
    impl Cast for Mock {
        fn fire(&self, _t: &str, i: usize) -> Option<String> {
            self.0.get(i).cloned().flatten()
        }
        fn check(&self, _t: &str, c: &str, cmd: &str) -> bool {
            c == cmd // "passes" iff the candidate equals the gate string (test stub)
        }
    }

    #[test]
    fn parses_and_runs_the_conclave() {
        let n = parse("(gather vote (spread 5 fire))").unwrap();
        assert_eq!(
            n,
            Node::Gather(Gate::Vote, Box::new(Node::Spread(5, Box::new(Node::Fire))))
        );
        let m = Mock(vec![s("42"), s("42"), s("7"), s("42"), s("7")]);
        assert_eq!(run(&n, "q", &m).as_deref(), Some("42"));
    }

    #[test]
    fn parses_the_bowtie() {
        let n = parse("(gather vote (spread 2 (gather (check \"x\") (spread 3 fire))))").unwrap();
        // inner gather keeps the candidate equal to "x"; outer votes
        let m = Mock(vec![s("x"), s("a"), s("b"), s("x"), s("c"), s("d")]);
        assert_eq!(run(&n, "q", &m).as_deref(), Some("x"));
    }

    #[test]
    fn gates_first_and_errors() {
        assert_eq!(
            parse("(gather first (spread 3 fire))").unwrap(),
            Node::Gather(Gate::First, Box::new(Node::Spread(3, Box::new(Node::Fire))))
        );
        assert!(parse("(bogus 3 fire)").is_err());
        assert!(parse("(spread x fire)").is_err());
        assert!(parse("fire fire").is_err());
    }

    #[test]
    fn pipe_threads_stages_and_ask_prepends_role() {
        let n = parse(r#"(pipe (ask "Propose") (ask "Refine"))"#).unwrap();
        assert_eq!(
            n,
            Node::Pipe(vec![
                Node::Ask("Propose".into()),
                Node::Ask("Refine".into()),
            ])
        );
        // stage 0 fires index 0 ("draft"), stage 1 fires index 1 ("final");
        // the pipe returns the LAST stage's candidates.
        let m = Mock(vec![s("draft"), s("final")]);
        assert_eq!(run(&n, "q", &m).as_deref(), Some("final"));
    }

    #[test]
    fn pipe_collapses_a_spread_before_feeding_forward() {
        // first stage is a spread of 3; the pipe votes it to one before stage 2,
        // and the whole myth ends on stage 2's single candidate.
        let n = parse(r#"(pipe (spread 3 fire) (ask "Finish"))"#).unwrap();
        let m = Mock(vec![s("a"), s("a"), s("b"), s("done")]);
        assert_eq!(run(&n, "q", &m).as_deref(), Some("done"));
    }

    #[test]
    fn leaves_counts_cost_exposure() {
        assert_eq!(parse("fire").unwrap().leaves(), 1);
        assert_eq!(parse("(gather vote (spread 8 fire))").unwrap().leaves(), 8);
        // a pipe sums its stages; spreads inside multiply
        let n = parse(
            "(pipe (gather vote (spread 4 fire)) (ask \"x\") (gather first (spread 3 fire)))",
        )
        .unwrap();
        assert_eq!(n.leaves(), 4 + 1 + 3);
    }

    #[test]
    fn check_command_keeps_escaped_quotes() {
        // the first recipe in docs/loop.md: a command that carries its own quotes.
        let n = parse(r#"(gather (check "grep -qxF \"$CANDIDATE\" answers.txt") (spread 8 fire))"#)
            .unwrap();
        match n {
            Node::Gather(Gate::Check(cmd), _) => {
                assert_eq!(cmd, r#"grep -qxF "$CANDIDATE" answers.txt"#);
            }
            other => panic!("expected a check gather, got {other:?}"),
        }
    }

    #[test]
    fn parses_the_mirror_gate() {
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
    fn mirror_gate_keeps_the_answer_that_round_trips() {
        // Deterministic reflection: the candidate's own content tokens are
        // its echo — an answer about the task round-trips, an off-topic one
        // cannot. No reflection calls are scripted; the Mock stays empty.
        let gate = Gate::Mirror(60);
        let candidates = vec![
            s("Paris is the capital of France."),
            s("Bananas are yellow."),
        ];
        let m = Mock(vec![]);
        let counter = AtomicUsize::new(0);
        assert_eq!(
            gate.pick("capital of France?", &candidates, &m, &counter)
                .as_deref(),
            Some("Paris is the capital of France.")
        );
    }

    #[test]
    fn mirror_gate_picks_the_lowest_holonomy_survivor() {
        // Candidate order must not matter: the off-topic answer comes first
        // and still loses to the answer that round-trips onto the task.
        let gate = Gate::Mirror(80);
        let candidates = vec![s("bananas are yellow"), s("sleep against commits tonight")];
        let m = Mock(vec![]);
        let counter = AtomicUsize::new(0);
        assert_eq!(
            gate.pick("sleep versus commits", &candidates, &m, &counter)
                .as_deref(),
            Some("sleep against commits tonight")
        );
    }

    #[test]
    fn mirror_gate_refuses_when_nothing_round_trips() {
        let n = parse("(gather (mirror 20) (spread 2 fire))").unwrap();
        let m = Mock(vec![s("the weather is nice"), s("bananas are yellow")]);
        assert_eq!(run(&n, "sleep versus commits", &m), None);
    }

    #[test]
    fn mirror_gate_adds_no_leaf_exposure() {
        assert_eq!(
            parse("(gather (mirror 40) (spread 5 fire))")
                .unwrap()
                .leaves(),
            5
        );
        assert_eq!(parse("(gather vote (spread 5 fire))").unwrap().leaves(), 5);
    }

    fn s(x: &str) -> Option<String> {
        Some(x.to_string())
    }
}
