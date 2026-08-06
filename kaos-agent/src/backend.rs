//! Backends — where a charged sigil actually fires.
//!
//! The simulation core never needs this: [`kaos_pact::gnosis::charge`] samples the
//! outcome from Carroll's equation, offline and deterministically. But the live
//! app can fire a charged intent at a *real* model. The only real backend wired
//! here is the `claude` CLI, which is authenticated on the host — no API key, no
//! crate, just `std::process`. Keeping the executor behind this seam means the
//! orchestration (routing, sigilization, banishing, the egregore) is identical
//! whether reward is simulated or real; swapping in a test-harness verifier is the
//! documented next step.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn claude_completion_permission_args(approved: bool) -> &'static [&'static str] {
    if approved {
        &["--dangerously-skip-permissions"]
    } else {
        &[
            "--tools",
            "",
            "--permission-mode",
            "dontAsk",
            "--disable-slash-commands",
        ]
    }
}

/// [`fire_claude`] with an explicit model tag (`sonnet`, `opus`, a full model id…)
/// passed to the CLI's `--model`. `None` keeps the CLI's own default.
pub fn fire_claude_as(
    model: Option<&str>,
    charged_intent: &str,
    system: &str,
) -> Result<String, String> {
    let mut cmd = Command::new("claude");
    cmd.arg("-p")
        .arg(charged_intent)
        .arg("--append-system-prompt")
        .arg(system);
    if let Some(m) = model {
        cmd.arg("--model").arg(m);
    }
    let approved = std::env::var("KAOS_CLAUDE_YOLO")
        .map(|value| !matches!(value.as_str(), "0" | "false" | "no" | ""))
        .unwrap_or(false);
    // Approved chaos calls must not pause for a second Claude permission.
    // Unapproved one-shot/normal calls are completions with no hidden tools.
    cmd.args(claude_completion_permission_args(approved));
    // This path is the claude.ai *subscription* CLI (no API key). A stray
    // ANTHROPIC_API_KEY in the environment makes the CLI switch to API-key auth and
    // fail ("Invalid API key", exit 1) — so strip it and let the host login stand.
    cmd.env_remove("ANTHROPIC_API_KEY");
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not summon `claude`: {e}"))?;

    // Nothing to write to stdin; close it.
    drop(child.stdin.take());

    let out = child
        .wait_with_output()
        .map_err(|e| format!("the charge was interrupted: {e}"))?;

    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        // The CLI prints its real error (auth, usage limits) to STDOUT in `-p` mode;
        // stderr is often just a warning. Surface both, or a failure looks blank.
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        let detail = format!("{} {}", stdout.trim(), stderr.trim());
        Err(format!(
            "the charge fizzled (exit {}): {}",
            out.status.code().unwrap_or(-1),
            detail.trim()
        ))
    }
}

/// Delegate a whole coding task to the `claude` CLI as a *real agent* in `root`.
///
/// The `claude` CLI is itself an agentic harness with its own read/edit/bash tools,
/// so the right move is not to drive it turn-by-turn through our own protocol but to
/// hand it the task and let it work — in the target directory, with permission to
/// edit. Its output lines are handed to `emit` as they complete, so a caller can
/// stream them into a trace. Returns Ok(()) on a clean exit.
///
/// Permissions: headless edits need a non-interactive permission mode. We default to
/// `acceptEdits` (auto-accept file writes); set `KAOS_CLAUDE_YOLO=1` to pass
/// `--dangerously-skip-permissions` so it may also run bash (e.g. to run tests).
pub fn run_claude_agent(
    root: &std::path::Path,
    task: &str,
    model: Option<&str>,
    emit: impl FnMut(&str),
) -> Result<(), String> {
    run_claude_agent_inner(root, task, model, true, None, emit).map(|_| ())
}

/// Direct Claude CLI agent with its final result returned to the host. Rebis
/// uses this seam so one native file-editing agent remains one language node
/// and its closing value can continue through arrows without invoking Kaos's
/// Conductor pipeline.
pub fn run_claude_agent_with_result(
    root: &std::path::Path,
    task: &str,
    model: Option<&str>,
    emit: impl FnMut(&str),
) -> Result<String, String> {
    run_claude_agent_inner(root, task, model, true, None, emit)
}

/// Chat-only Claude entry point. The regular agent API streams a rich tool
/// trace for terminal and visual work surfaces; a conversation needs only its
/// final answer, with the Rebis reference attached at the system boundary.
pub fn run_claude_chat_with_result(
    root: &std::path::Path,
    task: &str,
    model: Option<&str>,
) -> Result<String, String> {
    run_claude_chat_with_result_stream(root, task, model, |_| {})
}

/// Chat-only Claude entry point with a live event callback.
///
/// The ordinary [`run_claude_chat_with_result`] boundary stays final-answer
/// only for callers that embed chat as a request/response function. Console
/// callers can use this variant to receive each `stream-json` event as soon as
/// Claude writes it, while the returned string remains the canonical answer.
pub fn run_claude_chat_with_result_stream(
    root: &std::path::Path,
    task: &str,
    model: Option<&str>,
    emit: impl FnMut(&str),
) -> Result<String, String> {
    run_claude_agent_inner(root, task, model, true, None, emit)
}

/// One independent direct Claude agent with no conversation resume semantics.
/// Rebis data flow supplies node context explicitly, so sharing a Claude
/// session between nodes would leak sibling context and reuse one session id.
pub fn run_claude_agent_once_with_result(
    root: &std::path::Path,
    task: &str,
    model: Option<&str>,
    emit: impl FnMut(&str),
) -> Result<String, String> {
    run_claude_agent_inner(root, task, model, false, None, emit)
}

