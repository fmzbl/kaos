//! The Conductor — a real tool-using agentic loop, from scratch.
//!
//! Everything else in kaos is one-shot: show the model a file, take back a
//! patch. That is not what a coding agent is. A coding agent **reasons, calls tools,
//! observes the results, and iterates** until the work is done and verified. This
//! module implements exactly that loop, model-agnostic and zero-dependency:
//!
//! ```text
//!   loop:
//!     reply   = model(system + transcript)      // the adept thinks
//!     action  = parse one <act> from reply       // it chooses a tool
//!     if finish: stop
//!     obs     = execute(action) in the workspace // read/write/edit/bash
//!     transcript += reply + observation           // it sees the result, continues
//! ```
//!
//! Tools: `read_file`, `write_file`, `edit_file`, `bash`, `finish` — with `bash` the
//! agent can grep, list, run the tests, anything. The model is behind the [`Chat`]
//! seam so the same loop runs on claude, a local ollama model, or a scripted stub
//! (deterministic, for tests): an adept performing the Great Work, tool by tool.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::agent::{self, Workspace};

// ───────────────────────────── tools ───────────────────────────────

/// One action the agent can take in a turn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tool {
    ReadFile {
        path: String,
    },
    WriteFile {
        path: String,
        contents: String,
    },
    EditFile {
        path: String,
        find: String,
        replace: String,
    },
    Bash {
        cmd: String,
    },
    Finish {
        message: String,
    },
}

impl Tool {
    /// A one-line human description for the trace.
    pub fn describe(&self) -> String {
        match self {
            Tool::ReadFile { path } => format!("read_file {path}"),
            Tool::WriteFile { path, contents } => {
                format!("write_file {path} ({} bytes)", contents.len())
            }
            Tool::EditFile { path, find, .. } => {
                format!("edit_file {path} (find {:?})", trunc(find, 40))
            }
            Tool::Bash { cmd } => format!("bash: {}", trunc(cmd, 60)),
            Tool::Finish { .. } => "finish".to_string(),
        }
    }
}

/// Parse exactly one `<act tool="…">…</act>` block from a model reply.
///
/// Format (robust to surrounding prose):
/// ```text
/// <act tool="edit_file">
/// <arg name="path">sol.py</arg>
/// <arg name="find">return a - b</arg>
/// <arg name="replace">return a + b</arg>
/// </act>
/// ```
/// Parse EVERY `<act>` block in a reply, in order — a chain of sigils cast in
/// one breath. Caps at `limit` so a runaway reply cannot flood the executor.
pub fn parse_actions(text: &str, limit: usize) -> Vec<Tool> {
    let mut tools = Vec::new();
    let mut rest = text;
    while tools.len() < limit {
        let Some(start) = rest.find("<act") else {
            break;
        };
        let after = &rest[start..];
        let end = after
            .find("</act>")
            .map(|i| start + i + "</act>".len())
            .unwrap_or(rest.len());
        if let Some(tool) = parse_action(&rest[start..end]) {
            tools.push(tool);
        }
        rest = &rest[end..];
    }
    tools
}

pub fn parse_action(text: &str) -> Option<Tool> {
    let start = text.find("<act")?;
    let open_end = text[start..].find('>')? + start;
    let header = &text[start..open_end];
    let tool = attr(header, "tool")?;
    let block_end = text[open_end..]
        .find("</act>")
        .map(|i| i + open_end)
        .unwrap_or(text.len());
    let body = &text[open_end + 1..block_end];
    let args = parse_args(body);
    let get = |keys: &[&str]| -> String {
        for k in keys {
            if let Some(v) = args.get(*k) {
                return v.clone();
            }
        }
        String::new()
    };
    match tool.as_str() {
        "read_file" | "read" => Some(Tool::ReadFile {
            path: get(&["path", "file"]),
        }),
        "write_file" | "write" => Some(Tool::WriteFile {
            path: get(&["path", "file"]),
            contents: get(&["contents", "content", "body"]),
        }),
        "edit_file" | "edit" => Some(Tool::EditFile {
            path: get(&["path", "file"]),
            find: get(&["find", "old", "search"]),
            replace: get(&["replace", "new", "with"]),
        }),
        "bash" | "run" | "shell" => Some(Tool::Bash {
            cmd: get(&["cmd", "command"]),
        }),
        "finish" | "done" | "stop" => Some(Tool::Finish {
            message: get(&["message", "msg", "summary"]),
        }),
        _ => None,
    }
}

fn attr(header: &str, name: &str) -> Option<String> {
    let pat = format!("{name}=\"");
    let a = header.find(&pat)? + pat.len();
    let b = header[a..].find('"')? + a;
    Some(header[a..b].to_string())
}

fn parse_args(body: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    let mut rest = body;
    while let Some(a) = rest.find("<arg name=\"") {
        let name_start = a + "<arg name=\"".len();
        let Some(nq) = rest[name_start..].find('"') else {
            break;
        };
        let name = rest[name_start..name_start + nq].to_string();
        let Some(gt) = rest[name_start + nq..].find('>') else {
            break;
        };
        let val_start = name_start + nq + gt + 1;
        let Some(close) = rest[val_start..].find("</arg>") else {
            break;
        };
        let val = rest[val_start..val_start + close]
            .trim_matches('\n')
            .to_string();
        map.insert(name, val);
        rest = &rest[val_start + close + "</arg>".len()..];
    }
    map
}

// ──────────────────────────── the model seam ───────────────────────

/// A model that answers a (system, transcript) prompt with the next turn.
pub trait Chat {
    fn respond(&self, system: &str, transcript: &str) -> Result<String, String>;
    fn label(&self) -> String;
}

/// The unified mind: drives any [`crate::provider::Spec`] (claude cli/api, openai,
/// ollama) as the agent's model. This is the path the app uses.
pub struct ProviderChat {
    pub spec: crate::provider::Spec,
    pub timeout_s: u64,
    /// Explicit sampling control (temperature + seed), honoured on ollama minds;
    /// `None` keeps each provider's default draw. A conclave sets a distinct seed
    /// per adept so its k samples genuinely differ, reproducibly.
    pub sampling: Option<crate::backend::Sampling>,
}
impl Chat for ProviderChat {
    fn respond(&self, system: &str, transcript: &str) -> Result<String, String> {
        loop {
            match self.spec.complete_sampled(
                system,
                transcript,
                Duration::from_secs(self.timeout_s),
                self.sampling,
            ) {
                Ok(response) => return Ok(response),
                Err(error) if crate::pause::retry_model_error(&error) => continue,
                Err(error) => return Err(error),
            }
        }
    }
    fn label(&self) -> String {
        self.spec.label()
    }
}

/// A deterministic stub that replays canned turns in order — for testing the loop
/// with no model.
pub struct ScriptedChat {
    replies: Vec<String>,
    idx: Cell<usize>,
}
impl ScriptedChat {
    pub fn new(replies: Vec<&str>) -> ScriptedChat {
        ScriptedChat {
            replies: replies.into_iter().map(|s| s.to_string()).collect(),
            idx: Cell::new(0),
        }
    }
}
impl Chat for ScriptedChat {
    fn respond(&self, _system: &str, _transcript: &str) -> Result<String, String> {
        let i = self.idx.get();
        self.idx.set(i + 1);
        self.replies
            .get(i)
            .cloned()
            .ok_or_else(|| "scripted chat exhausted".to_string())
    }
    fn label(&self) -> String {
        "scripted".into()
    }
}

// ──────────────────────────── the loop ─────────────────────────────

/// The most sigils one reply may chain — enough for read→edit→test→finish in a
/// single breath, few enough that one bad turn cannot flood the session.
pub const MAX_ACTS_PER_TURN: usize = 5;

/// Human-readable Rebis reference compiled into Kaos. Keeping the examples in
/// `docs/` makes the chat's knowledge auditable and prevents its authoring rules
/// from drifting away from the documentation users see.
const REBIS_AUTHORING_CONTEXT: &str = include_str!("../../docs/REBIS_CHAT_CONTEXT.md");

/// Whether a task should carry the Rebis reference: true for `/chat`/`/code`
/// (the TUI sets `KAOS_REBIS_CONTEXT` for its coding mind) and for any task that
/// names the language. The native `claude` agent path gates its
/// `--append-system-prompt` on this too, so the chat mind and the `<act>` loop
/// learn Rebis under the same rule.
pub fn wants_rebis_authoring_context(task: &str) -> bool {
    kaos_core::config::enabled("KAOS_REBIS_CONTEXT") || task_requests_rebis_authoring_context(task)
}

fn task_requests_rebis_authoring_context(task: &str) -> bool {
    let task = task.to_ascii_lowercase();
    task.contains("rebis")
        || task.contains(".rebis")
        || task.contains("(# std")
        || task.contains("(~ ")
        || task.contains("/run block")
        || task.contains("/mandala")
}

/// The exact reference shared by human-facing docs, `/chat`, and executing
/// Rebis nodes. Hosts can reuse it without maintaining a second prompt copy.
pub fn rebis_authoring_context() -> &'static str {
    REBIS_AUTHORING_CONTEXT
}

/// The Rebis reference framed for the native `claude` CLI agent — the mind
/// behind `/chat` (and `/code` on a Claude model). That agent brings its own
/// tools and loop, so this is the ONLY seam through which it learns the
/// language it hosts; it is appended to Claude's own system prompt via
/// `--append-system-prompt`. Unlike [`rebis_agent_system_prompt`] this does not
/// narrow the agent to a single node — the chat mind explains, authors, and
/// repairs whole Rebis programs.
pub fn claude_agent_rebis_appendix() -> String {
    format!(
        "You are the coding mind inside Kaos, a terminal workspace for Rebis \u{2014} \
         a small Lisp-like language for composing model calls, agents, deterministic \
         judges, reusable macros, and execution flows (programs usually live in \
         `.rebis` files). Users will ask you to explain, write, debug, and repair \
         Rebis programs. The reference below is the language you host; use it to \
         answer questions directly and to author or fix Rebis code with your normal \
         tools. It is knowledge, not a request to edit files.\n\n{REBIS_AUTHORING_CONTEXT}"
    )
}

/// System prompt for a normal-mode Rebis node. The execution constraint is
/// repeated after the reference so a long cookbook cannot displace the node's
/// actual contract from the model's most recent instructions.
pub fn rebis_agent_system_prompt() -> String {
    format!(
        "You are exactly one agent node in a Rebis program. Answer the supplied \
         prompt directly and return only the value that should flow to the next \
         node. Do not delegate, create more agents, propose tool calls, or wrap \
         the answer in orchestration metadata.\n\n{REBIS_AUTHORING_CONTEXT}\n\n\
         You are now executing one node, not authoring the surrounding program. \
         Follow only the supplied node prompt and return only its flow value."
    )
}

