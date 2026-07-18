//! Binary-native conclave solve — the mid-band edge as a first-class operation.
//!
//! The measured math edge (a mid-band model lifted ~+23pts on AIME2025 by
//! verified best-of-k with majority/adjourned voting) lived only in an external
//! harness. This module brings it into the binary, on ANY mind that supports
//! sampled completion (ollama, OpenAI, OpenRouter) — not just the ollama GSM8K
//! bench. `conclave` in the CLI dispatches here.
//!
//! Enchant Long, Divine Short (PsyberMagick): generation is a lottery — draw `k`
//! diverse samples (solar/lunar polarity for spread); selection must be reliable
//! — the modal answer via [`crate::scry::adjourned_vote`], which spends only as
//! many ballots as the vote needs.
use crate::agent::{write_files_into, Workspace};
use crate::backend::Sampling;
use crate::conductor::{Chat, Conductor, ProviderChat, Step, Tool};
use crate::myth::Cast;
use crate::provider::Spec;
use crate::spiral::Polarity;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

const SYSTEM: &str = "You are an expert problem solver. Work through the problem step by \
    step, then give the final answer on its own line as \\boxed{...} with only the answer \
    inside the braces.";

/// Deterministic per-(question, sample) seed so a conclave is diverse yet
/// reproducible (FNV-1a; no external rng dependency).
fn seed_of(question: &str, i: usize) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in question.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h ^= i as u64;
    h.wrapping_mul(0x100000001b3)
}

/// Pull a final answer from a model reply: the last balanced `\boxed{...}`, else
/// the text after the last "answer:"/"answer=", else the last non-empty line.
pub fn extract_answer(text: &str) -> Option<String> {
    if let Some(a) = last_boxed(text) {
        return Some(a);
    }
    let lower = text.to_lowercase();
    if let Some(p) = lower.rfind("answer") {
        let tail = &text[p..];
        if let Some(rel) = tail.find([':', '=']) {
            let line = tail[rel + 1..]
                .lines()
                .next()
                .unwrap_or("")
                .trim()
                .trim_matches(|c| c == '*' || c == '.' || c == ' ' || c == '$');
            // a terse answer, not a rambling "answer: this is a grid where…" line
            if !line.is_empty() && line.chars().count() <= 24 {
                return Some(line.to_string());
            }
        }
    }
    // Last resort: the final non-empty line — but only if it is short enough to
    // BE an answer (a number/expression/letter). Long prose is a fizzle, not a
    // ballot; letting it vote is what produced garbled conclave verdicts.
    let last = text.lines().rev().find(|l| !l.trim().is_empty())?.trim();
    if last.chars().count() <= 24 {
        Some(last.to_string())
    } else {
        None
    }
}

fn last_boxed(text: &str) -> Option<String> {
    let idx = text.rfind("\\boxed")?;
    let bytes = text.as_bytes();
    let mut i = idx + "\\boxed".len();
    while i < bytes.len() && bytes[i] != b'{' {
        i += 1;
    }
    if i >= bytes.len() {
        return None;
    }
    let start = i + 1;
    let (mut depth, mut j) = (1i32, start);
    while j < bytes.len() {
        match bytes[j] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(text[start..j].trim().to_string());
                }
            }
            _ => {}
        }
        j += 1;
    }
    None
}

/// The bridge from a [`crate::myth`] graph down to the chat API: `fire` is one
/// sampled completion (diverse via solar/lunar polarity + a per-index seed);
/// `check` runs a shell verifier with the candidate on `$CANDIDATE` and stdin.
pub struct ChatCast<'a> {
    pub spec: &'a Spec,
    pub timeout: Duration,
}