/// The `claude` CLI requires `--session-id`/`--resume` to be a UUID. Kaos
/// session ids are time-ordered `{millis}-{pid}` strings, so map one to a stable
/// v4-shaped UUID (same id in → same UUID out, so a resume finds its session). A
/// value that is already a UUID is passed through unchanged.
fn claude_session_uuid(raw: &str) -> String {
    fn is_uuid(s: &str) -> bool {
        let bytes = s.as_bytes();
        bytes.len() == 36
            && bytes.iter().enumerate().all(|(i, &c)| {
                if matches!(i, 8 | 13 | 18 | 23) {
                    c == b'-'
                } else {
                    c.is_ascii_hexdigit()
                }
            })
    }
    if is_uuid(raw) {
        return raw.to_string();
    }
    use std::hash::{Hash, Hasher};
    let half = |salt: u64| {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        salt.hash(&mut hasher);
        raw.hash(&mut hasher);
        hasher.finish()
    };
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&half(0).to_be_bytes());
    bytes[8..].copy_from_slice(&half(1).to_be_bytes());
    bytes[6] = (bytes[6] & 0x0f) | 0x40; // version 4
    bytes[8] = (bytes[8] & 0x3f) | 0x80; // RFC 4122 variant
    let h = |b: u8| format!("{b:02x}");
    format!(
        "{}{}{}{}-{}{}-{}{}-{}{}-{}{}{}{}{}{}",
        h(bytes[0]),
        h(bytes[1]),
        h(bytes[2]),
        h(bytes[3]),
        h(bytes[4]),
        h(bytes[5]),
        h(bytes[6]),
        h(bytes[7]),
        h(bytes[8]),
        h(bytes[9]),
        h(bytes[10]),
        h(bytes[11]),
        h(bytes[12]),
        h(bytes[13]),
        h(bytes[14]),
        h(bytes[15]),
    )
}

fn run_claude_agent_inner(
    root: &std::path::Path,
    task: &str,
    model: Option<&str>,
    persist_session: bool,
    system_appendix: Option<&str>,
    mut emit: impl FnMut(&str),
) -> Result<String, String> {
    let mut cmd = Command::new("claude");
    // Feed the task through print-mode text stdin rather than a positional
    // argument. This keeps large multiline code intact and avoids the OS's
    // per-argument size limit.
    cmd.arg("-p").arg("--input-format").arg("text");
    // An explicit caller may still supply a provider-native system appendix
    // for a transport protocol. Rebis documentation is never added here.
    if let Some(appendix) = system_appendix {
        cmd.arg("--append-system-prompt").arg(appendix);
    }
    if let Some(m) = model {
        cmd.arg("--model").arg(m); // the subscription's inner model (sonnet/opus/…)
    }
    // Stream the agent's EVENTS, not just its final answer: with plain -p the CLI
    // is silent for minutes and the reader learns nothing. stream-json emits one
    // JSON line per event (its remarks, every tool call, results); we render them
    // live — narration, inline diffs, commands — via [`claude_event_lines`].
    #[cfg(feature = "api")]
    cmd.arg("--output-format")
        .arg("stream-json")
        .arg("--verbose");
    #[cfg(not(feature = "api"))]
    cmd.arg("--output-format").arg("text");
    let yolo = std::env::var("KAOS_CLAUDE_YOLO")
        .map(|v| !matches!(v.as_str(), "0" | "false" | "no" | ""))
        .unwrap_or(false);
    if yolo {
        cmd.arg("--dangerously-skip-permissions");
    } else {
        cmd.arg("--permission-mode").arg("acceptEdits");
    }
    // Memory across turns: the caller pins one claude conversation to the session
    // via KAOS_SESSION. The `claude` CLI requires that id to be a UUID, but Kaos
    // session ids are time-ordered strings, so derive a stable UUID from the id
    // (the same input always yields the same UUID, so create and resume match).
    // The first turn CREATES it (--session-id); later turns RESUME it
    // (--resume KAOS_RESUME=1), so claude keeps the full history.
    if persist_session {
        if let Some(sid) = std::env::var("KAOS_SESSION").ok().filter(|s| !s.is_empty()) {
            let sid = claude_session_uuid(&sid);
            let resume = std::env::var("KAOS_RESUME")
                .map(|v| matches!(v.as_str(), "1" | "true" | "yes"))
                .unwrap_or(false);
            if resume {
                cmd.arg("--resume").arg(sid);
            } else {
                cmd.arg("--session-id").arg(sid);
            }
        }
    }
    cmd.env_remove("ANTHROPIC_API_KEY"); // subscription CLI — a stray key breaks its auth
    let mut child = cmd
        .current_dir(root) // ← the fix: work in the target, not wherever kaos launched
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not summon `claude`: {e}"))?;

    // Write concurrently: a prompt larger than the pipe buffer must not block
    // Kaos before it starts draining the CLI's stdout/stderr.
    let prompt = task.to_string();
    let input_writer = child.stdin.take().map(|mut stdin| {
        std::thread::spawn(move || std::io::Write::write_all(&mut stdin, prompt.as_bytes()))
    });

    // Drain stderr on a reader thread WHILE stdout streams below: a chatty stderr
    // that fills the OS pipe buffer would otherwise wedge the child mid-Work.
    let err_reader = child.stderr.take().map(|mut e| {
        std::thread::spawn(move || {
            let mut s = String::new();
            let _ = std::io::Read::read_to_string(&mut e, &mut s);
            s
        })
    });
    // Stream stdout line by line so the trace shows progress.
    let mut final_result = String::new();
    if let Some(out) = child.stdout.take() {
        use std::io::BufRead;
        let reader = std::io::BufReader::new(out);
        for line in reader.lines().map_while(Result::ok) {
            capture_claude_result(&mut final_result, &line);
            emit(&line);
        }
    }
    let input_result = input_writer.map(|writer| {
        writer
            .join()
            .map_err(|_| "the Claude prompt writer panicked".to_string())
            .and_then(|result| {
                result.map_err(|error| format!("could not send task to Claude: {error}"))
            })
    });
    let stderr = err_reader.and_then(|h| h.join().ok()).unwrap_or_default();
    let status = child
        .wait()
        .map_err(|e| format!("the Work was interrupted: {e}"))?;
    if let Some(Err(error)) = input_result {
        return Err(error);
    }
    if status.success() {
        Ok(final_result)
    } else {
        // Surface the CLI's own reason (auth, usage limits, a bad session id)
        // rather than a bare exit code. The real message may land on stderr or,
        // in `-p` mode, on stdout — include whichever we captured.
        let detail = format!("{} {}", final_result.trim(), stderr.trim());
        let detail = detail.trim();
        if detail.is_empty() {
            Err(format!(
                "claude exited with {}",
                status.code().unwrap_or(-1)
            ))
        } else {
            Err(format!(
                "claude exited with {}: {detail}",
                status.code().unwrap_or(-1)
            ))
        }
    }
}