/// Contract for one Conductor run that hosts exactly one Rebis node — the
/// tool-agent path on every backend without a native agent CLI. The loop
/// mechanics stay the Conductor's; only the scope narrows to a single direct
/// agent whose finish message is the node's flow value.
pub fn rebis_node_tool_contract() -> String {
    "This run is exactly one agent node of a Rebis program, not a full Kaos \
     session. Use the tools to actually perform every file edit and command the \
     node prompt requests — never merely describe the changes. Complete only \
     this node's work, then call finish whose message is only the value that \
     should flow to the next Rebis node."
        .to_string()
}

fn add_rebis_authoring_context(mut system: String, include: bool) -> String {
    if include {
        system.push_str("\n\n");
        system.push_str(REBIS_AUTHORING_CONTEXT);
        system.push_str(
            "\n\nThe Rebis reference above is authoring knowledge, not a request to edit files. \
             Continue to obey the tool protocol in the first part of this system prompt. \
             For an explanation-only request, return the useful answer through finish(message).",
        );
    }
    system
}

fn system_prompt_for_task(task: &str) -> String {
    add_rebis_authoring_context(system_prompt(), wants_rebis_authoring_context(task))
}

#[cfg(feature = "api")]
fn native_system_prompt_for_task(task: &str) -> String {
    add_rebis_authoring_context(native_system_prompt(), wants_rebis_authoring_context(task))
}

/// The system prompt for the NATIVE tool loop — the doctrine without the
/// wire-format liturgy (the host carries the schemas).
pub fn native_system_prompt() -> String {
    "You are an autonomous coding adept working in a real project directory through \
     the provided tools. Inspect before you edit; never re-read a file that has not \
     changed. A bug report describes the SYMPTOM truthfully but often blames the \
     wrong place — reproduce the behaviour first (a small python -c or the failing \
     test via bash), trace the cause yourself, and only then edit. Change files ONLY \
     with edit_file/write_file (never sed/tee via bash) so every change is shown as \
     a diff; never edit anything under a tests/ directory. You may request several \
     tool calls in one reply — they run in order, and if one fails the rest of that \
     chain is skipped. After editing, verify with bash, then call finish."
        .to_string()
}

/// The system prompt that teaches the agent its tools and format.
pub fn system_prompt() -> String {
    "You are an autonomous coding adept of the Pact. You work in \
     a real project directory by calling tools. Each turn, reply with one OR MORE \
     actions (up to 5) and nothing else, each in this format:\n\
     <act tool=\"TOOL\">\n<arg name=\"KEY\">VALUE</arg>\n</act>\n\n\
     Tools:\n\
     - read_file: args path\n\
     - write_file: args path, contents\n\
     - edit_file: args path, find, replace (replaces the first exact occurrence)\n\
     - bash: args cmd (run any shell command in the project root — ls, grep, run tests)\n\
     - finish: args message (call when the task is done AND you have verified it)\n\n\
     Multiple <act> blocks run IN ORDER; if one fails, the rest of your chain is \
     skipped and you see everything that ran. Chain confident sequences \u{2014} \
     read then edit then test \u{2014} into one reply; a full round-trip per action \
     is the slow path. Inspect before you edit; never re-read a file that has not \
     changed. A bug report describes the SYMPTOM truthfully but often blames the \
     wrong place \u{2014} reproduce the behaviour first (a small python -c or the \
     failing test), trace the cause yourself, and only then edit. To CHANGE a \
     file, use edit_file or write_file (never sed/tee via bash) so every change \
     is shown as a diff. After editing, run the tests with bash to verify, then \
     finish. Directly before your first <act> block, narrate in ONE short line \
     what you are about to do and why \u{2014} the reader follows your work live. \
     Nothing else outside the blocks."
        .to_string()
}

/// One executed step, for the trace.
#[derive(Clone, Debug)]
pub struct Step {
    pub tool: Tool,
    pub observation: String,
    /// What the adept *said* around its action — the reply text outside the
    /// `<act>` block. This is the "what is really happening" a reader expands in
    /// the trace: the model's complete visible text, kept beside the mechanical
    /// record without a display-oriented size cap.
    pub thought: String,
}

/// The reply text outside the `<act>…</act>` block — the model's complete visible
/// narration for this step. Rendering may provide a compact projection, but the
/// retained run stream must never inherit a display-oriented truncation.
fn thought_of(reply: &str) -> String {
    let outside = match reply.find("<act") {
        Some(start) => {
            let after = reply[start..]
                .find("</act>")
                .map(|i| &reply[start + i + "</act>".len()..])
                .unwrap_or("");
            format!("{}{}", reply[..start].trim_end(), after)
        }
        None => reply.to_string(),
    };
    outside.trim().to_string()
}

/// The `<act>…</act>` span of a reply — first block through last block,
/// verbatim (a chain keeps every gesture); falls back to the trimmed reply
/// when the markers are absent (parse_action already accepted it).
fn act_block_of(reply: &str) -> String {
    match (reply.find("<act"), reply.rfind("</act>")) {
        (Some(a), Some(b)) if b > a => reply[a..b + "</act>".len()].to_string(),
        _ => reply.trim().to_string(),
    }
}

/// Render the working transcript through the Twin Ladders: symbol 0 is the
/// statement of intent (never compressed); each turn contributes its act and
/// its observation as one symbol whose budget follows the twin fibonacci
/// curve and whose polarity picks the surviving end.
fn render_transcript(task: &str, turns: &[(String, String)]) -> String {
    let n = 1 + turns.len();
    let mut out = format!("TASK: {task}\n\nBegin. Reply with one <act> block.");
    for (i, (acted, observation)) in turns.iter().enumerate() {
        // Charge is the max of POSITION (the twin ladders) and NATURE (edits
        // and verdicts carry intrinsic charge; reads rot fastest).
        let limit = kaos_pact::charge::budget_kinded(i + 1, n, observation);
        let negative = kaos_pact::charge::is_negative(observation);
        // The act itself stays whole (it is small); the observation is cut.
        let obs = kaos_pact::charge::cut(observation, limit, negative);
        out.push_str(&format!("\n\nAssistant: {acted}\n\nOBSERVATION:\n{obs}"));
    }
    out
}

/// The record of an agent session.
#[derive(Clone, Debug)]
pub struct Session {
    pub steps: Vec<Step>,
    pub finished: bool,
    pub final_message: String,
    pub error: Option<String>,
}

/// The agent: a workspace root and a step budget.
pub struct Conductor {
    pub root: PathBuf,
    pub max_steps: usize,
    /// Timeout for a single `bash` action (tests can be slow but not forever).
    pub bash_timeout_s: u64,
    /// Extra contract appended after the dialect's system prompt. Rebis sets
    /// [`rebis_node_tool_contract`] here so one Conductor run behaves as one
    /// direct node agent instead of a full Kaos session.
    pub system_appendix: Option<String>,
}

impl Conductor {
    pub fn new(root: impl Into<PathBuf>) -> Conductor {
        Conductor {
            root: root.into(),
            max_steps: 14,
            bash_timeout_s: 120,
            system_appendix: None,
        }
    }

    /// The dialect's base system prompt plus the caller's appended contract.
    /// The appendix comes last so it is the most recent instruction the model
    /// reads — a long doctrine or cookbook cannot displace it.
    fn session_system(&self, mut base: String) -> String {
        if let Some(contract) = &self.system_appendix {
            base.push_str("\n\n");
            base.push_str(contract);
        }
        base
    }

    /// Run the loop with `chat` as the mind. `on_step` is called after each action
    /// so a caller can render a live trace.
    ///
    /// The transcript is re-rendered every turn through the Twin Ladders of
    /// charge ([`kaos_pact::charge`]): the statement of intent is never compressed,
    /// fresh observations burn bright, the middle decays to a base budget, and
    /// each symbol's polarity decides which end of it survives the cut. An
    /// unparseable reply is BANISHED — it never enters the transcript; only a
    /// one-line nudge remains, so a format stumble cannot rot the context.
    pub fn run(&self, task: &str, chat: &dyn Chat, on_step: impl FnMut(&Step)) -> Session {
        self.run_observed(task, chat, |_| {}, |_, _| {}, on_step)
    }

