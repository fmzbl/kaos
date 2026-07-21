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

/// Fire a charged intent at the `claude` CLI in non-interactive mode. The charged
/// intent is the *banished* prompt — the verbose statement of intent has already
/// been lost to the mind; only the compressed imperative is sent. Returns the
/// model's reply, or an error string if the CLI is unavailable.
pub fn fire_claude(charged_intent: &str, system: &str) -> Result<String, String> {
    fire_claude_as(None, charged_intent, system)
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
    // The native `claude` agent runs the `/chat` mind with its own tools and
    // loop, so `--append-system-prompt` is the only seam through which it can
    // learn the Rebis language Kaos hosts. Gate the cookbook on the same
    // predicate the `<act>` loop uses (the TUI sets KAOS_REBIS_CONTEXT for
    // chat; a bare `/code` opts in when its task names Rebis).
    let appendix = crate::conductor::wants_rebis_authoring_context(task)
        .then(crate::conductor::claude_agent_rebis_appendix);
    run_claude_agent_inner(root, task, model, true, appendix.as_deref(), emit).map(|_| ())
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
    // Extra doctrine appended to Claude's own system prompt (e.g. the Rebis
    // cookbook for the chat mind). System-level, not part of the task message,
    // so a resumed multi-turn session carries it without bloating history.
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
    // via KAOS_SESSION (a UUID). The first turn CREATES it (--session-id); later
    // turns RESUME it (--resume KAOS_RESUME=1), so claude keeps the full history.
    if persist_session {
        if let Some(sid) = std::env::var("KAOS_SESSION").ok().filter(|s| !s.is_empty()) {
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
    let _ = err_reader.map(|h| h.join());
    let status = child
        .wait()
        .map_err(|e| format!("the Work was interrupted: {e}"))?;
    if let Some(Err(error)) = input_result {
        return Err(error);
    }
    if status.success() {
        Ok(final_result)
    } else {
        Err(format!(
            "claude exited with {}",
            status.code().unwrap_or(-1)
        ))
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
            for block in v["message"]["content"].as_array().into_iter().flatten() {
                match block["type"].as_str() {
                    Some("text") => {
                        for line in block["text"]
                            .as_str()
                            .unwrap_or("")
                            .lines()
                            .filter(|l| !l.trim().is_empty())
                            .take(3)
                        {
                            out.push(format!(
                                "     {}",
                                dim(
                                    (150, 130, 200),
                                    &format!("\u{263d} {}", clip(line.trim(), 92))
                                )
                            ));
                        }
                    }
                    Some("tool_use") => {
                        let name = block["name"].as_str().unwrap_or("tool");
                        let input = &block["input"];
                        match name {
                            "Edit" => {
                                let path = input["file_path"].as_str().unwrap_or("?");
                                out.push(format!(
                                    "   {} {}",
                                    bold((220, 170, 60), "\u{00b1} edit"),
                                    bone(short(path))
                                ));
                                push_block(
                                    &mut out,
                                    input["old_string"].as_str().unwrap_or(""),
                                    '-',
                                    (200, 90, 90),
                                );
                                push_block(
                                    &mut out,
                                    input["new_string"].as_str().unwrap_or(""),
                                    '+',
                                    (90, 200, 110),
                                );
                            }
                            "Write" => {
                                let path = input["file_path"].as_str().unwrap_or("?");
                                let contents = input["content"].as_str().unwrap_or("");
                                out.push(format!(
                                    "   {} {}  {}",
                                    bold((220, 170, 60), "\u{271a} write"),
                                    bone(short(path)),
                                    dim(ASH(), &format!("({} lines)", contents.lines().count())),
                                ));
                                push_block(&mut out, contents, '+', (90, 200, 110));
                            }
                            "Bash" => {
                                let cmd = input["command"].as_str().unwrap_or("?");
                                out.push(format!(
                                    "   {} {}",
                                    bold(RED(), "$"),
                                    bone(&clip(cmd, 88))
                                ));
                            }
                            "Read" => {
                                let path = input["file_path"].as_str().unwrap_or("?");
                                out.push(format!(
                                    "   {} {}",
                                    fg((150, 130, 200), "\u{25cb} read"),
                                    dim(ASH(), short(path))
                                ));
                            }
                            "Grep" | "Glob" => {
                                let pat = input["pattern"].as_str().unwrap_or("?");
                                out.push(format!(
                                    "   {} {}",
                                    fg((150, 130, 200), "\u{2315} search"),
                                    dim(ASH(), &clip(pat, 80))
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
                                    fg((150, 130, 200), "\u{2699}"),
                                    dim(ASH(), other)
                                ));
                            }
                        }
                    }
                    _ => {}
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
                            fg((200, 90, 90), &format!("\u{2192} {}", clip(l.trim(), 88)))
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

#[cfg(feature = "api")]
fn push_block(out: &mut Vec<String>, text: &str, sign: char, colour: (u8, u8, u8)) {
    use kaos_core::theme::*;
    const SHOWN: usize = 6;
    let lines: Vec<&str> = text.lines().collect();
    for l in lines.iter().take(SHOWN) {
        out.push(format!(
            "     {}",
            fg(colour, &format!("{sign} {}", clip(l, 88)))
        ));
    }
    if lines.len() > SHOWN {
        out.push(format!(
            "     {}",
            dim(
                ASH(),
                &format!("{sign} \u{2026} {} more lines", lines.len() - SHOWN)
            )
        ));
    }
}

/// Last two path components — enough to recognise a file without the noise.
#[cfg(feature = "api")]
fn short(path: &str) -> &str {
    let mut idx = 0;
    let mut seen = 0;
    for (i, c) in path.char_indices().rev() {
        if c == '/' {
            seen += 1;
            if seen == 2 {
                idx = i + 1;
                break;
            }
        }
    }
    &path[idx..]
}

#[cfg(feature = "api")]
fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(n.saturating_sub(1)).collect();
        t.push('\u{2026}');
        t
    }
}

/// Fire a charged intent at a local `ollama` model. The `ollama run` CLI takes a
/// single prompt and has no system flag, so the adept's persona is prepended as a
/// preamble — the charged intent still follows last, where it carries most weight.
/// Stays zero-dependency (no HTTP crate): we shell out exactly as for `claude`.
/// qwen3-style `<think>…</think>` reasoning is stripped so only the work remains.
pub fn fire_ollama(model: &str, charged_intent: &str, system: &str) -> Result<String, String> {
    let prompt = format!("{system}\n\nCHARGED SIGIL: {charged_intent}");
    // A generous ceiling for interactive use; the benchmark sets its own.
    ollama_complete(model, &prompt, Duration::from_secs(300))
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
    /// falls back to the `KAOS_NUM_PREDICT` env, then ollama's default. A caller
    /// that knows its answer is short sets this so a small model that loops or
    /// rambles cannot burn the whole timeout.
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
    #[cfg(feature = "api")]
    {
        match ollama_http(model, prompt, timeout, sampling) {
            Ok(text) => return Ok(text),
            // The server may be down or the endpoint absent on an old ollama; the CLI
            // path below still works via `ollama run`, so degrade rather than fail.
            Err(_e) => {}
        }
    }
    let _ = sampling; // honoured only on the HTTP path
    ollama_complete(model, prompt, timeout)
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
    let host = std::env::var("OLLAMA_HOST").unwrap_or_else(|_| "http://127.0.0.1:11434".into());
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
    if let Some(n) = sampling.num_ctx {
        options["num_ctx"] = serde_json::json!(n);
    }
    let mut body = serde_json::json!({
        "model": model,
        "prompt": prompt,
        "stream": false,
        "think": sampling.think, // reasoning at the source: off for terse Q&A, on when asked
        "options": options,
    });
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
    let mut child = Command::new("ollama")
        .arg("run")
        .arg(model)
        .arg(prompt)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null()) // ollama writes load/progress noise here; ignore it
        .spawn()
        .map_err(|e| format!("could not summon `ollama`: {e}"))?;

    let mut stdout = child.stdout.take().ok_or("no stdout handle from ollama")?;
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = std::io::Read::read_to_string(&mut stdout, &mut s);
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
                    return Err(format!("charge timed out after {}s", timeout.as_secs()));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(format!("charge error: {e}")),
        }
    };

    let raw = reader
        .join()
        .map_err(|_| "reader thread panicked".to_string())?;
    if status.success() {
        Ok(strip_think(&raw).trim().to_string())
    } else {
        Err(format!(
            "charge fizzled (exit {})",
            status.code().unwrap_or(-1)
        ))
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

        let edit = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Edit","input":{"file_path":"/a/b/todo/query.py","old_string":"if done","new_string":"if not done"}}]}}"#;
        let lines = claude_event_lines(edit);
        assert!(lines[0].contains("edit") && lines[0].contains("todo/query.py"));
        assert!(lines[1].contains("- if done"));
        assert!(lines[2].contains("+ if not done"));

        let bash = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"python3 tests.py"}}]}}"#;
        assert!(claude_event_lines(bash)[0].contains("python3 tests.py"));

        // Plumbing stays silent; garbage passes through.
        assert!(claude_event_lines(r#"{"type":"system","subtype":"init"}"#).is_empty());
        assert_eq!(claude_event_lines("not json").len(), 1);
    }
}