#[cfg(feature = "api")]
fn capture_claude_result(result: &mut String, raw: &str) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return;
    };
    if value["type"].as_str() == Some("result") {
        *result = value["result"].as_str().unwrap_or_default().to_string();
    }
}

#[cfg(not(feature = "api"))]
fn capture_claude_result(result: &mut String, raw: &str) {
    if !result.is_empty() {
        result.push('\n');
    }
    result.push_str(raw);
}

/// Render one line of the claude CLI's `stream-json` feed into themed, human
/// trace lines — the same live language as the conductor's own steps: the
/// model's remarks as ☾ narration, Edit/Write as inline diffs, Bash as `$`.
/// Unparseable lines pass through untouched (so a plain-text claude still shows
/// something). Returns no lines for pure plumbing events (init, tool results).
#[cfg(feature = "api")]
pub fn claude_event_lines(raw: &str) -> Vec<String> {
    use kaos_core::theme::*;
    let raw = raw.trim();
    if raw.is_empty() {
        return Vec::new();
    }
    let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) else {
        return vec![format!("  {}", ash(raw))];
    };
    let mut out = Vec::new();
    match v["type"].as_str() {
        Some("assistant") => {
            // The mind's narration is held back rather than printed where it
            // arrives, so it can be placed UNDER the gesture it explains. A
            // turn that narrates once and then acts five times would otherwise
            // leave four gestures unexplained; `explained` hands the sentence
            // to the first, and the rest fall back to what the tool does.
            let mut narration: Option<String> = None;
            let mut acted = false;
            for block in v["message"]["content"].as_array().into_iter().flatten() {
                match block["type"].as_str() {
                    Some("text") => {
                        let text = block["text"].as_str().unwrap_or("");
                        if let Some(line) = text.lines().find(|l| !l.trim().is_empty()) {
                            narration.get_or_insert_with(|| line.trim().to_string());
                        }
                    }
                    Some("tool_use") => {
                        acted = true;
                        let name = block["name"].as_str().unwrap_or("tool");
                        let input = &block["input"];
                        let explanation = narration
                            .take()
                            .unwrap_or_else(|| claude_gesture_of(name, input));
                        // Every arm below pushes its gesture line first, so the
                        // explanation slots in directly beneath it — above any
                        // diff the gesture carries.
                        let at = out.len();
                        match name {
                            "Edit" => {
                                let path = input["file_path"].as_str().unwrap_or("?");
                                out.push(format!(
                                    "   {} {}",
                                    bold(PURPLE(), "\u{00b1} edit"),
                                    bone(path)
                                ));
                                push_block(
                                    &mut out,
                                    input["old_string"].as_str().unwrap_or(""),
                                    '-',
                                    PURPLE(),
                                );
                                push_block(
                                    &mut out,
                                    input["new_string"].as_str().unwrap_or(""),
                                    '+',
                                    GREEN(),
                                );
                            }
                            "Write" => {
                                let path = input["file_path"].as_str().unwrap_or("?");
                                let contents = input["content"].as_str().unwrap_or("");
                                out.push(format!(
                                    "   {} {}  {}",
                                    bold(PURPLE(), "\u{271a} write"),
                                    bone(path),
                                    dim(ASH(), &format!("({} lines)", contents.lines().count())),
                                ));
                                push_block(&mut out, contents, '+', GREEN());
                            }
                            "Bash" => {
                                let cmd = input["command"].as_str().unwrap_or("?");
                                out.push(format!("   {} {}", bold(GREEN(), "$"), bone(cmd)));
                            }
                            "Read" => {
                                let path = input["file_path"].as_str().unwrap_or("?");
                                out.push(format!(
                                    "   {} {}",
                                    fg(GREEN(), "\u{25cb} read"),
                                    dim(ASH(), path)
                                ));
                            }
                            "Grep" | "Glob" => {
                                let pat = input["pattern"].as_str().unwrap_or("?");
                                out.push(format!(
                                    "   {} {}",
                                    fg(GREEN(), "\u{2315} search"),
                                    dim(ASH(), pat)
                                ));
                            }
                            "TodoWrite" => {
                                out.push(format!(
                                    "   {}",
                                    dim(ASH(), "\u{2611} plans its next steps")
                                ));
                            }
                            other => {
                                out.push(format!(
                                    "   {} {}",
                                    fg(GREEN(), "\u{2699}"),
                                    dim(ASH(), other)
                                ));
                            }
                        }
                        if out.len() > at {
                            out.insert(
                                at + 1,
                                format!(
                                    "     {}",
                                    dim(GREEN(), &format!("\u{263d} {explanation}"))
                                ),
                            );
                        }
                    }
                    _ => {}
                }
            }
            // A turn that only spoke is the mind talking to the reader, not an
            // unexplained gesture: it is shown as it always was.
            if !acted {
                if let Some(said) = narration {
                    out.push(format!(
                        "     {}",
                        dim(GREEN(), &format!("\u{263d} {said}"))
                    ));
                }
            }
        }
        Some("user") => {
            // Tool results: stay quiet except for errors — the trace shows actions,
            // the errors show trouble, and everything else is noise.
            for block in v["message"]["content"].as_array().into_iter().flatten() {
                if block["is_error"].as_bool().unwrap_or(false) {
                    let text = block["content"]
                        .as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| {
                            block["content"][0]["text"]
                                .as_str()
                                .unwrap_or("")
                                .to_string()
                        });
                    if let Some(l) = text.lines().find(|l| !l.trim().is_empty()) {
                        out.push(format!(
                            "     {}",
                            fg(PURPLE(), &format!("\u{2192} {}", l.trim()))
                        ));
                    }
                }
            }
        }
        Some("result") => {
            // The agent's closing message is the deliverable — render it WHOLE,
            // every line, unclipped (the TUI wraps long lines itself).
            if let Some(text) = v["result"].as_str() {
                for line in text.lines() {
                    out.push(format!("  {}", bone(line)));
                }
            }
        }
        _ => {} // init/system plumbing — silent
    }
    out
}