    /// Run while exposing every model-call boundary and the complete raw reply.
    /// The ordinary agent API above stays compact; Rebis uses this observer seam
    /// to retain provider text alongside its structured tool trace.
    pub fn run_observed(
        &self,
        task: &str,
        chat: &dyn Chat,
        mut on_model_call: impl FnMut(usize),
        mut on_model_reply: impl FnMut(usize, &str),
        mut on_step: impl FnMut(&Step),
    ) -> Session {
        let system = self.session_system(system_prompt_for_task(task));
        let mut steps = Vec::new();
        // (acted, observation) per turn; acted is the reply's <act> block(s) or
        // a nudge marker. The intent lives at index 0 of the rendered transcript.
        let mut turns: Vec<(String, String)> = Vec::new();
        let mut nudges = 0usize;
        // The repetition ward: hash of the last identical read, per gesture.
        let mut last_reads: std::collections::HashMap<String, u64> =
            std::collections::HashMap::new();
        let mut model_turn = 0usize;

        let mut step_limit = self.max_steps;
        loop {
            if steps.len() >= step_limit {
                let reason = format!(
                    "agent step limit ({}) reached without a failure",
                    self.max_steps
                );
                if self.max_steps > 0 && crate::pause::current_run(&reason) {
                    step_limit = step_limit.saturating_add(self.max_steps);
                    continue;
                }
                break;
            }
            let transcript = render_transcript(task, &turns);
            model_turn += 1;
            on_model_call(model_turn);
            let reply = match chat.respond(&system, &transcript) {
                Ok(r) => r,
                Err(e) => {
                    return Session {
                        steps,
                        finished: false,
                        final_message: String::new(),
                        error: Some(e),
                    };
                }
            };
            on_model_reply(model_turn, &reply);
            if kaos_core::config::enabled("KAOS_DEBUG") {
                eprintln!("\n=== RAW REPLY ===\n{reply}\n=== END ===");
            }
            let tools = parse_actions(&reply, MAX_ACTS_PER_TURN);
            if tools.is_empty() {
                // Banish the malformed reply; keep only the nudge. Three
                // consecutive banishments end the session — the mind is not
                // speaking the protocol and further turns only spend.
                nudges += 1;
                if nudges >= 3 {
                    return Session {
                        steps,
                        finished: false,
                        final_message: String::new(),
                        error: Some(
                            "the mind would not speak in <act> blocks (3 banished replies)".into(),
                        ),
                    };
                }
                turns.push((
                    "(your previous reply held no <act> block and was banished)".to_string(),
                    "reply with <act tool=\"…\">…</act> blocks.".to_string(),
                ));
                continue;
            };
            nudges = 0;
            let thought = thought_of(&reply);

            // A chain of sigils cast in one breath: execute in order, halting
            // at finish, at a negative observation, or at the step budget —
            // acting past a failure would build on rubble.
            let mut turn_log: Vec<(String, String)> = Vec::new();
            let mut finished: Option<String> = None;
            for tool in tools {
                if steps.len() >= self.max_steps {
                    break;
                }
                if let Tool::Finish { message } = &tool {
                    let step = Step {
                        tool: tool.clone(),
                        observation: message.clone(),
                        thought: thought.clone(),
                    };
                    on_step(&step);
                    steps.push(step);
                    finished = Some(message.clone());
                    break;
                }

                // The repetition ward: an identical read of an unchanged file
                // is a gesture already made — banish it to one line instead of
                // paying its full weight in context again.
                let observation = match &tool {
                    Tool::ReadFile { path } => {
                        let obs = self.execute(&tool);
                        let h = kaos_pact::rng::hash_str(&obs);
                        let key = path.clone();
                        if last_reads.get(&key) == Some(&h) {
                            format!("(unchanged since your last read of {path} — re-reading is a wasted step; act on what you already know)")
                        } else {
                            last_reads.insert(key, h);
                            obs
                        }
                    }
                    Tool::WriteFile { path, .. } | Tool::EditFile { path, .. } => {
                        last_reads.remove(path);
                        self.execute(&tool)
                    }
                    _ => self.execute(&tool),
                };

                let step = Step {
                    tool: tool.clone(),
                    observation: observation.clone(),
                    thought: thought.clone(),
                };
                on_step(&step);
                turn_log.push((
                    format!("<act:{}>", step.tool.describe()),
                    observation.clone(),
                ));
                steps.push(step);
                if kaos_pact::charge::is_negative(&observation) {
                    break; // the rest of the chain is skipped, not executed
                }
            }

            if let Some(message) = finished {
                if message.trim().is_empty()
                    && crate::pause::current_run("agent finished without returning a value")
                {
                    if steps.len() >= step_limit {
                        step_limit = step_limit.saturating_add(self.max_steps);
                    }
                    turns.push((
                        act_block_of(&reply),
                        "finish returned no value; continue and finish with a flow value"
                            .to_string(),
                    ));
                    continue;
                }
                return Session {
                    steps,
                    finished: true,
                    final_message: message,
                    error: None,
                };
            }

            // The turn enters the transcript as ONE symbol: acts + observations.
            let acted = act_block_of(&reply);
            let obs_joined = turn_log
                .iter()
                .map(|(d, o)| format!("{d}\n{o}"))
                .collect::<Vec<_>>()
                .join("\n\n");
            turns.push((acted, obs_joined));
        }

        Session {
            steps,
            finished: false,
            final_message: "step budget exhausted".into(),
            error: None,
        }
    }

    /// The Open Hand: the same loop in the mind's NATIVE tool dialect. The
    /// executor, wards, ladders, and fizzle semantics are shared with
    /// [`Conductor::run`]; only the wire protocol differs — structured
    /// `tool_calls` instead of parsed `<act>` blocks. Available on HTTP minds
    /// (openrouter / openai / ollama).
    #[cfg(feature = "api")]
    pub fn run_native(
        &self,
        task: &str,
        spec: &crate::provider::Spec,
        sampling: Option<crate::backend::Sampling>,
        timeout: std::time::Duration,
        on_step: impl FnMut(&Step),
    ) -> Session {
        self.run_native_observed(task, spec, sampling, timeout, |_| {}, |_, _| {}, on_step)
    }

    /// Native-tool equivalent of [`Self::run_observed`]. The observed response
    /// is the provider's complete assistant message rendered as readable JSON.
    #[cfg(feature = "api")]
    #[allow(clippy::too_many_arguments)]
    pub fn run_native_observed(
        &self,
        task: &str,
        spec: &crate::provider::Spec,
        sampling: Option<crate::backend::Sampling>,
        timeout: std::time::Duration,
        mut on_model_call: impl FnMut(usize),
        mut on_model_reply: impl FnMut(usize, &str),
        mut on_step: impl FnMut(&Step),
    ) -> Session {
        use crate::hand::{parse_reply, render_messages, tool_schemas, Msg};
        let system = self.session_system(native_system_prompt_for_task(task));
        let tools = tool_schemas();
        let mut history: Vec<Msg> = vec![Msg::user(format!(
            "TASK: {task}\n\nBegin. Work with tool calls."
        ))];
        let mut steps = Vec::new();
        let mut idle = 0usize; // consecutive call-less replies
        let mut last_reads: std::collections::HashMap<String, u64> =
            std::collections::HashMap::new();
        let mut model_turn = 0usize;

        let mut step_limit = self.max_steps;
        loop {
            if steps.len() >= step_limit {
                let reason = format!(
                    "agent step limit ({}) reached without a failure",
                    self.max_steps
                );
                if self.max_steps > 0 && crate::pause::current_run(&reason) {
                    step_limit = step_limit.saturating_add(self.max_steps);
                    continue;
                }
                break;
            }
            let messages = render_messages(&system, &history);
            model_turn += 1;
            on_model_call(model_turn);
            let raw = loop {
                match spec.complete_native(&messages, &tools, timeout, sampling) {
                    Ok(message) => break message,
                    Err(error) if crate::pause::retry_model_error(&error) => continue,
                    Err(error) => {
                        return Session {
                            steps,
                            finished: false,
                            final_message: String::new(),
                            error: Some(error),
                        };
                    }
                }
            };
            let raw_text = serde_json::to_string_pretty(&raw).unwrap_or_else(|_| raw.to_string());
            on_model_reply(model_turn, &raw_text);
            let reply = parse_reply(&raw);
            if kaos_core::config::enabled("KAOS_DEBUG") {
                eprintln!("\n=== NATIVE REPLY ===\n{raw}\n=== END ===");
            }
            if reply.calls.is_empty() {
                // A bare-text reply is a stall in a tool loop. Nudge twice,
                // then end — same banishment law as the act protocol.
                idle += 1;
                if idle >= 3 {
                    return Session {
                        steps,
                        finished: false,
                        final_message: String::new(),
                        error: Some("the mind stopped calling tools (3 idle replies)".into()),
                    };
                }
                history.push(Msg::assistant(reply.content, Vec::new()));
                history.push(Msg::user(
                    "Continue with tool calls (finish(message) when done and verified)."
                        .to_string(),
                ));
                continue;
            }
            idle = 0;

            let thought = reply.content.clone();
            let mut executed: Vec<(String, Tool)> = Vec::new();
            let mut observations: Vec<Msg> = Vec::new();
            let mut finished: Option<String> = None;
            let mut halted = false;
            for (id, tool) in reply.calls.iter().take(MAX_ACTS_PER_TURN) {
                if halted || steps.len() >= self.max_steps {
                    // Chain halted: unexecuted calls still need tool-role
                    // answers or the next completion is rejected by the host.
                    executed.push((id.clone(), tool.clone()));
                    observations.push(Msg::tool(
                        id.clone(),
                        "(not executed: an earlier call in your chain failed)",
                    ));
                    continue;
                }
                if let Tool::Finish { message } = tool {
                    let step = Step {
                        tool: tool.clone(),
                        observation: message.clone(),
                        thought: thought.clone(),
                    };
                    on_step(&step);
                    steps.push(step);
                    finished = Some(message.clone());
                    executed.push((id.clone(), tool.clone()));
                    observations.push(Msg::tool(id.clone(), "done"));
                    continue;
                }
                // The repetition ward, shared law with the act loop.
                let observation = match tool {
                    Tool::ReadFile { path } => {
                        let obs = self.execute(tool);
                        let h = kaos_pact::rng::hash_str(&obs);
                        if last_reads.get(path.as_str()) == Some(&h) {
                            format!("(unchanged since your last read of {path} — act on what you already know)")
                        } else {
                            last_reads.insert(path.clone(), h);
                            obs
                        }
                    }
                    Tool::WriteFile { path, .. } | Tool::EditFile { path, .. } => {
                        last_reads.remove(path.as_str());
                        self.execute(tool)
                    }
                    _ => self.execute(tool),
                };
                let step = Step {
                    tool: tool.clone(),
                    observation: observation.clone(),
                    thought: thought.clone(),
                };
                on_step(&step);
                steps.push(step);
                if kaos_pact::charge::is_negative(&observation) {
                    halted = true;
                }
                executed.push((id.clone(), tool.clone()));
                observations.push(Msg::tool(id.clone(), observation));
            }
            history.push(Msg::assistant(thought, executed));
            history.extend(observations);
            if let Some(message) = finished {
                if message.trim().is_empty()
                    && crate::pause::current_run("agent finished without returning a value")
                {
                    if steps.len() >= step_limit {
                        step_limit = step_limit.saturating_add(self.max_steps);
                    }
                    history.push(Msg::user(
                        "Continue and finish with the value this node should return.".to_string(),
                    ));
                    continue;
                }
                return Session {
                    steps,
                    finished: true,
                    final_message: message,
                    error: None,
                };
            }
        }
        Session {
            steps,
            finished: false,
            final_message: "step budget exhausted".into(),
            error: None,
        }
    }

    /// The lint gate on edits — Carroll's *"take all possible ordinary steps"*,
    /// applied mechanically, escalated (v3) from a warning to the Inquisitor's
    /// VETO: the devbench data showed a warned agent still kept working atop the
    /// wreck, so a `.py` write/edit that does not parse is now rolled back by the
    /// callers above. Returns the compile error when the file is broken, `None`
    /// when healthy, not Python, or python3 is unavailable (the gate never
    /// blocks work it cannot judge).
    fn lint_error(&self, p: &Path) -> Option<String> {
        if p.extension().and_then(|e| e.to_str()) != Some("py") {
            return None;
        }
        let out = Command::new("python3")
            .arg("-m")
            .arg("py_compile")
            .arg(p)
            .current_dir(&self.root)
            .output()
            .ok()?;
        if out.status.success() {
            return None;
        }
        let err = String::from_utf8_lossy(&out.stderr);
        Some(
            err.lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("syntax error")
                .chars()
                .take(160)
                .collect(),
        )
    }