impl Cast for ChatCast<'_> {
    fn fire(&self, task: &str, i: usize) -> Option<String> {
        status(&format!(
            "myth \u{25b8} leaf {i} \u{00b7} asking the mind \u{2026}"
        ));
        let mut s = Sampling::seeded(seed_of(task, i));
        s.temperature = Polarity::of_attempt(i).temperature();
        let answer = match self
            .spec
            .complete_sampled(SYSTEM, task, self.timeout, Some(s))
        {
            Ok(reply) => extract_answer(&reply),
            Err(_) => None,
        };
        match &answer {
            Some(a) => {
                let a: String = a.chars().take(48).collect();
                status(&format!("myth \u{25b8} leaf {i} \u{00b7} {a}"));
            }
            None => status(&format!("myth \u{25b8} leaf {i} \u{00b7} no answer")),
        }
        answer
    }

    fn check(&self, _task: &str, candidate: &str, cmd: &str) -> bool {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let child = Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .env("CANDIDATE", candidate)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        let mut child = match child {
            Ok(c) => c,
            Err(_) => return false,
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(candidate.as_bytes());
        }
        child.wait().map(|s| s.success()).unwrap_or(false)
    }
}

/// The myth's leaf as a *real agentic session*, not a single completion. Each
/// `fire` runs a full [`Conductor`] tool-loop (read/edit/bash) in an **isolated
/// copy** of `root`, then returns the diff it produced — encoded so it both votes
/// (identical diffs share a string) and can be re-applied. `check` re-applies that
/// diff to a fresh copy and runs the gate there. This is what makes a myth *act*:
/// a `(spread k (ask …))` becomes k agents editing k private copies, and a
/// `(gather (check …) …)` keeps only the one whose changes actually pass.
///
/// Selected by `KAOS_AGENTIC`; `root` is the working tree (`KAOS_ARENA`, default
/// the current dir). Concurrency is safe: every session gets its own [`Workspace`].
pub struct AgentCast<'a> {
    pub spec: &'a Spec,
    pub timeout: Duration,
    pub root: PathBuf,
    pub max_steps: usize,
    pub bash_timeout_s: u64,
    /// Wall cap for a `check` gate (a build/test/benchmark re-run). Wider than the
    /// per-`bash` cap because a gate may run the whole suite; `KAOS_GATE_TIMEOUT_S`.
    pub gate_timeout_s: u64,
}

/// A std-only counting semaphore: caps how many agent sessions run at once, so a
/// wide `spread` throttles instead of firing every opus session in one burst (which
/// drains a subscription in minutes). `KAOS_MAX_CONCURRENCY`, default 3.
struct Semaphore {
    n: std::sync::Mutex<usize>,
    cv: std::sync::Condvar,
}

fn concurrency_gate() -> &'static Semaphore {
    use std::sync::OnceLock;
    static GATE: OnceLock<Semaphore> = OnceLock::new();
    GATE.get_or_init(|| {
        let n = std::env::var("KAOS_MAX_CONCURRENCY")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3usize)
            .max(1);
        Semaphore {
            n: std::sync::Mutex::new(n),
            cv: std::sync::Condvar::new(),
        }
    })
}

/// Held for the duration of one agent session; releases a slot on drop (so an early
/// `?` return can't leak a permit).
struct Permit;
impl Drop for Permit {
    fn drop(&mut self) {
        let g = concurrency_gate();
        *g.n.lock().unwrap() += 1;
        g.cv.notify_one();
    }
}
fn acquire_permit() -> Permit {
    let g = concurrency_gate();
    let mut n = g.n.lock().unwrap();
    while *n == 0 {
        n = g.cv.wait(n).unwrap();
    }
    *n -= 1;
    Permit
}