#[cfg(not(feature = "api"))]
pub fn claude_event_lines(raw: &str) -> Vec<String> {
    vec![format!("  {raw}")]
}

/// What one of the native agent's gestures IS, in a plain sentence — used when
/// the mind acted without narrating, so no step in the trace is ever left
/// standing on its own. The twin of the `<act>` loop's own `gesture_of`.
#[cfg(feature = "api")]
fn claude_gesture_of(name: &str, input: &serde_json::Value) -> String {
    let field = |key: &str| input[key].as_str().unwrap_or("?").to_string();
    match name {
        "Edit" => format!(
            "changes one exact passage of {} — the diff below is it",
            field("file_path")
        ),
        "Write" => format!(
            "writes {} lines to {}",
            input["content"].as_str().unwrap_or("").lines().count(),
            field("file_path")
        ),
        "Bash" => input["description"]
            .as_str()
            .filter(|d| !d.trim().is_empty())
            .map(|d| d.trim().to_string())
            .unwrap_or_else(|| {
                "runs that command in the project and reads what it prints".to_string()
            }),
        "Read" => format!("reads {} into what it knows", field("file_path")),
        "Grep" => "searches the project for that pattern".to_string(),
        "Glob" => "lists the files whose names match that pattern".to_string(),
        "TodoWrite" => "sets out the steps it means to take".to_string(),
        "WebSearch" => {
            "searches the open web — this returns a ranked list of pages, not an answer".to_string()
        }
        "WebFetch" => "reads that page as text".to_string(),
        "Task" => "hands a piece of this work to a second agent".to_string(),
        other => format!("uses its own {other} tool"),
    }
}

#[cfg(feature = "api")]
fn push_block(out: &mut Vec<String>, text: &str, sign: char, colour: (u8, u8, u8)) {
    use kaos_core::theme::*;
    let lines: Vec<&str> = text.lines().collect();
    for l in &lines {
        out.push(format!("     {}", fg(colour, &format!("{sign} {l}"))));
    }
}

/// Sampling controls for a local ollama completion.
///
/// The audit's finding: the conclave's whole premise is *k diverse samples*, but the
/// bare `ollama run` CLI exposes no `temperature`/`seed`, so diversity was accidental
/// (whatever the server default happened to be) and non-reproducible. `Sampling`
/// makes both explicit — a fixed `temperature` for controlled diversity and a
/// per-sample `seed` so a conclave is diverse *and* reproducible.
#[derive(Clone, Copy, Debug)]
pub struct Sampling {
    pub temperature: f32,
    /// A concrete seed makes the draw reproducible; vary it per conclave member so
    /// the k samples genuinely differ instead of collapsing to one.
    pub seed: Option<u64>,
    /// Let a reasoning model (qwen3, deepseek-r1) think before answering. Off by
    /// default — realbench showed suppression is right for terse Q&A — but an
    /// agent step may benefit from deliberation; devbench measures that.
    pub think: bool,
    /// Constrain the completion to valid JSON (ollama's `format: "json"`). Off by
    /// default. A small model asked for JSON often spends its whole token budget
    /// on a prose preamble and never emits the object; this makes the sampler
    /// only produce JSON, so a caller that needs to parse the reply gets one it
    /// can. Honoured on the HTTP path; the CLI fallback ignores it.
    pub json: bool,
    /// Hard cap on generated tokens for THIS call (ollama's `num_predict`). `None`
    /// falls back to the `KAOS_NUM_PREDICT` env, and then to NO CAP — ollama does
    /// not impose one of its own. A caller that knows its answer is short sets
    /// this so a small model that loops or rambles cannot burn the whole timeout;
    /// leaving it unset is the right default, because what a long reply actually
    /// runs out of is window ([`ANSWER_HEADROOM`]), not permission.
    pub num_predict: Option<i64>,
    /// Context window for THIS call (ollama's `num_ctx`). `None` keeps the
    /// server default — which ollama ships at a small 4096: a long transcript
    /// plus a thinking model's monologue overflows it and generation is cut
    /// mid-sentence. A caller with a growing prompt or a long answer sets this.
    pub num_ctx: Option<i64>,
}

impl Default for Sampling {
    fn default() -> Self {
        // A middling temperature: diverse enough for self-consistency voting to have
        // something to vote on, low enough to stay on-task.
        Sampling {
            temperature: 0.7,
            seed: None,
            think: false,
            json: false,
            num_predict: None,
            num_ctx: None,
        }
    }
}

impl Sampling {
    /// A reproducible draw at the default temperature with an explicit seed.
    pub fn seeded(seed: u64) -> Self {
        Sampling {
            temperature: 0.7,
            seed: Some(seed),
            think: false,
            json: false,
            num_predict: None,
            num_ctx: None,
        }
    }

    /// The same draw with reasoning enabled.
    pub fn thinking(mut self) -> Self {
        self.think = true;
        self
    }

    /// The same draw constrained to emit valid JSON.
    pub fn json(mut self) -> Self {
        self.json = true;
        self
    }

    /// The same draw with a hard token cap, so a short answer can't run away.
    pub fn capped(mut self, tokens: i64) -> Self {
        self.num_predict = Some(tokens);
        self
    }

    /// The same draw with an explicit context window (ollama's `num_ctx`), for
    /// calls whose transcript or answer outgrows the server's small default.
    pub fn context(mut self, tokens: i64) -> Self {
        self.num_ctx = Some(tokens);
        self
    }
}