    /// Resolve a tool path INSIDE the workspace root. Absolute paths and `..`
    /// traversal are refused: the file tools honour the isolation the conclave
    /// promises. (`bash` can still roam — that is inherent to offering a shell —
    /// but the write-back only ever ships what changed *under the root*.)
    fn resolve(&self, path: &str) -> Result<PathBuf, String> {
        let p = Path::new(path);
        if p.is_absolute()
            || p.components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(format!(
                "error: path {path:?} escapes the workspace; use a path relative to the project root"
            ));
        }
        Ok(self.root.join(p))
    }

    /// The Seal (`KAOS_PROTECT`, comma-separated substrings): paths matching a
    /// protected pattern are REFUSED at the executor for writes and edits — a
    /// mechanical ward where prompt-level prohibitions measurably failed
    /// (k2.7 tampered with test files through every prompt ban tried). Reads
    /// stay free; only mutation is sealed. Bash can still bypass — inherent to
    /// offering a shell — but every measured tamper came through edit tools.
    fn sealed(&self, path: &str) -> Option<String> {
        // The Lost Sigil's guard (G14): the forged gate is ALWAYS sealed, by
        // law, not by env — the self pursues the intent; the metric belongs
        // to the Weighing alone.
        if path.contains("kaos_repro.py") {
            return Some(
                "REFUSED: the Weighing is sealed — it judges each attempt automatically; \
                 fix the source, never the verifier."
                    .to_string(),
            );
        }
        let spec = std::env::var("KAOS_PROTECT").ok()?;
        for pat in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            if path.contains(pat) {
                return Some(format!(
                    "REFUSED: {path} is SEALED (matches {pat:?}) — fix the source, never the tests."
                ));
            }
        }
        None
    }

    /// Execute a tool in the workspace and return the observation text.
    pub(crate) fn execute(&self, tool: &Tool) -> String {
        match tool {
            Tool::ReadFile { path } => match self.resolve(path).map(std::fs::read_to_string) {
                Ok(Ok(s)) => number_lines(&s),
                Ok(Err(e)) => format!("error: {e}"),
                Err(e) => e,
            },
            Tool::WriteFile { path, contents } => {
                if let Some(refusal) = self.sealed(path) {
                    return refusal;
                }
                let p = match self.resolve(path) {
                    Ok(p) => p,
                    Err(e) => return e,
                };
                let old = std::fs::read_to_string(&p).unwrap_or_default();
                let existed = p.exists();
                if let Some(parent) = p.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                match std::fs::write(&p, contents) {
                    Ok(_) => {
                        // The Inquisitor's veto: a write that leaves the file
                        // unparsable is ROLLED BACK, not merely flagged — the
                        // warning alone proved insufficient (the agent kept
                        // working atop the wreck). A brand-new broken file is
                        // removed; a broken overwrite restores the prior text.
                        if let Some(err) = self.lint_error(&p) {
                            if existed {
                                let _ = std::fs::write(&p, &old);
                            } else {
                                let _ = std::fs::remove_file(&p);
                            }
                            return format!(
                                "REFUSED: that content does not parse ({err}). {path} is UNCHANGED. \
                                 Write the complete, valid file — check backslashes and indentation."
                            );
                        }
                        let lines = contents.lines().count();
                        if existed {
                            let (a, r) = line_delta(&old, contents);
                            format!("wrote {path}: {lines} lines (+{a} -{r})")
                        } else {
                            format!("created {path}: {lines} lines")
                        }
                    }
                    Err(e) => format!("error: {e}"),
                }
            }
            Tool::EditFile {
                path,
                find,
                replace,
            } => {
                if let Some(refusal) = self.sealed(path) {
                    return refusal;
                }
                let p = match self.resolve(path) {
                    Ok(p) => p,
                    Err(e) => return e,
                };
                match std::fs::read_to_string(&p) {
                    Ok(cur) => {
                        let hits = cur.matches(find.as_str()).count();
                        if hits > 1 {
                            // A non-unique anchor silently edits the WRONG place —
                            // the classic failure of weak models that pass a tiny
                            // `find` (e.g. a bare identifier). Refuse and demand
                            // context, rather than corrupt the file.
                            return format!(
                                "REFUSED: `find` matches {hits} places in {path}; the edit is \
                                 ambiguous. Include enough surrounding lines to make `find` unique."
                            );
                        }
                        if let Some(pos) = cur.find(find) {
                            let next =
                                format!("{}{}{}", &cur[..pos], replace, &cur[pos + find.len()..]);
                            match std::fs::write(&p, next) {
                                Ok(_) => {
                                    // The veto, same as write_file: a breaking edit
                                    // is rolled back, never left in place.
                                    if let Some(err) = self.lint_error(&p) {
                                        let _ = std::fs::write(&p, &cur);
                                        format!(
                                            "REFUSED: that edit breaks the file ({err}). {path} is \
                                             UNCHANGED. Re-read the region and try a smaller edit."
                                        )
                                    } else {
                                        format!("edited {path}: replaced 1 occurrence")
                                    }
                                }
                                Err(e) => format!("error: {e}"),
                            }
                        } else {
                            format!("error: `find` text not present in {path}; read it first")
                        }
                    }
                    Err(e) => format!("error: {e}"),
                }
            }
            Tool::Bash { cmd } => self.run_bash(cmd),
            Tool::Finish { message } => message.clone(),
        }
    }

    fn run_bash(&self, cmd: &str) -> String {
        run_shell(&self.root, cmd, self.bash_timeout_s)
    }
}

/// Run a shell command in `root` with drained pipes and a hard timeout, returning
/// `"exit N\n<output>"` (or an error line). The primitive under the agent's bash
/// tool and the adaptive gate check.
pub fn run_shell(root: &Path, cmd: &str, timeout_s: u64) -> String {
    {
        let mut child = match Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .current_dir(root)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => return format!("error launching bash: {e}"),
        };

        // Drain both pipes on reader threads WHILE waiting. Without this, a command
        // whose output exceeds the OS pipe buffer (~64KB — any real test run or
        // grep) blocks on write, never exits, and gets reported as a bogus timeout.
        fn drain<R: std::io::Read + Send + 'static>(
            src: Option<R>,
        ) -> std::thread::JoinHandle<String> {
            std::thread::spawn(move || {
                let mut s = String::new();
                if let Some(mut r) = src {
                    let _ = std::io::Read::read_to_string(&mut r, &mut s);
                }
                s
            })
        }
        let out_reader = drain(child.stdout.take());
        let err_reader = drain(child.stderr.take());

        // Bounded wait, so a runaway command cannot hang the agent.
        let start = std::time::Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let so = out_reader.join().unwrap_or_default();
                    let se = err_reader.join().unwrap_or_default();
                    return format!("exit {}\n{}{}", status.code().unwrap_or(-1), so, se);
                }
                Ok(None) => {
                    if start.elapsed() > Duration::from_secs(timeout_s) {
                        let _ = child.kill();
                        let _ = child.wait();
                        let _ = out_reader.join();
                        let _ = err_reader.join();
                        return format!("error: command timed out after {timeout_s}s");
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => return format!("error waiting on bash: {e}"),
            }
        }
    }
}

/// A cheap line-level change summary: (added, removed) as multiset differences.
/// Order-insensitive, but enough for a compact "+A -R" readout.
pub fn line_delta(old: &str, new: &str) -> (usize, usize) {
    use std::collections::HashMap;
    let mut counts: HashMap<&str, i32> = HashMap::new();
    for l in old.lines() {
        *counts.entry(l).or_default() += 1;
    }
    for l in new.lines() {
        *counts.entry(l).or_default() -= 1;
    }
    let removed = counts
        .values()
        .filter(|&&v| v > 0)
        .map(|&v| v as usize)
        .sum();
    let added = counts
        .values()
        .filter(|&&v| v < 0)
        .map(|&v| (-v) as usize)
        .sum();
    (added, removed)
}

// ─────────────────────── the adaptive quorum ───────────────────────
//
// The app's DEFAULT way of spending agents, so the user never has to size a
// conclave by hand. The second equation's allocation, made mechanical:
// one attempt first (nothing is spent at the ceiling); only a failed Weighing
// grows the quorum, and each further attempt carries the gate's verdict as a
// distilled memory (the retroactive-enchantment retry devbench measured).
// Work happens IN PLACE, like the single-adept path — review with git.

/// Look at the project and find its own Weighing: the test command a developer
/// would run. First match wins; None if the project offers nothing to divine.
pub fn detect_gate(root: &Path) -> Option<String> {
    if root.join("tests.py").is_file() {
        return Some("python3 tests.py".into());
    }
    if root.join("pytest.ini").is_file()
        || root.join("conftest.py").is_file()
        || root.join("tests").is_dir()
    {
        return Some("python3 -m pytest -q".into());
    }
    if root.join("Cargo.toml").is_file() {
        return Some("cargo test".into());
    }
    if let Ok(pkg) = std::fs::read_to_string(root.join("package.json")) {
        if pkg.contains("\"test\"") {
            return Some("npm test --silent".into());
        }
    }
    if let Ok(mk) = std::fs::read_to_string(root.join("Makefile")) {
        if mk.lines().any(|l| l.starts_with("test:")) {
            return Some("make test".into());
        }
    }
    None
}

/// The adaptive run's record.
pub struct AdaptiveOutcome {
    pub attempts: usize,
    /// Did the gate weigh true in the end?
    pub verified: bool,
}

/// Read every small text file under `root` (skipping `.git` and caches) so a
/// failed audit can be banished whole. `None` when the tree is too large —
/// the audit then simply doesn't run rather than risking an unrestorable mess.
fn snapshot_tree(root: &Path, cap: usize) -> Option<std::collections::HashMap<PathBuf, String>> {
    let mut map = std::collections::HashMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir).ok()?;
        for e in entries.flatten() {
            let p = e.path();
            let name = e.file_name();
            let name = name.to_string_lossy();
            if name == ".git"
                || name == "__pycache__"
                || name == ".hidden"
                || name == "node_modules"
            {
                continue;
            }
            if p.is_dir() {
                stack.push(p);
            } else if let Ok(s) = std::fs::read_to_string(&p) {
                map.insert(p, s);
                if map.len() > cap {
                    return None;
                }
            }
        }
    }
    Some(map)
}

/// Put every snapshotted file back exactly as it was; remove files the audit
/// created that were not in the snapshot is NOT attempted (created files are
/// harmless next to restored originals, and deletion is the riskier move).
fn restore_tree(before: &std::collections::HashMap<PathBuf, String>) {
    for (p, contents) in before {
        let _ = std::fs::write(p, contents);
    }
}