impl Cast for AgentCast<'_> {
    fn fire(&self, task: &str, i: usize) -> Option<String> {
        let _permit = acquire_permit(); // throttle: at most KAOS_MAX_CONCURRENCY at once
        status(&format!(
            "myth \u{25b8} leaf {i} \u{00b7} agent session started"
        ));
        let mut s = Sampling::seeded(seed_of(task, i));
        s.temperature = Polarity::of_attempt(i).temperature();
        let chat = ProviderChat {
            spec: self.spec.clone(),
            timeout_s: self.timeout.as_secs(),
            sampling: Some(s),
        };
        // Trace the model round-trips too, so the (often long) first call and any
        // action-less reply are visible instead of a silent gap.
        let chat = TracingChat {
            inner: &chat,
            leaf: i,
        };
        let patch = agent_patch(
            &self.root,
            task,
            &chat,
            self.max_steps,
            self.bash_timeout_s,
            |step| step_status(i, step),
        );
        match &patch {
            Some(p) => {
                let n = decode_patchset(p).map(|m| m.len()).unwrap_or(0);
                status(&format!(
                    "myth \u{25b8} leaf {i} \u{00b7} captured a {n}-file diff"
                ));
            }
            None => status(&format!(
                "myth \u{25b8} leaf {i} \u{00b7} changed nothing \u{2014} no ballot"
            )),
        }
        patch
    }

    fn check(&self, _task: &str, candidate: &str, cmd: &str) -> bool {
        let changed = match decode_patchset(candidate) {
            Some(c) => c,
            None => {
                status("gate \u{25b8} malformed candidate \u{00b7} FAIL");
                return false;
            }
        };
        let n = changed.len();
        let ws = match Workspace::isolate(&self.root) {
            Ok(w) => w,
            Err(_) => return false,
        };
        if write_files_into(&ws.root, &changed).is_err() {
            return false;
        }
        status(&format!(
            "gate \u{25b8} {n}-file candidate \u{00b7} running \u{2026}"
        ));
        let ok = ws.verify_within(cmd, self.gate_timeout_s).0;
        status(&format!(
            "gate \u{25b8} {n}-file candidate \u{00b7} {}",
            if ok { "PASS" } else { "FAIL" }
        ));
        ok
        // ws dropped here → the copy is removed
    }
}

/// Run one agentic session in a private copy of `root` and return its diff as an
/// encoded patchset (or `None` if it changed nothing / failed). Split out from
/// [`AgentCast::fire`] so the loop can be tested with a scripted [`Chat`], no model.
fn agent_patch(
    root: &Path,
    task: &str,
    chat: &dyn Chat,
    max_steps: usize,
    bash_timeout_s: u64,
    on_step: impl FnMut(&Step),
) -> Option<String> {
    let ws = Workspace::isolate(root).ok()?;
    let conductor = Conductor {
        root: ws.root.clone(),
        max_steps,
        bash_timeout_s,
        system_appendix: None,
    };
    let _ = conductor.run(task, chat, on_step);
    let changed = ws.changed_files(root).ok()?;
    if changed.is_empty() {
        return None; // an agent that changed nothing casts no ballot
    }
    Some(encode_patchset(&changed))
}

/// Wraps a [`Chat`] to narrate each model round-trip: a "thinking" line before the
/// call (so a slow first response isn't a silent gap) and a flag when a reply
/// carried no `<act>` block — the usual reason an agentic leaf churns to its step
/// budget without doing anything.
struct TracingChat<'a> {
    inner: &'a dyn Chat,
    leaf: usize,
}

impl Chat for TracingChat<'_> {
    fn respond(&self, system: &str, transcript: &str) -> Result<String, String> {
        status(&format!(
            "  \u{22ef} leaf {} \u{00b7} thinking \u{2026}",
            self.leaf
        ));
        let r = self.inner.respond(system, transcript);
        match &r {
            Ok(reply) if !reply.contains("<act") => status(&format!(
                "  \u{22ef} leaf {} \u{00b7} reply carried no action \u{2014} nudging",
                self.leaf
            )),
            Err(e) => {
                let e: String = e.chars().take(60).collect();
                status(&format!(
                    "  \u{22ef} leaf {} \u{00b7} model call failed: {e}",
                    self.leaf
                ));
            }
            _ => {}
        }
        r
    }
    fn label(&self) -> String {
        self.inner.label()
    }
}