/// Run a prompt through ollama with *explicit* sampling control (temperature + seed).
///
/// Prefers the HTTP `/api/generate` endpoint (built with the `api` feature), which is
/// the only way to actually pin `temperature`/`seed` — `ollama run` on the CLI cannot.
/// If HTTP is unavailable (feature off, or the call fails) it falls back to the CLI
/// path, which ignores the sampling knobs but still returns a completion. `OLLAMA_HOST`
/// overrides the endpoint (default `http://127.0.0.1:11434`).
pub fn ollama_generate(
    model: &str,
    prompt: &str,
    timeout: Duration,
    sampling: Sampling,
) -> Result<String, String> {
    ollama_generate_attached(model, prompt, &[], timeout, sampling)
}

/// A local completion carrying images, for a vision model.
///
/// The HTTP path only: the `ollama run` CLI fallback takes a prompt on stdin
/// and has nowhere to put a picture, so a request with files that cannot reach
/// the endpoint fails rather than silently answering about nothing.
///
/// # Errors
///
/// Whatever the endpoint returns, or a message when files are present and the
/// endpoint is unreachable.
pub fn ollama_generate_attached(
    model: &str,
    prompt: &str,
    files: &[rebis_lang::Attachment],
    timeout: Duration,
    sampling: Sampling,
) -> Result<String, String> {
    #[cfg(feature = "api")]
    if !files.is_empty() {
        return ollama_http_attached(model, prompt, files, timeout, sampling);
    }
    let _ = files;
    ollama_generate_text(model, prompt, timeout, sampling)
}

/// The original text-only path, unchanged.
fn ollama_generate_text(
    model: &str,
    prompt: &str,
    timeout: Duration,
    sampling: Sampling,
) -> Result<String, String> {
    #[cfg(feature = "api")]
    {
        match ollama_http(model, prompt, timeout, sampling) {
            Ok(text) => Ok(text),
            // The server may be down or the endpoint absent on an old ollama; the CLI
            // path below still works via `ollama run`, so degrade rather than fail.
            // Preserve the HTTP error if the fallback fails too: otherwise a
            // remote model problem becomes an opaque `exit 1`.
            //
            // Only when the ENDPOINT failed. A model that answered badly —
            // spent its budget reasoning, returned nothing — has already been
            // paid for, and `ollama run` cannot do better: it ignores this
            // call's sampling (its cap, its seed, its `think` setting) and
            // charges a second full generation to say the same thing. Worse,
            // it says it as raw monologue, which the agent loop then cannot
            // read as an action. Let the model's own verdict stand.
            Err(http_error) if endpoint_failed(&http_error) => {
                ollama_complete(model, prompt, timeout)
                    .map_err(|cli_error| format!("{cli_error}; HTTP attempt: {http_error}"))
            }
            Err(model_error) => Err(model_error),
        }
    }
    #[cfg(not(feature = "api"))]
    {
        let _ = sampling; // honoured only on the HTTP path
        ollama_complete(model, prompt, timeout)
    }
}

/// The largest window this build will ask ollama to open by itself. A window is
/// KV cache — real memory on the machine serving it — so a prompt that outgrows
/// even this gets the honest truncation error instead of an allocation nobody
/// asked for. `KAOS_NUM_CTX` overrides the whole calculation in either
/// direction for someone who knows their hardware.
#[cfg(feature = "api")]
const MAX_FITTED_CONTEXT: i64 = 65_536;

/// Room kept for the model's own reply inside the fitted window.
///
/// This — not `num_predict` — is what actually bounds a local generation here.
/// Left empty, ollama imposes no token cap of its own (measured: a request with
/// no `num_predict` ran to 1492 tokens and stopped because the model was
/// finished, not because it was cut). What stops it early is running out of
/// WINDOW, which the prompt and the reply share. So the reply's share is
/// reserved up front, and it is sized for a reasoning model: the monologue is
/// generated tokens too, and 2k left one barely enough to think, let alone
/// think and then answer.
#[cfg(feature = "api")]
const ANSWER_HEADROOM: i64 = 8_192;

/// The context window to ask for, given what THIS prompt needs.
///
/// ollama's own default is 4096 tokens, and it does not fail when a prompt
/// exceeds it — it silently drops the front and generates in what is left. For
/// a one-shot completion that is survivable. For an agent it is fatal and
/// invisible: the transcript grows every turn, crosses the line, and from then
/// on the mind is answering a truncated question with no room to finish, so its
/// reply arrives cut mid-thought and holds no executable action. That is a
/// context failure wearing the mask of a model that "won't follow the format".
///
/// So the window is named rather than inherited — but it is named with the SAME
/// number every time, and that stability is the whole design.
///
/// ollama keys a resident model on its options: change `num_ctx` and it evicts
/// the model and loads it again. A window sized to each prompt therefore looks
/// reasonable and behaves terribly — every turn whose transcript crosses a
/// threshold pays a full reload of a multi-gigabyte model, which on a remote
/// box reads as `timed out reading response` and looks like the network. So one
/// window is chosen up front, big enough for an agent's whole conversation, and
/// held: the model is loaded once and stays.
///
/// A prompt that outgrows even [`MAX_FITTED_CONTEXT`] gets the honest truncation
/// error rather than a bespoke allocation. `KAOS_NUM_CTX` (or an explicit
/// `num_ctx` from the caller) replaces the choice entirely, for someone who
/// knows what their hardware will hold.
#[cfg(feature = "api")]
pub(crate) fn context_window(explicit: Option<i64>, prompt_chars: usize) -> Option<i64> {
    if let Some(n) = explicit {
        return Some(n);
    }
    if let Some(n) = std::env::var("KAOS_NUM_CTX")
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|n| *n > 0)
    {
        return Some(n);
    }
    // ~4 chars per token is the rough English/code ratio. Only a prompt that
    // genuinely will not fit the standing window moves off it — one step, to
    // the ceiling, so there are at most two windows a session can ask for
    // instead of a ladder of reloads.
    let needed = (prompt_chars as i64 / 4) + ANSWER_HEADROOM;
    Some(if needed > FITTED_CONTEXT {
        MAX_FITTED_CONTEXT
    } else {
        FITTED_CONTEXT
    })
}

/// The standing window every local call asks for. Large enough to hold an agent
/// transcript plus [`ANSWER_HEADROOM`] without ever being renegotiated.
#[cfg(feature = "api")]
const FITTED_CONTEXT: i64 = 32_768;