/// Sequential verified attempts, in place: run the agent, run the gate; a pass
/// ends the working, a failure banishes the context and begins one more attempt
/// carrying the verdict — up to `max_attempts`. `chat_for(i)` supplies attempt
/// i's mind (distinct sampling seeds make the retries genuinely different).
#[allow(clippy::too_many_arguments)]
pub fn run_adaptive(
    root: &Path,
    task: &str,
    gate: &str,
    mut chat_for: impl FnMut(usize) -> Box<dyn Chat>,
    budgets: &[usize],
    bash_timeout_s: u64,
    on_step: impl FnMut(usize, &Step),
    emit: impl FnMut(&str),
) -> AdaptiveOutcome {
    let root_b = root.to_path_buf();
    let bt = bash_timeout_s;
    run_adaptive_with(
        root,
        task,
        gate,
        move |attempt, intent, steps_budget, on: &mut dyn FnMut(&Step)| {
            let mut conductor = Conductor::new(&root_b);
            conductor.max_steps = steps_budget;
            conductor.bash_timeout_s = bt;
            let chat = chat_for(attempt);
            conductor.run(intent, chat.as_ref(), on)
        },
        |_| String::new(), // no dreamer on the simple wrapper
        budgets,
        bash_timeout_s,
        on_step,
        emit,
    )
}

/// [`run_adaptive`] with the session-running strategy injected: `run_attempt`
/// receives (attempt index, intent, step budget, step sink) and returns the
/// session. This is the seam that lets the Open Hand (native tool-calling)
/// drive the same gate/spiral/gnosis/audit skeleton as the act protocol.
#[allow(clippy::too_many_arguments)]
pub fn run_adaptive_with(
    root: &Path,
    task: &str,
    gate: &str,
    mut run_attempt: impl FnMut(usize, &str, usize, &mut dyn FnMut(&Step)) -> Session,
    mut dream: impl FnMut(&str) -> String,
    budgets: &[usize],
    bash_timeout_s: u64,
    mut on_step: impl FnMut(usize, &Step),
    mut emit: impl FnMut(&str),
) -> AdaptiveOutcome {
    let one = [14usize];
    let budgets = if budgets.is_empty() {
        &one[..]
    } else {
        budgets
    };
    let max_attempts = budgets.len();
    let mut intent = task.to_string();
    for (attempt, &steps_budget) in budgets.iter().enumerate() {
        if attempt > 0 {
            emit(&format!(
                "the spiral turns — attempt {}/{} under {} stars, {} steps",
                attempt + 1,
                max_attempts,
                crate::spiral::Polarity::of_attempt(attempt).name(),
                steps_budget,
            ));
        }
        let session = run_attempt(attempt, &intent, steps_budget, &mut |step| {
            on_step(attempt, step)
        });

        let out = run_shell(root, gate, bash_timeout_s.max(120));
        let passed = out.starts_with("exit 0");
        let verdict = tail_line(&out);
        if passed {
            emit(&format!(
                "\u{2713} the Weighing passed on attempt {} \u{2014} `{gate}`",
                attempt + 1
            ));
            // ── the Lunar Audit ──
            // A gate is necessary, not sufficient: the Ordeal data showed a
            // passing fix can still ship a regression the visible tests never
            // cover (an over-broad guard; a deleted quantization). So after
            // the solar work passes, its reverse twin attacks it: a short
            // session told to REFUTE the diff by probing an input class the
            // tests don't exercise. If the audit's own edits break the gate,
            // they are banished whole and the original passing state stands.
            if let Some(before) = snapshot_tree(root, 300) {
                // ONE sampled probe, not a checklist: the Mirror measured the
                // eight-ray sweep WORSE than the single hunch (7/9 vs 8/9) —
                // a scripted list doesn't out-see the prior that wrote the bug,
                // it just spends the budget; the lucky draw is the mechanism.
                let audit_task = format!(
                    "{task}\n\nThe fix is in place and `{gate}` PASSES. You are now its adversary. \
                     Read the current diff-relevant code, pick the ONE untested input class your \
                     change most plausibly broke (edge values, signs, other currencies/formats, \
                     error types), and probe it with a quick bash command. If the probe shows a \
                     real break, fix it minimally and re-run `{gate}`. If nothing breaks, finish \
                     WITHOUT editing anything."
                );
                // The auditor is the other polarity's draw, 12 steps.
                let _ = run_attempt(attempt + 1, &audit_task, 12, &mut |_| {});
                let out2 = run_shell(root, gate, bash_timeout_s.max(120));
                if !out2.starts_with("exit 0") {
                    restore_tree(&before);
                    emit("\u{263e} the lunar audit broke the gate \u{2014} its edits were banished; the passing work stands");
                } else {
                    emit("\u{263e} the lunar audit held \u{2014} the work survives its own adversary");
                }
            }
            return AdaptiveOutcome {
                attempts: attempt + 1,
                verified: true,
            };
        }
        emit(&format!("\u{2717} the Weighing failed: {verdict}"));
        // The Gnosis Crossing: the banished self's map and the gate's verdict
        // both cross to the next attempt, phrased as known fact.
        let gnosis = crate::spiral::gnosis(&session);
        // The Dream (G12): a toolless divination over the failure seeds the
        // next self. `dream` returns "" when dreaming is off/unavailable.
        let dream = dream(&format!("{gnosis}\nThe verifier reported: {verdict}"));
        intent = format!(
            "{task}\n\nYou have worked on this before. The verifier `{gate}` then reported:\n{verdict}\n\
             The banished working's gnosis:\n{gnosis}{dream}\
             Fix what remains, run the verifier yourself to confirm, then finish."
        );
    }
    AdaptiveOutcome {
        attempts: max_attempts,
        verified: false,
    }
}

// ──────────────────────── the coding conclave ──────────────────────
//
// The one mechanism the benchmarks proved worth its cost — **verified best-of-k** —
// applied to the *real* agentic loop, not just single-shot file rewrites. Convene k
// adepts; each runs a full [`Conductor`] session in its OWN isolated copy of the
// target (a banished, private context); gate each copy with the project's own tests;
// then ship the **consensus verified** diff — the modal change-set among the ones
// that passed — back into the real project. Nothing unverified ships.

/// A live event from a running conclave, for the caller's trace/fold rendering.
pub enum ConclaveEvent {
    /// An adept has begun working in its isolated copy.
    AdeptStart { i: usize, k: usize, name: String },
    /// One tool step that adept took.
    Step { i: usize, step: Step },
    /// That adept finished: did the gate weigh it true, how big was its diff.
    AdeptEnd {
        i: usize,
        verified: bool,
        changed: usize,
        diff_lines: usize,
        note: String,
    },
    /// The quorum adjourned early: the leading verified change-set can no longer be
    /// overtaken by the adepts not yet convened, so convening them is pure waste —
    /// *"there is very little point in repeating a conjuration unless there is a
    /// chance of doing it better"* (Liber Kaos). The decision is identical to
    /// running all k; only the spend differs. See [`crate::scry`].
    Adjourned { convened: usize, k: usize },
    /// The conclave shipped the consensus verified diff.
    Shipped {
        winner: usize,
        votes: usize,
        gated: bool,
        files: usize,
    },
    /// Nothing passed the gate — the project is left untouched.
    NothingShipped { gated: bool },
}

/// One adept's full attempt within a conclave. `changes` maps rel-path →
/// `Some(new contents)` for writes and `None` for deletions (see
/// [`Workspace::changed_files`]).
pub struct ConclaveAttempt {
    pub name: String,
    pub finished: bool,
    pub verified: bool,
    pub changes: BTreeMap<String, Option<String>>,
    pub diff_lines: usize,
}

/// The outcome of a coding conclave.
pub struct ConclaveOutcome {
    pub attempts: Vec<ConclaveAttempt>,
    pub winner: Option<usize>,
    pub shipped: bool,
    /// True when a real verifier decided; false when we fell back to consensus-only
    /// (no gate) — an honestly weaker signal the caller should label as such.
    pub gated: bool,
}

/// Run a verified best-of-k coding conclave against `target`.
///
/// Each `(name, chat)` in `adepts` drives one [`Conductor`] session in a fresh
/// isolated copy of `target`. When `verify_cmd` is `Some`, it is run in each copy as
/// the gate (exit 0 == verified); when `None`, an attempt counts as "verified" if the
/// loop finished and produced a non-empty diff — consensus-only, no true gate. Among
/// verified attempts the **modal** change-set wins (ties → smallest diff, then first),
/// and it is written back into `target`. Events stream through `emit` for live folds.
#[allow(clippy::too_many_arguments)]
pub fn run_conclave(
    target: &Path,
    task: &str,
    verify_cmd: Option<&str>,
    adepts: Vec<(String, Box<dyn Chat>)>,
    max_steps: usize,
    bash_timeout_s: u64,
    mut emit: impl FnMut(ConclaveEvent),
) -> std::io::Result<ConclaveOutcome> {
    let k = adepts.len();
    let mut attempts: Vec<ConclaveAttempt> = Vec::with_capacity(k);

    for (i, (name, chat)) in adepts.into_iter().enumerate() {
        emit(ConclaveEvent::AdeptStart {
            i,
            k,
            name: name.clone(),
        });

        // A private, banished context: one adept's mess never touches another's.
        let ws = Workspace::isolate(target)?;
        let mut conductor = Conductor::new(&ws.root);
        conductor.max_steps = max_steps;
        conductor.bash_timeout_s = bash_timeout_s;
        let session = conductor.run(task, chat.as_ref(), |step| {
            emit(ConclaveEvent::Step {
                i,
                step: step.clone(),
            });
        });

        let changes = ws.changed_files(target).unwrap_or_default();
        let diff_lines = changes
            .iter()
            .map(|(rel, new)| {
                let old = std::fs::read_to_string(target.join(rel)).unwrap_or_default();
                // A deletion (`None`) diffs against emptiness: every old line removed.
                let (a, r) = line_delta(&old, new.as_deref().unwrap_or(""));
                a + r
            })
            .sum();

        // The Weighing: a real gate if given, else the loop's own claim of completion
        // backed by an actual diff (consensus-only mode).
        let (verified, note) = match verify_cmd {
            Some(cmd) => {
                let (ok, log) = ws.verify(cmd);
                (ok, tail_line(&log))
            }
            None => {
                let ok = session.finished && !changes.is_empty();
                (
                    ok,
                    if ok {
                        "finished with a diff".into()
                    } else {
                        "no verifiable result".into()
                    },
                )
            }
        };

        emit(ConclaveEvent::AdeptEnd {
            i,
            verified,
            changed: changes.len(),
            diff_lines,
            note,
        });
        attempts.push(ConclaveAttempt {
            name,
            finished: session.finished,
            verified,
            changes,
            diff_lines,
        });
        // ws dropped here → the isolated copy is removed.

        // The adjourned quorum: if the leading verified change-set already has more
        // votes than any rival could reach even if EVERY remaining adept joined that
        // rival, the outcome is settled — stop convening. Strict inequality, so a
        // reachable tie (whose tie-break could shift the winner) never adjourns.
        let remaining = k - attempts.len();
        if remaining > 0 && settled_beyond_overturning(&attempts, remaining) {
            emit(ConclaveEvent::Adjourned {
                convened: attempts.len(),
                k,
            });
            break;
        }
    }

    // Vote among the verified attempts: modal change-set wins, ties → smallest diff.
    let winner = pick_winner(&attempts);
    let gated = verify_cmd.is_some();
    if let Some(w) = winner {
        let votes = {
            let key = agent::changeset_key(&attempts[w].changes);
            attempts
                .iter()
                .filter(|a| a.verified && agent::changeset_key(&a.changes) == key)
                .count()
        };
        agent::write_files_into(target, &attempts[w].changes)?;
        emit(ConclaveEvent::Shipped {
            winner: w,
            votes,
            gated,
            files: attempts[w].changes.len(),
        });
    } else {
        emit(ConclaveEvent::NothingShipped { gated });
    }

    Ok(ConclaveOutcome {
        attempts,
        winner,
        shipped: winner.is_some(),
        gated,
    })
}