/// Turn a myth's collapsed verdict into readable final-result text. A plain answer
/// is returned as-is; an agentic patchset (from [`AgentCast`]) is summarized as the
/// files it changed — the raw length-prefixed blob is never shown to a human.
pub fn render_verdict(verdict: &str) -> String {
    match decode_patchset(verdict) {
        Some(files) if verdict.starts_with("KAOSPATCH1") => {
            let mut lines = vec![format!("the winning diff — {} file(s):", files.len())];
            for (path, contents) in &files {
                match contents {
                    Some(c) => lines.push(format!("    ~ {path}  ({} bytes)", c.len())),
                    None => lines.push(format!("    - {path}  (deleted)")),
                }
            }
            lines.join("\n")
        }
        _ => verdict.to_string(),
    }
}

/// Live progress for the agentic myth. Suppressed by `KAOS_QUIET`.
fn quiet() -> bool {
    crate::config::enabled("KAOS_QUIET")
}

/// One self-contained, dim status line, flushed. Thread-safe: `println!` locks
/// stdout per call, so the concurrent leaves of a `spread` never split a line —
/// each carries its own leaf index, so interleaved output stays legible.
fn status(line: &str) {
    if quiet() {
        return;
    }
    use std::io::Write;
    println!("\x1b[38;5;244m{line}\x1b[0m");
    let _ = std::io::stdout().flush();
}

/// Render one agent step under its leaf: the tool, and for a `bash`/`finish` the
/// tail of what it observed (a test verdict, the finish message).
fn step_status(i: usize, step: &Step) {
    let mut line = format!("  \u{22ef} leaf {i} \u{00b7} {}", step.tool.describe());
    if matches!(step.tool, Tool::Bash { .. } | Tool::Finish { .. }) {
        if let Some(tail) = step
            .observation
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
        {
            let tail = tail.trim();
            let tail: String = tail.chars().take(60).collect();
            line.push_str(&format!(" \u{2192} {tail}"));
        }
    }
    status(&line);
}

/// Encode a change-set (rel-path → `Some(contents)` / `None` for a deletion) as a
/// length-prefixed text blob: reversible, and canonical so two identical diffs
/// produce the identical string (self-consistency voting still works on patches).
fn encode_patchset(files: &BTreeMap<String, Option<String>>) -> String {
    let mut s = String::from("KAOSPATCH1\n");
    for (path, contents) in files {
        match contents {
            Some(c) => {
                s.push_str(&format!("W {} {}\n", c.len(), path));
                s.push_str(c);
                s.push('\n');
            }
            None => s.push_str(&format!("D {}\n", path)),
        }
    }
    s
}