/// Did the ENDPOINT fail, as opposed to the model failing at it?
///
/// The `ollama run` fallback exists for one case: an ollama that cannot serve
/// this request over HTTP (down, too old to know the endpoint, answering
/// something that is not JSON). Everything else in the error vocabulary of
/// [`ollama_http`] is a verdict on the completion itself and must not be paid
/// for twice. A timeout is on this side of the line too: the same model on the
/// same box, asked the same question through a subprocess, is not faster.
#[cfg(feature = "api")]
fn endpoint_failed(error: &str) -> bool {
    let slow = error.contains("timed out") || error.contains("timeout");
    (error.starts_with("ollama http:") || error.starts_with("ollama: bad json:")) && !slow
}

/// The HTTP path to ollama (`/api/generate`), used directly by callers that must
/// NOT fall back to spawning an `ollama run` subprocess — a degraded server then
/// returns its real error instead of silently degrading to the CLI. This is also
/// the preferred inner path of [`ollama_generate`].
#[cfg(feature = "api")]
pub fn ollama_http(
    model: &str,
    prompt: &str,
    timeout: Duration,
    sampling: Sampling,
) -> Result<String, String> {
    ollama_http_attached(model, prompt, &[], timeout, sampling)
}

/// The real builder: `ollama_http` is this with nothing attached.
///
/// # Errors
///
/// Whatever the endpoint returns.
#[cfg(feature = "api")]
pub fn ollama_http_attached(
    model: &str,
    prompt: &str,
    files: &[rebis_lang::Attachment],
    timeout: Duration,
    sampling: Sampling,
) -> Result<String, String> {
    let (model, selector_host) = match model.rsplit_once('\\') {
        Some((name, host)) if !host.is_empty() => (name, Some(host.to_string())),
        _ => (model, None),
    };
    let host = selector_host
        .or_else(crate::provider::configured_ollama_host)
        .unwrap_or_else(|| "http://127.0.0.1:11434".into());
    // Accept a bare host:port (as `OLLAMA_HOST` is often set) by adding a scheme.
    let base = if host.starts_with("http") {
        host
    } else {
        format!("http://{host}")
    };
    let mut options = serde_json::json!({ "temperature": sampling.temperature });
    if let Some(seed) = sampling.seed {
        // ollama takes an i64/i32 seed; fold u64 into the positive i32 range.
        options["seed"] = serde_json::json!((seed % (i32::MAX as u64)) as i64);
    }
    // Opt-in generation cap: a runaway local model (a reasoning model that won't
    // stop, on slow CPU) can otherwise burn the whole wall clock producing tokens
    // no one reads. A per-call `Sampling::capped` wins; else `KAOS_NUM_PREDICT`;
    // unset ⇒ ollama's own default.
    let num_predict = sampling.num_predict.or_else(|| {
        std::env::var("KAOS_NUM_PREDICT")
            .ok()
            .and_then(|v| v.trim().parse::<i64>().ok())
    });
    if let Some(n) = num_predict {
        options["num_predict"] = serde_json::json!(n);
    }
    if let Some(n) = context_window(sampling.num_ctx, prompt.len()) {
        options["num_ctx"] = serde_json::json!(n);
    }
    let mut body = serde_json::json!({
        "model": model,
        "prompt": prompt,
        "stream": false,
        "think": sampling.think, // reasoning at the source: off for terse Q&A, on when asked
        "options": options,
    });
    // This endpoint keeps the prompt a flat string and takes pictures beside
    // it. See `crate::attach` for why the shape lives there and not here.
    let images = crate::attach::Wire::Ollama.images(files);
    if !images.is_empty() {
        body["images"] = serde_json::json!(images);
    }
    // Structured output, when the caller asked for it: constrain the sampler to
    // valid JSON so a model can't burn its budget on a prose preamble.
    if sampling.json {
        body["format"] = serde_json::json!("json");
    }
    let resp = ureq::agent()
        .post(&format!("{base}/api/generate"))
        .timeout(timeout)
        .send_json(body)
        .map_err(|e| format!("ollama http: {e}"))?;
    let v: serde_json::Value = resp
        .into_json()
        .map_err(|e| format!("ollama: bad json: {e}"))?;
    let text = v["response"].as_str().unwrap_or_default();
    if text.is_empty() {
        // With `think: true` ollama routes the monologue to a separate field;
        // if that is all that came back, the token budget died deliberating.
        if !v["thinking"].as_str().unwrap_or_default().is_empty() {
            return Err(
                "ollama: the model spent its whole token budget thinking and never answered — \
                 raise the cap (num_predict) or shorten the prompt"
                    .into(),
            );
        }
        return Err(format!("ollama: empty response: {v}"));
    }
    let clean = strip_think(text).trim().to_string();
    if clean.is_empty() {
        // The reply existed but was reasoning wall-to-wall (an unclosed
        // <think>, or a monologue cut by the cap). Surface that instead of
        // handing the caller an empty string it will mistake for an answer.
        return Err("ollama: the reply was all reasoning, cut before any answer".into());
    }
    Ok(clean)
}