/// Is the conclave's outcome already beyond overturning? True when the modal
/// verified change-set leads every rival by more than `remaining` — no sequence of
/// future attempts (all joining the best rival, or founding a new one) can change
/// [`pick_winner`]'s decision. Same-key attempts carry byte-identical changes, so
/// the shipped content is fixed the moment the key is.
fn settled_beyond_overturning(attempts: &[ConclaveAttempt], remaining: usize) -> bool {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for a in attempts
        .iter()
        .filter(|a| a.verified && !a.changes.is_empty())
    {
        *counts.entry(agent::changeset_key(&a.changes)).or_insert(0) += 1;
    }
    let mut sorted: Vec<usize> = counts.values().copied().collect();
    sorted.sort_unstable_by(|a, b| b.cmp(a));
    let leader = sorted.first().copied().unwrap_or(0);
    let rival = sorted.get(1).copied().unwrap_or(0);
    leader > rival + remaining
}

/// The winning attempt index: among verified attempts, the modal change-set (most
/// adepts agreeing byte-for-byte); ties broken by the smallest diff, then first seen.
fn pick_winner(attempts: &[ConclaveAttempt]) -> Option<usize> {
    let verified: Vec<usize> = attempts
        .iter()
        .enumerate()
        .filter(|(_, a)| a.verified && !a.changes.is_empty())
        .map(|(i, _)| i)
        .collect();
    if verified.is_empty() {
        return None;
    }
    let key_of = |i: usize| agent::changeset_key(&attempts[i].changes);
    verified.iter().copied().max_by(|&a, &b| {
        let va = verified.iter().filter(|&&j| key_of(j) == key_of(a)).count();
        let vb = verified.iter().filter(|&&j| key_of(j) == key_of(b)).count();
        // More votes wins; then fewer diff lines; then earlier index (Reverse).
        va.cmp(&vb)
            .then(attempts[b].diff_lines.cmp(&attempts[a].diff_lines))
            .then(b.cmp(&a))
    })
}

/// The last non-empty line of a verifier log, trimmed to a compact note.
fn tail_line(log: &str) -> String {
    let line = log
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    if line.chars().count() > 80 {
        line.chars().take(80).collect()
    } else {
        line.to_string()
    }
}

fn number_lines(s: &str) -> String {
    let mut out = String::new();
    for (i, line) in s.lines().enumerate() {
        out.push_str(&format!("{:>4}  {}\n", i + 1, line));
    }
    if out.is_empty() {
        out.push_str("(empty file)");
    }
    trunc(&out, 4000)
}