/// Inverse of [`encode_patchset`]. Returns `None` on any malformed input so a
/// corrupt candidate fails the gate rather than applying a partial diff.
fn decode_patchset(s: &str) -> Option<BTreeMap<String, Option<String>>> {
    let b = s.as_bytes();
    let nl = |from: usize| (from..b.len()).find(|&i| b[i] == b'\n');
    let mut out = BTreeMap::new();
    let mut pos = nl(0)?;
    if &s[..pos] != "KAOSPATCH1" {
        return None;
    }
    pos += 1;
    while pos < b.len() {
        let end = nl(pos)?;
        let line = &s[pos..end];
        if let Some(rest) = line.strip_prefix("W ") {
            let (len_str, path) = rest.split_once(' ')?;
            let len: usize = len_str.parse().ok()?;
            let cstart = end + 1;
            let cend = cstart.checked_add(len)?;
            if cend > b.len() {
                return None;
            }
            let contents = std::str::from_utf8(&b[cstart..cend]).ok()?.to_string();
            out.insert(path.to_string(), Some(contents));
            pos = cend;
            if pos < b.len() && b[pos] == b'\n' {
                pos += 1;
            }
        } else if let Some(path) = line.strip_prefix("D ") {
            out.insert(path.to_string(), None);
            pos = end + 1;
        } else if line.is_empty() {
            pos = end + 1;
        } else {
            return None;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_boxed_over_prose() {
        assert_eq!(
            extract_answer("blah \\boxed{p - q} done").as_deref(),
            Some("p - q")
        );
        assert_eq!(
            extract_answer("nested \\boxed{\\frac{1}{2}} x").as_deref(),
            Some("\\frac{1}{2}")
        );
    }

    #[test]
    fn falls_back_to_answer_line_then_last_line() {
        assert_eq!(
            extract_answer("reasoning...\nAnswer: 42").as_deref(),
            Some("42")
        );
        assert_eq!(extract_answer("final answer = B").as_deref(), Some("B"));
        assert_eq!(
            extract_answer("no marker\njust 7").as_deref(),
            Some("just 7")
        );
        assert_eq!(extract_answer("   ").as_deref(), None);
    }

    #[test]
    fn seed_is_deterministic_and_varies() {
        assert_eq!(seed_of("q", 0), seed_of("q", 0));
        assert_ne!(seed_of("q", 0), seed_of("q", 1));
        assert_ne!(seed_of("a", 0), seed_of("b", 0));
    }

    #[test]
    fn patchset_roundtrips_including_tricky_contents() {
        let mut cs: BTreeMap<String, Option<String>> = BTreeMap::new();
        // contents that mimic the encoding's own header lines must survive
        cs.insert(
            "a.rs".into(),
            Some("line1\nW 5 fake\nD gone\nline4\n".into()),
        );
        cs.insert("dir/b.py".into(), Some("x = 1".into()));
        cs.insert("gone.txt".into(), None);
        let enc = encode_patchset(&cs);
        assert_eq!(decode_patchset(&enc).unwrap(), cs);
        assert!(decode_patchset("not a patchset").is_none());
        assert!(decode_patchset("KAOSPATCH1\nW 99 a.rs\nshort").is_none());
    }

    #[test]
    fn agent_session_captures_diff_and_check_reverifies() {
        use crate::conductor::ScriptedChat;
        let dir = std::env::temp_dir().join(format!(
            "kaos-agentcast-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("answer.txt"), "WRONG").unwrap();

        // A scripted agent that edits the file and finishes — a real Conductor loop,
        // no model — running in an isolated copy of `dir`.
        let chat = ScriptedChat::new(vec![
            "<act tool=\"write_file\"><arg name=\"path\">answer.txt</arg><arg name=\"contents\">RIGHT</arg></act>",
            "<act tool=\"finish\"><arg name=\"message\">done</arg></act>",
        ]);
        let patch =
            agent_patch(&dir, "make it right", &chat, 4, 10, |_| {}).expect("a captured diff");
        let decoded = decode_patchset(&patch).unwrap();
        assert_eq!(decoded.get("answer.txt"), Some(&Some("RIGHT".to_string())));
        // the source was never mutated — the session ran in a private copy
        assert_eq!(
            std::fs::read_to_string(dir.join("answer.txt")).unwrap(),
            "WRONG"
        );

        // check()'s semantics: re-apply the diff to a fresh copy and run the gate.
        let changed = decode_patchset(&patch).unwrap();
        let ws = Workspace::isolate(&dir).unwrap();
        write_files_into(&ws.root, &changed).unwrap();
        assert!(
            ws.verify("grep -q RIGHT answer.txt").0,
            "gate should pass on the applied diff"
        );
        assert!(!ws.verify("grep -q NOPE answer.txt").0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn agentcast_is_sync() {
        fn is_sync<T: Sync>() {}
        is_sync::<AgentCast<'_>>(); // required: `spread` fans out concurrently
    }

    #[test]
    fn check_honors_gate_timeout() {
        // A hanging gate must weigh false in bounded time (KAOS_GATE_TIMEOUT_S),
        // not stall the whole conclave.
        let dir = std::env::temp_dir().join(format!(
            "kaos-gate-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("f.txt"), "x").unwrap();
        let spec = Spec::simulated();
        let cast = AgentCast {
            spec: &spec,
            timeout: Duration::from_secs(1),
            root: dir.clone(),
            max_steps: 1,
            bash_timeout_s: 5,
            gate_timeout_s: 1,
        };
        let patch = encode_patchset(&BTreeMap::from([(
            "f.txt".to_string(),
            Some("y".to_string()),
        )]));
        let start = std::time::Instant::now();
        assert!(
            !cast.check("t", &patch, "sleep 30"),
            "a hung gate must weigh false"
        );
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "must return near the 1s cap"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