/// Run a raw prompt through a local `ollama` model with a hard timeout, returning
/// the (think-stripped) completion. A reader thread drains stdout so a verbose
/// model can never fill the pipe and deadlock. A hosted Rebis run cooperatively
/// pauses the whole process group at its time boundary and can continue the same
/// local generation; one-shot callers retain the hard-kill timeout.
/// This is the primitive the real benchmark and `fire_ollama` are built on.
pub fn ollama_complete(model: &str, prompt: &str, timeout: Duration) -> Result<String, String> {
    // A selector may name the machine that serves it —
    // `llama3.2:3b\\192.168.1.20:11434` — so one Kaos reaches a rack of boxes
    // with different models on them. Without it there is a single global
    // `OLLAMA_HOST` and every model has to live on the same server.
    //
    // The host is handed to the child through its environment rather than a
    // flag, because that is what the `ollama` CLI reads. The model name it is
    // asked for never carries the host.
    let (model, selector_host) = match model.rsplit_once('\\') {
        Some((name, host)) if !host.is_empty() => (name, Some(host.to_string())),
        _ => (model, None),
    };
    let host = selector_host.or_else(crate::provider::configured_ollama_host);
    let mut command = Command::new("ollama");
    command.arg("run").arg(model).arg(prompt);
    if let Some(host) = &host {
        command.env("OLLAMA_HOST", host);
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not summon `ollama`: {e}"))?;

    let mut stdout = child.stdout.take().ok_or("no stdout handle from ollama")?;
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = std::io::Read::read_to_string(&mut stdout, &mut s);
        s
    });
    let mut stderr = child.stderr.take().ok_or("no stderr handle from ollama")?;
    let error_reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = std::io::Read::read_to_string(&mut stderr, &mut s);
        s
    });

    let mut start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) => {
                if start.elapsed() > timeout {
                    let reason = format!("model time limit ({}s) reached", timeout.as_secs());
                    if crate::pause::current_run(&reason) {
                        // SIGCONT grants another time slice to the same Ollama
                        // child and reader; neither is recreated or discarded.
                        start = Instant::now();
                        continue;
                    }
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = reader.join();
                    let _ = error_reader.join();
                    return Err(format!("charge timed out after {}s", timeout.as_secs()));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                let _ = error_reader.join();
                return Err(format!("charge error: {e}"));
            }
        }
    };

    let raw = reader
        .join()
        .map_err(|_| "reader thread panicked".to_string())?;
    let stderr = error_reader
        .join()
        .map_err(|_| "stderr reader thread panicked".to_string())?;
    if status.success() {
        let clean = strip_think(&raw).trim().to_string();
        // Same law as the HTTP path: a reply that is monologue wall to wall —
        // a thought the model never closed because generation stopped inside
        // it — is not an answer, and handing it up as one gives every caller
        // above a wall of reasoning where it expected a result (an agent loop
        // reads no action in it and banishes the turn).
        if clean.is_empty() && !raw.trim().is_empty() {
            return Err("ollama: the reply was all reasoning, cut before any answer".into());
        }
        Ok(clean)
    } else {
        let detail = stderr.trim().trim_start_matches("Error:").trim();
        let detail = if detail.is_empty() {
            raw.trim()
        } else {
            detail
        };
        let detail = detail.chars().take(500).collect::<String>();
        let host = host.as_deref().unwrap_or("http://127.0.0.1:11434");
        if detail.is_empty() {
            Err(format!(
                "charge fizzled (exit {}) — `{model}` on {host}; is ollama reachable there and does it have that model?",
                status.code().unwrap_or(-1)
            ))
        } else {
            Err(format!(
                "charge fizzled (exit {}) — `{model}` on {host}: {detail}",
                status.code().unwrap_or(-1)
            ))
        }
    }
}

/// Strip `<think>…</think>` reasoning blocks some local models (qwen3, deepseek-r1)
/// emit, leaving only the charged result. Tolerant of an unclosed trailing block,
/// of a monologue that only CLOSES its block (qwen3-2507 templates bake the
/// opening tag into the prompt, so the reply starts mid-thought and only
/// `</think>` appears), and of the `ollama run` CLI's rendering, which brackets
/// the monologue with `Thinking...` / `...done thinking.` instead of tags.
pub fn strip_think(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("<think>") {
        out.push_str(&rest[..start]);
        match rest[start..].find("</think>") {
            Some(end) => rest = &rest[start + end + "</think>".len()..],
            None => {
                rest = ""; // unclosed block — drop the remainder
                break;
            }
        }
    }
    out.push_str(rest);
    // Any closer still present had no opening tag: everything before it is
    // monologue. Keep only what follows the LAST closer of each kind.
    for marker in ["</think>", "...done thinking.", "…done thinking."] {
        if let Some(i) = out.rfind(marker) {
            out = out[i + marker.len()..].to_string();
        }
    }
    // An OPENER with no closer left, in the CLI's own rendering: the model was
    // still thinking when generation stopped, so every word after it is
    // monologue. Dropped for the same reason an unclosed `<think>` is — what
    // survives must be answer, never reasoning.
    for opener in ["Thinking...", "Thinking…"] {
        if out.trim_start().starts_with(opener) {
            out = String::new();
        }
    }
    out
}