fn trunc(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(n).collect();
        t.push_str("\n…(truncated)");
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rebis_authoring_context_is_auditable_complete_and_parseable() {
        let prompt = add_rebis_authoring_context(system_prompt(), true);
        let routed_prompt = system_prompt_for_task("help me write a Rebis review program");
        for required in [
            "(<- A B)",
            "equivalent to `(-> B A)`",
            "(~ name (parameters) body)",
            "(# std)",
            "/run block parallel",
            "finish(message)",
        ] {
            assert!(prompt.contains(required), "missing Rebis rule: {required}");
            assert!(
                routed_prompt.contains(required),
                "routed chat prompt missing Rebis rule: {required}"
            );
        }

        let examples = REBIS_AUTHORING_CONTEXT
            .split("```rebis\n")
            .skip(1)
            .map(|tail| tail.split("\n```").next().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(examples.len(), 7);
        for (index, source) in examples.iter().enumerate() {
            rebis_lang::parse(source)
                .unwrap_or_else(|error| panic!("Rebis chat example {}: {error}", index + 1));
        }

        assert!(task_requests_rebis_authoring_context(
            "help repair this Rebis macro"
        ));
        assert!(task_requests_rebis_authoring_context(
            "why does (~ inspect (topic) topic) fail?"
        ));
        assert!(task_requests_rebis_authoring_context(
            "You are a Rebis agent operating in this workspace:\n/tmp/project"
        ));
        let node_prompt = rebis_agent_system_prompt();
        assert!(node_prompt.contains("## Example 7: bounded recursive refinement"));
        assert!(node_prompt
            .ends_with("Follow only the supplied node prompt and return only its flow value."));
        assert_eq!(rebis_authoring_context(), REBIS_AUTHORING_CONTEXT);
    }

    #[test]
    fn a_rebis_node_tool_agent_edits_files_and_returns_the_flow_value() {
        use std::cell::RefCell;
        struct CapturingChat {
            replies: ScriptedChat,
            system: RefCell<String>,
        }
        impl Chat for CapturingChat {
            fn respond(&self, system: &str, transcript: &str) -> Result<String, String> {
                *self.system.borrow_mut() = system.to_string();
                self.replies.respond(system, transcript)
            }
            fn label(&self) -> String {
                "capturing".into()
            }
        }

        let dir = tmpdir(&[]);
        let mut conductor = Conductor::new(&dir);
        conductor.system_appendix = Some(rebis_node_tool_contract());
        let chat = CapturingChat {
            replies: ScriptedChat::new(vec![
                "<act tool=\"write_file\"><arg name=\"path\">note.txt</arg>\
                 <arg name=\"contents\">flow</arg></act>",
                "<act tool=\"finish\"><arg name=\"message\">the flow value</arg></act>",
            ]),
            system: RefCell::new(String::new()),
        };
        let session = conductor.run(
            "You are a Rebis agent operating in this workspace: write note.txt",
            &chat,
            |_| {},
        );

        assert!(session.finished);
        assert_eq!(session.final_message, "the flow value");
        assert_eq!(
            std::fs::read_to_string(dir.join("note.txt")).unwrap(),
            "flow"
        );
        let system = chat.system.borrow();
        assert!(
            system.ends_with(&rebis_node_tool_contract()),
            "the node contract must be the model's most recent instruction"
        );
        assert!(
            system.contains("## Example 7"),
            "a Rebis-framed task must carry the authoring cookbook"
        );
    }

    fn tmpdir(files: &[(&str, &str)]) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let uniq = N.fetch_add(1, Ordering::Relaxed);
        let d =
            std::env::temp_dir().join(format!("kaos-cond-{}-{nanos}-{uniq}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        for (p, c) in files {
            std::fs::write(d.join(p), c).unwrap();
        }
        d
    }

    #[test]
    fn parse_simple_and_multiarg() {
        let r = parse_action(
            "thinking...\n<act tool=\"read_file\">\n<arg name=\"path\">src/x.rs</arg>\n</act>",
        )
        .unwrap();
        assert_eq!(
            r,
            Tool::ReadFile {
                path: "src/x.rs".into()
            }
        );
        let e = parse_action(
            "<act tool=\"edit_file\">\n<arg name=\"path\">a.py</arg>\n<arg name=\"find\">a - b</arg>\n<arg name=\"replace\">a + b</arg>\n</act>",
        )
        .unwrap();
        assert_eq!(
            e,
            Tool::EditFile {
                path: "a.py".into(),
                find: "a - b".into(),
                replace: "a + b".into()
            }
        );
    }

    #[test]
    fn thought_is_the_text_outside_the_act_block() {
        let reply = "I should fix the sign first.\n<act tool=\"edit_file\"><arg name=\"path\">a</arg></act>\nThen verify.";
        assert_eq!(
            thought_of(reply),
            "I should fix the sign first.\nThen verify."
        );
        assert_eq!(thought_of("<act tool=\"bash\"></act>"), "");
        // No act block at all: the whole reply is the thought.
        assert_eq!(thought_of("just musing"), "just musing");

        let long = "model-stream-line\n".repeat(80);
        let reply = format!("{long}<act tool=\"finish\"><arg name=\"message\">done</arg></act>");
        assert_eq!(thought_of(&reply), long.trim_end());
    }

    #[test]
    fn observed_run_reports_every_complete_raw_model_turn() {
        let dir = tmpdir(&[]);
        let raw =
            "full model narration\n<act tool=\"finish\"><arg name=\"message\">done</arg></act>";
        let chat = ScriptedChat::new(vec![raw]);
        let conductor = Conductor::new(&dir);
        let mut calls = Vec::new();
        let mut replies = Vec::new();

        let session = conductor.run_observed(
            "finish",
            &chat,
            |turn| calls.push(turn),
            |turn, reply| replies.push((turn, reply.to_string())),
            |_| {},
        );

        assert!(session.finished);
        assert_eq!(calls, vec![1]);
        assert_eq!(replies, vec![(1, raw.to_string())]);
    }

    #[test]
    fn steps_carry_the_adepts_thought() {
        let dir = tmpdir(&[("f.txt", "x")]);
        let chat = ScriptedChat::new(vec![
            "peeking at the file first\n<act tool=\"read_file\"><arg name=\"path\">f.txt</arg></act>",
            "<act tool=\"finish\"><arg name=\"message\">done</arg></act>",
        ]);
        let c = Conductor::new(&dir);
        let session = c.run("t", &chat, |_| {});
        assert_eq!(session.steps[0].thought, "peeking at the file first");
        assert_eq!(session.steps[1].thought, "");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_rejects_garbage() {
        assert!(parse_action("no action here").is_none());
        assert!(parse_action("<act tool=\"nonsense\"></act>").is_none());
    }

    #[test]
    fn a_chain_of_sigils_executes_in_order() {
        let dir = tmpdir(&[("a.py", "def f():\n    return 1\n")]);
        let chat = ScriptedChat::new(vec![
            // read + edit + finish in ONE breath
            "fixing in one pass\n\
             <act tool=\"read_file\"><arg name=\"path\">a.py</arg></act>\n\
             <act tool=\"edit_file\"><arg name=\"path\">a.py</arg><arg name=\"find\">return 1</arg><arg name=\"replace\">return 2</arg></act>\n\
             <act tool=\"finish\"><arg name=\"message\">done</arg></act>",
        ]);
        let c = Conductor::new(&dir);
        let session = c.run("t", &chat, |_| {});
        assert!(session.finished);
        assert_eq!(session.steps.len(), 3);
        let now = std::fs::read_to_string(dir.join("a.py")).unwrap();
        assert!(now.contains("return 2"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_weighing_is_sealed_by_law() {
        let dir = tmpdir(&[("kaos_repro.py", "assert False\n"), ("src.py", "x = 1\n")]);
        let c = Conductor::new(&dir);
        // writes and edits to the forged gate are refused regardless of env
        let w = c.execute(&Tool::WriteFile {
            path: "kaos_repro.py".into(),
            contents: "assert True\n".into(),
        });
        assert!(w.contains("REFUSED") && w.contains("sealed"), "{w}");
        let e = c.execute(&Tool::EditFile {
            path: "kaos_repro.py".into(),
            find: "False".into(),
            replace: "True".into(),
        });
        assert!(e.contains("REFUSED"), "{e}");
        assert_eq!(
            std::fs::read_to_string(dir.join("kaos_repro.py")).unwrap(),
            "assert False\n"
        );
        // ordinary source stays writable
        let ok = c.execute(&Tool::EditFile {
            path: "src.py".into(),
            find: "x = 1".into(),
            replace: "x = 2".into(),
        });
        assert!(ok.contains("edited"), "{ok}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_sigil_halts_the_chain() {
        let dir = tmpdir(&[("a.py", "x = 1\n")]);
        let chat = ScriptedChat::new(vec![
            // The edit's find does not exist → negative observation → the
            // write_file that follows must NOT run.
            "<act tool=\"edit_file\"><arg name=\"path\">a.py</arg><arg name=\"find\">NOPE</arg><arg name=\"replace\">y</arg></act>\n\
             <act tool=\"write_file\"><arg name=\"path\">clobbered.txt</arg><arg name=\"contents\">should not exist</arg></act>",
            "<act tool=\"finish\"><arg name=\"message\">gave up</arg></act>",
        ]);
        let c = Conductor::new(&dir);
        let session = c.run("t", &chat, |_| {});
        assert!(session.finished);
        assert!(
            !dir.join("clobbered.txt").exists(),
            "chain must halt at the failure"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_repetition_ward_banishes_identical_rereads() {
        let dir = tmpdir(&[("f.txt", "the contents of f")]);
        let chat = ScriptedChat::new(vec![
            "<act tool=\"read_file\"><arg name=\"path\">f.txt</arg></act>",
            "<act tool=\"read_file\"><arg name=\"path\">f.txt</arg></act>",
            "<act tool=\"finish\"><arg name=\"message\">ok</arg></act>",
        ]);
        let c = Conductor::new(&dir);
        let session = c.run("t", &chat, |_| {});
        assert!(session.steps[0].observation.contains("the contents of f"));
        assert!(session.steps[1]
            .observation
            .contains("unchanged since your last read"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn edits_lift_the_repetition_ward() {
        let dir = tmpdir(&[("f.txt", "v1")]);
        let chat = ScriptedChat::new(vec![
            "<act tool=\"read_file\"><arg name=\"path\">f.txt</arg></act>",
            "<act tool=\"write_file\"><arg name=\"path\">f.txt</arg><arg name=\"contents\">v2</arg></act>",
            "<act tool=\"read_file\"><arg name=\"path\">f.txt</arg></act>",
            "<act tool=\"finish\"><arg name=\"message\">ok</arg></act>",
        ]);
        let c = Conductor::new(&dir);
        let session = c.run("t", &chat, |_| {});
        assert!(
            session.steps[2].observation.contains("v2"),
            "post-edit read must be fresh"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_actions_caps_the_flood() {
        let one = "<act tool=\"bash\"><arg name=\"cmd\">true</arg></act>\n";
        let flood = one.repeat(20);
        assert_eq!(
            parse_actions(&flood, MAX_ACTS_PER_TURN).len(),
            MAX_ACTS_PER_TURN
        );
        assert_eq!(parse_actions("nothing", MAX_ACTS_PER_TURN).len(), 0);
    }

    #[test]
    fn line_delta_counts_changes() {
        assert_eq!(line_delta("a\nb\nc", "a\nb\nc"), (0, 0));
        assert_eq!(line_delta("a\nb", "a\nb\nc"), (1, 0)); // added one
        assert_eq!(line_delta("a\nb\nc", "a\nc"), (0, 1)); // removed one
        assert_eq!(line_delta("return a - b", "return a + b"), (1, 1)); // changed line
    }

    #[test]
    fn execute_read_write_edit_bash() {
        let dir = tmpdir(&[("f.txt", "hello world\n")]);
        let c = Conductor::new(&dir);
        assert!(c
            .execute(&Tool::ReadFile {
                path: "f.txt".into()
            })
            .contains("hello world"));
        // A brand-new file reports "created"; an overwrite reports a "+A -B" delta.
        assert!(c
            .execute(&Tool::WriteFile {
                path: "g.txt".into(),
                contents: "abc".into()
            })
            .contains("created"));
        assert_eq!(std::fs::read_to_string(dir.join("g.txt")).unwrap(), "abc");
        assert!(c
            .execute(&Tool::WriteFile {
                path: "g.txt".into(),
                contents: "abc\nxyz".into()
            })
            .contains("+1"));
        let obs = c.execute(&Tool::EditFile {
            path: "f.txt".into(),
            find: "world".into(),
            replace: "kaos".into(),
        });
        assert!(obs.contains("replaced"));
        assert_eq!(
            std::fs::read_to_string(dir.join("f.txt")).unwrap(),
            "hello kaos\n"
        );
        assert!(c
            .execute(&Tool::Bash {
                cmd: "echo hi".into()
            })
            .contains("hi"));
        // edit that can't find its target is a clear error, not a silent pass.
        assert!(c
            .execute(&Tool::EditFile {
                path: "f.txt".into(),
                find: "ZZZ".into(),
                replace: "x".into()
            })
            .contains("error"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn full_scripted_session_fixes_a_bug_and_verifies() {
        // A real end-to-end loop with no model: the agent reads, edits, runs the
        // check, sees it pass, and finishes.
        let dir = tmpdir(&[
            ("sol.py", "def add(a, b):\n    return a - b\n"),
            (
                "check.py",
                "from sol import add\nassert add(2,3)==5\nprint('ok')\n",
            ),
        ]);
        let chat = ScriptedChat::new(vec![
            "<act tool=\"read_file\"><arg name=\"path\">sol.py</arg></act>",
            "<act tool=\"edit_file\"><arg name=\"path\">sol.py</arg><arg name=\"find\">a - b</arg><arg name=\"replace\">a + b</arg></act>",
            "<act tool=\"bash\"><arg name=\"cmd\">python3 check.py</arg></act>",
            "<act tool=\"finish\"><arg name=\"message\">fixed add and verified</arg></act>",
        ]);
        let c = Conductor::new(&dir);
        let session = c.run("fix add so the check passes", &chat, |_| {});
        assert!(session.finished, "agent should reach finish");
        assert_eq!(
            std::fs::read_to_string(dir.join("sol.py")).unwrap(),
            "def add(a, b):\n    return a + b\n"
        );
        // The bash step observed a passing check.
        assert!(session.steps.iter().any(|s| s.observation.contains("ok")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn edit_then_finish(find: &str, replace: &str) -> ScriptedChat {
        ScriptedChat::new(vec![
            Box::leak(format!(
                "<act tool=\"edit_file\"><arg name=\"path\">sol.txt</arg><arg name=\"find\">{find}</arg><arg name=\"replace\">{replace}</arg></act>"
            ).into_boxed_str()),
            "<act tool=\"finish\"><arg name=\"message\">done</arg></act>",
        ])
    }

    #[test]
    fn conclave_ships_consensus_verified_diff() {
        let dir = tmpdir(&[("sol.txt", "WRONG\n")]);
        let adepts: Vec<(String, Box<dyn Chat>)> = vec![
            (
                "Frater A".into(),
                Box::new(edit_then_finish("WRONG", "RIGHT")),
            ),
            (
                "Soror B".into(),
                Box::new(edit_then_finish("WRONG", "RIGHT")),
            ),
        ];
        let out = run_conclave(
            &dir,
            "make it right",
            Some("grep -q RIGHT sol.txt"),
            adepts,
            8,
            30,
            |_| {},
        )
        .unwrap();
        assert!(out.shipped);
        assert!(out.gated);
        // Both adepts converged on the same verified diff → consensus of 2.
        assert_eq!(out.attempts.iter().filter(|a| a.verified).count(), 2);
        assert_eq!(
            std::fs::read_to_string(dir.join("sol.txt")).unwrap(),
            "RIGHT\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn conclave_ships_nothing_when_the_gate_holds() {
        let dir = tmpdir(&[("sol.txt", "WRONG\n")]);
        let adepts: Vec<(String, Box<dyn Chat>)> = vec![
            (
                "A".into(),
                Box::new(edit_then_finish("WRONG", "STILL WRONG")),
            ),
            ("B".into(), Box::new(edit_then_finish("WRONG", "ALSO NO"))),
        ];
        let out = run_conclave(
            &dir,
            "t",
            Some("grep -q RIGHT sol.txt"),
            adepts,
            8,
            30,
            |_| {},
        )
        .unwrap();
        assert!(!out.shipped);
        // Target left untouched — never ship unverified.
        assert_eq!(
            std::fs::read_to_string(dir.join("sol.txt")).unwrap(),
            "WRONG\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn conclave_ships_deletions() {
        // A verified fix whose whole point is REMOVING a file: the shipped tree must
        // match the verified tree, so the deletion must ride the diff into `target`.
        let dir = tmpdir(&[("sol.txt", "keep\n"), ("doomed.txt", "delete me\n")]);
        let chat = ScriptedChat::new(vec![
            "<act tool=\"bash\"><arg name=\"cmd\">rm doomed.txt</arg></act>",
            "<act tool=\"finish\"><arg name=\"message\">the file is banished</arg></act>",
        ]);
        let adepts: Vec<(String, Box<dyn Chat>)> = vec![("A".into(), Box::new(chat))];
        let out = run_conclave(
            &dir,
            "delete doomed.txt",
            Some("test ! -e doomed.txt"),
            adepts,
            8,
            30,
            |_| {},
        )
        .unwrap();
        assert!(out.shipped, "a verified deletion is a shippable diff");
        assert!(
            !dir.join("doomed.txt").exists(),
            "the deletion must reach the target"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("sol.txt")).unwrap(),
            "keep\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn conclave_consensus_only_mode_without_a_gate() {
        // No verify command: an attempt is "verified" if it finished with a diff.
        let dir = tmpdir(&[("sol.txt", "WRONG\n")]);
        let adepts: Vec<(String, Box<dyn Chat>)> =
            vec![("A".into(), Box::new(edit_then_finish("WRONG", "MAYBE")))];
        let out = run_conclave(&dir, "t", None, adepts, 8, 30, |_| {}).unwrap();
        assert!(out.shipped);
        assert!(!out.gated, "no verifier → consensus-only, honestly ungated");
        assert_eq!(
            std::fs::read_to_string(dir.join("sol.txt")).unwrap(),
            "MAYBE\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A Chat that panics if consulted — proves an adjourned adept is never convened.
    struct MustNotSummon;
    impl Chat for MustNotSummon {
        fn respond(&self, _s: &str, _t: &str) -> Result<String, String> {
            panic!("this adept must never be summoned: the quorum had adjourned");
        }
        fn label(&self) -> String {
            "must-not-summon".into()
        }
    }

    #[test]
    fn conclave_adjourns_when_the_vote_is_beyond_overturning() {
        // k=3; the first two adepts converge on the same verified diff. The leader
        // has 2 votes, the best rival 0, remaining 1: 2 > 0 + 1 → settled. The
        // third adept must never be summoned, and the decision must equal a full run.
        let dir = tmpdir(&[("sol.txt", "WRONG\n")]);
        let adepts: Vec<(String, Box<dyn Chat>)> = vec![
            ("A".into(), Box::new(edit_then_finish("WRONG", "RIGHT"))),
            ("B".into(), Box::new(edit_then_finish("WRONG", "RIGHT"))),
            ("C".into(), Box::new(MustNotSummon)),
        ];
        let mut adjourned_at = None;
        let out = run_conclave(
            &dir,
            "t",
            Some("grep -q RIGHT sol.txt"),
            adepts,
            8,
            30,
            |ev| {
                if let ConclaveEvent::Adjourned { convened, k } = ev {
                    adjourned_at = Some((convened, k));
                }
            },
        )
        .unwrap();
        assert_eq!(
            adjourned_at,
            Some((2, 3)),
            "quorum should adjourn after two agreeing adepts"
        );
        assert_eq!(out.attempts.len(), 2, "the third charge is never fired");
        assert!(out.shipped);
        assert_eq!(
            std::fs::read_to_string(dir.join("sol.txt")).unwrap(),
            "RIGHT\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn contested_conclave_convenes_everyone() {
        // Disagreeing diffs: 1-1 with 1 remaining is a reachable tie — no adjournment.
        let dir = tmpdir(&[("sol.txt", "WRONG\n")]);
        let adepts: Vec<(String, Box<dyn Chat>)> = vec![
            ("A".into(), Box::new(edit_then_finish("WRONG", "RIGHT"))),
            ("B".into(), Box::new(edit_then_finish("WRONG", "RIGHT ish"))),
            ("C".into(), Box::new(edit_then_finish("WRONG", "RIGHT"))),
        ];
        let out = run_conclave(
            &dir,
            "t",
            Some("grep -q RIGHT sol.txt"),
            adepts,
            8,
            30,
            |_| {},
        )
        .unwrap();
        assert_eq!(out.attempts.len(), 3);
        assert_eq!(
            std::fs::read_to_string(dir.join("sol.txt")).unwrap(),
            "RIGHT\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_tools_cannot_escape_the_workspace() {
        let dir = tmpdir(&[("f.txt", "inside\n")]);
        let c = Conductor::new(&dir);
        let abs = c.execute(&Tool::WriteFile {
            path: "/tmp/kaos-escape.txt".into(),
            contents: "x".into(),
        });
        assert!(
            abs.contains("escapes the workspace"),
            "absolute write must be refused: {abs}"
        );
        let up = c.execute(&Tool::ReadFile {
            path: "../secrets".into(),
        });
        assert!(
            up.contains("escapes the workspace"),
            "traversal read must be refused: {up}"
        );
        let edit = c.execute(&Tool::EditFile {
            path: "../../x".into(),
            find: "a".into(),
            replace: "b".into(),
        });
        assert!(edit.contains("escapes the workspace"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bash_survives_output_larger_than_the_pipe_buffer() {
        // >64KB on stdout used to deadlock the un-drained pipe until the timeout.
        let dir = tmpdir(&[]);
        let mut c = Conductor::new(&dir);
        c.bash_timeout_s = 10;
        let obs = c.execute(&Tool::Bash {
            cmd: "yes x | head -c 200000; echo DONE_MARKER".into(),
        });
        assert!(
            obs.starts_with("exit 0"),
            "large output must not be a bogus timeout: {}",
            &obs[..40.min(obs.len())]
        );
        assert!(obs.contains("DONE_MARKER"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_breaking_edit_is_vetoed_and_rolled_back() {
        // The Inquisitor's veto (needs python3): an edit that leaves a .py file
        // unparsable is REFUSED and the file restored — the agent can never work
        // atop a syntactically dead workspace.
        let dir = tmpdir(&[("mod.py", "def f():\n    return 1\n")]);
        let c = Conductor::new(&dir);
        let bad = c.execute(&Tool::EditFile {
            path: "mod.py".into(),
            find: "return 1".into(),
            replace: "return 1 \\ oops".into(),
        });
        assert!(
            bad.contains("REFUSED"),
            "syntax break must be vetoed: {bad}"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("mod.py")).unwrap(),
            "def f():\n    return 1\n",
            "the file must be rolled back"
        );
        let good = c.execute(&Tool::EditFile {
            path: "mod.py".into(),
            find: "return 1".into(),
            replace: "return 2".into(),
        });
        assert!(!good.contains("REFUSED"), "a healthy edit passes: {good}");
        // Non-Python files are never lint-gated.
        let txt = c.execute(&Tool::WriteFile {
            path: "notes.txt".into(),
            contents: "\\ {".into(),
        });
        assert!(!txt.contains("REFUSED"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_breaking_write_is_vetoed_new_file_removed() {
        let dir = tmpdir(&[]);
        let c = Conductor::new(&dir);
        let obs = c.execute(&Tool::WriteFile {
            path: "new.py".into(),
            contents: "def broken(:\n    pass\n".into(),
        });
        assert!(obs.contains("REFUSED"), "{obs}");
        assert!(
            !dir.join("new.py").exists(),
            "a vetoed brand-new file is removed"
        );
        // A broken OVERWRITE restores the previous healthy contents.
        let ok = c.execute(&Tool::WriteFile {
            path: "new.py".into(),
            contents: "x = 1\n".into(),
        });
        assert!(!ok.contains("REFUSED"));
        let broke = c.execute(&Tool::WriteFile {
            path: "new.py".into(),
            contents: "x = = 1\n".into(),
        });
        assert!(broke.contains("REFUSED"));
        assert_eq!(
            std::fs::read_to_string(dir.join("new.py")).unwrap(),
            "x = 1\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn gate_divination_reads_the_project() {
        let dir = tmpdir(&[("tests.py", "print('ok')\n")]);
        assert_eq!(detect_gate(&dir).as_deref(), Some("python3 tests.py"));
        let _ = std::fs::remove_dir_all(&dir);
        let dir = tmpdir(&[("Cargo.toml", "[package]\nname=\"x\"\n")]);
        assert_eq!(detect_gate(&dir).as_deref(), Some("cargo test"));
        let _ = std::fs::remove_dir_all(&dir);
        let dir = tmpdir(&[("README.md", "nothing to divine")]);
        assert_eq!(detect_gate(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn adaptive_quorum_stops_at_first_verified_pass() {
        // Attempt 1 fixes the file -> the gate passes -> NO second attempt.
        let dir = tmpdir(&[("sol.txt", "WRONG\n")]);
        let mut summoned = 0usize;
        let out = run_adaptive(
            &dir,
            "make it right",
            "grep -q RIGHT sol.txt",
            |_| {
                summoned += 1;
                Box::new(edit_then_finish("WRONG", "RIGHT")) as Box<dyn Chat>
            },
            &[8, 8, 8, 8],
            30,
            |_, _| {},
            |_| {},
        );
        assert!(out.verified);
        assert_eq!(
            out.attempts, 1,
            "the quorum never grows past a passing Weighing"
        );
        // Two summonings: the working itself, then its lunar auditor.
        assert_eq!(summoned, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn adaptive_quorum_grows_on_failure_and_carries_the_verdict() {
        // Attempt 1 makes a wrong edit; attempt 2's intent must CARRY the gate's
        // verdict (probed via a wrapping Chat) and fixes what remains.
        let dir = tmpdir(&[("sol.txt", "WRONG\n")]);
        struct Probe {
            inner: ScriptedChat,
            saw: std::rc::Rc<std::cell::Cell<bool>>,
        }
        impl Chat for Probe {
            fn respond(&self, system: &str, transcript: &str) -> Result<String, String> {
                if transcript.contains("You have worked on this before") {
                    self.saw.set(true);
                }
                self.inner.respond(system, transcript)
            }
            fn label(&self) -> String {
                "probe".into()
            }
        }
        let saw = std::rc::Rc::new(std::cell::Cell::new(false));
        let saw_in = saw.clone();
        let mut n = 0usize;
        let out = run_adaptive(
            &dir,
            "make it right",
            "grep -q RIGHT sol.txt",
            move |_| {
                n += 1;
                if n == 1 {
                    Box::new(edit_then_finish("WRONG", "STILL WRONG")) as Box<dyn Chat>
                } else {
                    Box::new(Probe {
                        inner: edit_then_finish("STILL WRONG", "RIGHT"),
                        saw: saw_in.clone(),
                    }) as Box<dyn Chat>
                }
            },
            &[8, 8, 8, 8],
            30,
            |_, _| {},
            |_| {},
        );
        assert!(out.verified, "the second attempt lands it");
        assert_eq!(out.attempts, 2);
        assert!(saw.get(), "attempt 2 must receive the enchanted verdict");
        assert_eq!(
            std::fs::read_to_string(dir.join("sol.txt")).unwrap(),
            "RIGHT\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn budget_is_respected_when_never_finishing() {
        let dir = tmpdir(&[("f.txt", "x")]);
        // Always emits the same read; never finishes.
        let chat = ScriptedChat::new(vec![
            "<act tool=\"read_file\"><arg name=\"path\">f.txt</arg></act>";
            50
        ]);
        let mut c = Conductor::new(&dir);
        c.max_steps = 5;
        let session = c.run("loop forever", &chat, |_| {});
        assert!(!session.finished);
        assert_eq!(session.steps.len(), 5);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