/// The system prompt that puts the executor *in character* as a sworn adept of the
/// Pact — a small, real piece of prompt engineering: persona + constraint to keep
/// the reply terse, which is the charged-sigil discipline carried into the model.
pub fn adept_system_prompt(adept_name: &str, ray_name: &str, ray_sphere: &str) -> String {
    format!(
        "You are {adept_name}, a sworn adept of the Pact, working \
         the {ray_name} ray ({ray_sphere}). Reason through the problem step by step, \
         showing your working and naming each intermediate quantity. Then end with a \
         final line of the exact form 'ANSWER: <result>' giving only the single number \
         or word, with nothing after it."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kaos_session_ids_map_to_a_stable_valid_uuid() {
        let raw = "1737590000123-45678"; // the `{millis}-{pid}` shape kaos mints
        let uuid = claude_session_uuid(raw);
        // Well-formed UUID: 8-4-4-4-12 hex, version 4, RFC 4122 variant.
        let parts: Vec<&str> = uuid.split('-').collect();
        assert_eq!(
            parts.iter().map(|p| p.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12]
        );
        assert!(uuid.chars().all(|c| c == '-' || c.is_ascii_hexdigit()));
        assert_eq!(parts[2].as_bytes()[0], b'4', "version 4");
        assert!(matches!(parts[3].as_bytes()[0], b'8' | b'9' | b'a' | b'b'));
        // Deterministic, and an already-UUID input is passed through.
        assert_eq!(uuid, claude_session_uuid(raw));
        let already = "550e8400-e29b-41d4-a716-446655440000";
        assert_eq!(claude_session_uuid(already), already);
    }

    #[test]
    fn claude_completion_permissions_follow_the_single_kaos_decision() {
        assert_eq!(
            claude_completion_permission_args(true),
            ["--dangerously-skip-permissions"]
        );
        let normal = claude_completion_permission_args(false);
        assert!(normal.windows(2).any(|pair| pair == ["--tools", ""]));
        assert!(normal
            .windows(2)
            .any(|pair| pair == ["--permission-mode", "dontAsk"]));
    }

    #[test]
    fn strip_think_removes_reasoning() {
        assert_eq!(strip_think("<think>pondering</think>the work"), "the work");
        assert_eq!(strip_think("before<think>x</think> after"), "before after");
        assert_eq!(strip_think("no tags here"), "no tags here");
        // qwen3-2507 style: the opening tag lives in the prompt template, so the
        // reply is bare monologue closed by a stray </think>.
        assert_eq!(
            strip_think("Okay, let me reason...\n</think>\nThe answer."),
            "\nThe answer."
        );
        // `ollama run` CLI rendering of the same monologue.
        assert_eq!(
            strip_think("Thinking...\nweighing it\n...done thinking.\n\nFinal."),
            "\n\nFinal."
        );
        // Unclosed block: drop the dangling remainder.
        assert_eq!(strip_think("done<think>still musing"), "done");
        // The CLI's opener with no closer: generation stopped inside the
        // thought, so there is no answer in it at all.
        assert_eq!(
            strip_think("Thinking...\nI should read the file next, then decide"),
            ""
        );
    }

    #[cfg(feature = "api")]
    #[test]
    fn the_window_is_fitted_to_the_prompt_that_must_fit_in_it() {
        // ONE window, whatever the prompt: changing it makes ollama evict and
        // reload the model, so a per-prompt size would pay a multi-gigabyte
        // reload every time a transcript crossed a threshold.
        assert_eq!(context_window(None, 0), Some(FITTED_CONTEXT));
        assert_eq!(context_window(None, 4_000), Some(FITTED_CONTEXT));
        assert_eq!(context_window(None, 40_000), Some(FITTED_CONTEXT));
        // Only a prompt that genuinely will not fit moves, and it moves once —
        // straight to the ceiling, never up a ladder.
        assert_eq!(context_window(None, 4_000_000), Some(MAX_FITTED_CONTEXT));
        // Every size a session can ask for, across a full sweep of prompt
        // sizes: two, so a conversation reloads the model at most once.
        let asked: std::collections::BTreeSet<Option<i64>> = (0..400)
            .map(|step| context_window(None, step * 2_000))
            .collect();
        assert!(asked.len() <= 2, "a sweep asked for {asked:?}");
        // An explicit window is obeyed exactly, in either direction.
        assert_eq!(context_window(Some(2_048), 4_000_000), Some(2_048));
        assert_eq!(context_window(Some(262_144), 10), Some(262_144));
    }

    #[cfg(feature = "api")]
    #[test]
    fn only_a_failed_endpoint_falls_back_to_the_cli() {
        // The server could not serve the request: `ollama run` may still work.
        assert!(endpoint_failed("ollama http: connection refused"));
        assert!(endpoint_failed("ollama: bad json: expected value"));
        // The model answered, badly. Running it again through a subprocess
        // costs a second full generation and ignores this call's sampling.
        assert!(!endpoint_failed(
            "ollama: the model spent its whole token budget thinking and never answered — \
             raise the cap (num_predict) or shorten the prompt"
        ));
        assert!(!endpoint_failed(
            "ollama: the reply was all reasoning, cut before any answer"
        ));
        assert!(!endpoint_failed("ollama: empty response: {}"));
        // Slow is slow on either path.
        assert!(!endpoint_failed("ollama http: request timed out"));
    }

    #[test]
    fn system_prompt_names_the_adept_and_ray() {
        let p = adept_system_prompt("Frater Stokastikos", "Red", "war & vitality");
        assert!(p.contains("Frater Stokastikos"));
        assert!(p.contains("Red"));
    }
    #[cfg(feature = "api")]
    #[test]
    fn claude_events_render_as_live_trace() {
        // The stream-json feed becomes the same live language as the conductor:
        // remarks -> narration, Edit -> a diff, Bash -> $, results only on error.
        let text = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Fixing the inverted filter now."}]}}"#;
        let lines = claude_event_lines(text);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("Fixing the inverted filter"));

        // Every gesture carries a line UNDER it saying what it is. Unnarrated,
        // that line is what the tool does — no step stands unexplained.
        let edit = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Edit","input":{"file_path":"/a/b/todo/query.py","old_string":"if done","new_string":"if not done"}}]}}"#;
        let lines = claude_event_lines(edit);
        assert!(lines[0].contains("edit") && lines[0].contains("todo/query.py"));
        assert!(lines[1].contains("changes one exact passage"));
        assert!(lines[2].contains("- if done"));
        assert!(lines[3].contains("+ if not done"));

        let bash = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"python3 tests.py"}}]}}"#;
        let lines = claude_event_lines(bash);
        assert!(lines[0].contains("python3 tests.py"));
        assert!(lines[1].contains("runs that command"));

        // When the mind DID narrate, its own words explain the gesture — and
        // they sit under it, not above.
        let narrated = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Checking the tests still pass."},{"type":"tool_use","name":"Bash","input":{"command":"pytest"}}]}}"#;
        let lines = claude_event_lines(narrated);
        assert!(lines[0].contains("pytest"));
        assert!(lines[1].contains("Checking the tests still pass"));
        assert_eq!(lines.len(), 2, "the narration is not repeated above");

        // One sentence, five gestures: the rest explain themselves rather than
        // echoing it or standing bare.
        let chain = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Reading both files."},{"type":"tool_use","name":"Read","input":{"file_path":"a.py"}},{"type":"tool_use","name":"Read","input":{"file_path":"b.py"}}]}}"#;
        let lines = claude_event_lines(chain);
        assert!(lines[1].contains("Reading both files"));
        assert!(lines[3].contains("reads b.py into what it knows"));

        // Plumbing stays silent; garbage passes through.
        assert!(claude_event_lines(r#"{"type":"system","subtype":"init"}"#).is_empty());
        assert_eq!(claude_event_lines("not json").len(), 1);
    }
}
