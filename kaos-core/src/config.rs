//! Persistent, non-secret Kaos configuration.
//!
//! On the first run Kaos creates `~/.config/kaos/config` (honouring
//! `XDG_CONFIG_HOME`) with every supported setting and its effective default.
//! Existing files are never replaced or automatically rewritten. Values seed
//! the process environment without overriding an explicit shell export.
//!
//! [`CONFIG_DOCS`] is the machine-readable reference for the same settings.
//! The terminal help, visual Settings tab, generated defaults file, and the
//! configuration guide should agree with this inventory. Secrets and private
//! child-process transport variables deliberately remain outside this module.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Every persistent, user-facing setting understood by Kaos. Runtime transport
/// variables (`KAOS_SESSION`, `KAOS_RESUME`, `KAOS_FOLD`) and provider secrets
/// intentionally live elsewhere.
pub const CONFIG_KEYS: &[&str] = &[
    "theme",
    "vim_mode",
    "KAOS_MODEL",
    "KAOS_TIMEOUT_S",
    "KAOS_CHAT_TIMEOUT_S",
    "KAOS_MAX_TOKENS",
    "KAOS_NUM_PREDICT",
    "KAOS_FABLE_FALLBACK_MODEL",
    "KAOS_PROVIDER_SORT",
    "KAOS_PROVIDER_ONLY",
    "OPENAI_BASE_URL",
    "OPENROUTER_BASE_URL",
    "OLLAMA_HOST",
    "KAOS_MAX_STEPS",
    "KAOS_HAND",
    "KAOS_PROTECT",
    "KAOS_NO_FORGE",
    "KAOS_NO_DREAM",
    "KAOS_NO_PARADIGM",
    "KAOS_CLAUDE_YOLO",
    "KAOS_MYTH",
    "KAOS_K",
    "KAOS_AGENTIC",
    "KAOS_ARENA",
    "KAOS_MAX_CONCURRENCY",
    "KAOS_BASH_TIMEOUT_S",
    "KAOS_GATE_TIMEOUT_S",
    "KAOS_QUIET",
    "KAOS_UNIT",
    "KAOS_BASE",
    "KAOS_RUNGS",
    "KAOS_REBIS_MAX_EXPANSIONS",
    "KAOS_REBIS_MAX_MODULES",
    "KAOS_REBIS_MAX_CALLS",
    "KAOS_REBIS_MAX_CONCURRENCY",
    "KAOS_REBIS_TIMEOUT_S",
    "KAOS_DEBUG",
];

/// The kind of value a configuration entry accepts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValueKind {
    /// A small closed set such as `dark` or `light`.
    Choice,
    /// A truth value written as `0`, `1`, `false`, or `true`.
    Boolean,
    /// An empty value has a third meaning in addition to true and false.
    TriState,
    /// A non-negative count or tuning value.
    Integer,
    /// A wall-clock duration expressed in seconds.
    DurationSeconds,
    /// A model name, path, list, or other free-form value.
    Text,
    /// A provider endpoint URL.
    Url,
    /// A Rebis expression evaluated by a Kaos host.
    Expression,
}

impl ValueKind {
    /// Short type text suitable for compact settings interfaces.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Choice => "choice",
            Self::Boolean => "boolean",
            Self::TriState => "tri-state",
            Self::Integer => "integer",
            Self::DurationSeconds => "seconds",
            Self::Text => "text",
            Self::Url => "URL",
            Self::Expression => "Rebis expression",
        }
    }
}

/// Documentation for one persistent, non-secret setting.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConfigDoc {
    /// The exact key written to the config file or exported to a child.
    pub key: &'static str,
    /// The input shape accepted by the setting.
    pub kind: ValueKind,
    /// A one-line explanation for filtered settings lists.
    pub summary: &'static str,
    /// The operational behavior and important edge cases.
    pub details: &'static str,
    /// A copyable example, including the key for clarity.
    pub example: &'static str,
}

/// Complete metadata for every key in [`CONFIG_KEYS`].
pub const CONFIG_DOCS: &[ConfigDoc] = &[
    ConfigDoc {
        key: "theme",
        kind: ValueKind::Choice,
        summary: "Palette used by terminal and visual surfaces.",
        details: "Accepts `dark` or `light`. It is file-backed rather than exported as an environment variable and the visual control applies it immediately.",
        example: "theme = light",
    },
    ConfigDoc {
        key: "vim_mode",
        kind: ValueKind::Boolean,
        summary: "Default Vim-style editing bindings.",
        details: "Controls the default embedded Rebis editor mode in terminal and visual surfaces. Existing editor panes may keep their current mode until reopened.",
        example: "vim_mode = true",
    },
    ConfigDoc {
        key: "KAOS_MODEL",
        kind: ValueKind::Text,
        summary: "Provider and model binding for model calls.",
        details: "Use `sim` for offline behavior or a provider:model form such as `ollama:qwen3:4b`, `openai:gpt-4o`, `openrouter:provider/model`, or `claude:sonnet`. The binding is shared by chat, agents, conclaves, and Rebis prompts.",
        example: "KAOS_MODEL = ollama:qwen3:4b",
    },
    ConfigDoc {
        key: "KAOS_TIMEOUT_S",
        kind: ValueKind::DurationSeconds,
        summary: "Timeout for one-shot completion calls.",
        details: "Sets the HTTP or subprocess wall timeout for direct completions such as `/cast`. It is not a total budget for a multi-step coding session.",
        example: "KAOS_TIMEOUT_S = 180",
    },
    ConfigDoc {
        key: "KAOS_CHAT_TIMEOUT_S",
        kind: ValueKind::DurationSeconds,
        summary: "Timeout for one chat or coding-agent model turn.",
        details: "A tool-using chat can make many turns, so this bounds one model response while the run keeps its own step budget. A positive value is required; invalid or zero values fall back to the one-shot timeout floor.",
        example: "KAOS_CHAT_TIMEOUT_S = 900",
    },
    ConfigDoc {
        key: "KAOS_MAX_TOKENS",
        kind: ValueKind::Integer,
        summary: "Maximum response-token budget for hosted providers.",
        details: "Sent as `max_tokens` to OpenAI-compatible APIs. Local Ollama generation uses `KAOS_NUM_PREDICT` instead; this value still governs hosted model calls.",
        example: "KAOS_MAX_TOKENS = 16384",
    },
    ConfigDoc {
        key: "KAOS_NUM_PREDICT",
        kind: ValueKind::Integer,
        summary: "Optional Ollama generation-token cap.",
        details: "When non-empty, sends Ollama's `num_predict` option. Leave it empty to let Ollama choose its own generation limit; this does not configure hosted providers.",
        example: "KAOS_NUM_PREDICT = 4096",
    },
    ConfigDoc {
        key: "KAOS_FABLE_FALLBACK_MODEL",
        kind: ValueKind::Text,
        summary: "Optional fallback for a refused Claude Fable request.",
        details: "Empty disables fallback. Set a Claude model tag such as `claude-opus-5` when the Fable route should retry a refusal with another model.",
        example: "KAOS_FABLE_FALLBACK_MODEL = claude-opus-5",
    },
    ConfigDoc {
        key: "KAOS_PROVIDER_SORT",
        kind: ValueKind::Text,
        summary: "OpenRouter provider preference ordering.",
        details: "Optional OpenRouter routing policy. Common values are `throughput`, `latency`, and `price`; empty leaves provider selection to OpenRouter.",
        example: "KAOS_PROVIDER_SORT = throughput",
    },
    ConfigDoc {
        key: "KAOS_PROVIDER_ONLY",
        kind: ValueKind::Text,
        summary: "OpenRouter provider allow-list.",
        details: "Optional comma-separated provider slugs. When set, OpenRouter is told to use only those providers and not fall back to another provider.",
        example: "KAOS_PROVIDER_ONLY = together,novita",
    },
    ConfigDoc {
        key: "OPENAI_BASE_URL",
        kind: ValueKind::Url,
        summary: "OpenAI-compatible API base URL.",
        details: "Changes the host used by the OpenAI provider while retaining the `/v1/chat/completions` path. Credentials still come from `OPENAI_API_KEY` or `kaos auth`.",
        example: "OPENAI_BASE_URL = https://api.example.test",
    },
    ConfigDoc {
        key: "OPENROUTER_BASE_URL",
        kind: ValueKind::Url,
        summary: "OpenRouter API base URL.",
        details: "Overrides the OpenRouter host root. The provider appends its versioned catalog and chat paths; credentials still come from `OPENROUTER_API_KEY` or `kaos auth`.",
        example: "OPENROUTER_BASE_URL = https://openrouter.ai/api",
    },
    ConfigDoc {
        key: "OLLAMA_HOST",
        kind: ValueKind::Url,
        summary: "Ollama server URL.",
        details: "Points local model calls at another Ollama server. A bare `host:port` is accepted and receives an `http://` scheme automatically.",
        example: "OLLAMA_HOST = http://127.0.0.1:11434",
    },
    ConfigDoc {
        key: "KAOS_MAX_STEPS",
        kind: ValueKind::Integer,
        summary: "Maximum tool/action loop steps in a coding agent.",
        details: "Each read, edit, command, or model-directed action consumes a step. The bound applies to one agent session; zero permits no action steps.",
        example: "KAOS_MAX_STEPS = 24",
    },
    ConfigDoc {
        key: "KAOS_HAND",
        kind: ValueKind::Boolean,
        summary: "Use native provider tool calling when available.",
        details: "With `1`, HTTP providers receive their native tool-calling schema instead of Kaos's parsed action protocol. It affects OpenAI, OpenRouter, and Ollama API paths; Claude CLI has its own tools.",
        example: "KAOS_HAND = true",
    },
    ConfigDoc {
        key: "KAOS_PROTECT",
        kind: ValueKind::Text,
        summary: "Comma-separated path fragments protected from edits.",
        details: "Reads remain allowed, but file-tool writes and edits whose paths contain a listed fragment are refused. Shell commands can still bypass this guard, so it is a mutation ward rather than a sandbox.",
        example: "KAOS_PROTECT = tests,fixtures/locked",
    },
    ConfigDoc {
        key: "KAOS_NO_FORGE",
        kind: ValueKind::Boolean,
        summary: "Disable automatic reproduction-script forging.",
        details: "With `1`, a coding run without an explicit validation gate skips the Forge phase that tries to create `kaos_repro.py` before solving the task.",
        example: "KAOS_NO_FORGE = true",
    },
    ConfigDoc {
        key: "KAOS_NO_DREAM",
        kind: ValueKind::Boolean,
        summary: "Disable the between-attempt Dream phase.",
        details: "With `1`, failed coding attempts do not make the short toolless hypothesis call used to seed the next spiral attempt.",
        example: "KAOS_NO_DREAM = false",
    },
    ConfigDoc {
        key: "KAOS_NO_PARADIGM",
        kind: ValueKind::Boolean,
        summary: "Disable paradigm rotation between coding attempts.",
        details: "With `1`, retries do not receive the prompts that force a different debugging hypothesis. The first attempt remains unchanged.",
        example: "KAOS_NO_PARADIGM = false",
    },
    ConfigDoc {
        key: "KAOS_CLAUDE_YOLO",
        kind: ValueKind::TriState,
        summary: "Claude CLI authority mode.",
        details: "Empty asks on the first coding task, `1` grants Claude shell access and skips permission prompts, and `0` uses edit acceptance without unrestricted shell authority. Kaos may set this in child runs after the user chooses.",
        example: "KAOS_CLAUDE_YOLO = 0",
    },
    ConfigDoc {
        key: "KAOS_MYTH",
        kind: ValueKind::Expression,
        summary: "Rebis orchestration expression used by `/conclave`.",
        details: "The expression is parsed by Kaos's myth language. `${KAOS_K}` is expanded while the config loads, so the default spread width follows the quorum setting. Override it to compose a different gather, vote, spread, or agentic flow.",
        example: "KAOS_MYTH = (gather vote (spread ${KAOS_K} fire))",
    },
    ConfigDoc {
        key: "KAOS_K",
        kind: ValueKind::Integer,
        summary: "Default number of conclave leaves.",
        details: "Controls the default myth's `(spread k fire)` width. Values below one are normalized to one when `/conclave` starts; custom `KAOS_MYTH` expressions may use their own spread width.",
        example: "KAOS_K = 5",
    },
    ConfigDoc {
        key: "KAOS_AGENTIC",
        kind: ValueKind::Boolean,
        summary: "Make conclave leaves full tool-using agents.",
        details: "With `1`, each myth leaf gets an isolated working copy and can read, edit, and run commands before its result is voted on. With `0`, leaves are ordinary model completions.",
        example: "KAOS_AGENTIC = true",
    },
    ConfigDoc {
        key: "KAOS_ARENA",
        kind: ValueKind::Text,
        summary: "Working tree used by agentic conclave leaves.",
        details: "Sets the root copied for each agentic leaf. The default `.` means the current directory of the `/conclave` process; file-tool writes are isolated before a winning diff is selected.",
        example: "KAOS_ARENA = /home/facu/code/project",
    },
    ConfigDoc {
        key: "KAOS_MAX_CONCURRENCY",
        kind: ValueKind::Integer,
        summary: "Maximum concurrent agentic conclave leaves.",
        details: "Bounds the semaphore shared by agentic myth sessions. Values below one are normalized to one. This is separate from `KAOS_REBIS_MAX_CONCURRENCY`, which bounds Rebis square branches.",
        example: "KAOS_MAX_CONCURRENCY = 2",
    },
    ConfigDoc {
        key: "KAOS_BASH_TIMEOUT_S",
        kind: ValueKind::DurationSeconds,
        summary: "Per-shell-action timeout for agentic leaves.",
        details: "Each agentic `bash` action is killed after this many seconds. A full validation gate has its own wider `KAOS_GATE_TIMEOUT_S` cap.",
        example: "KAOS_BASH_TIMEOUT_S = 900",
    },
    ConfigDoc {
        key: "KAOS_GATE_TIMEOUT_S",
        kind: ValueKind::DurationSeconds,
        summary: "Validation-gate wall timeout.",
        details: "Bounds one test or verification command used to weigh an agentic attempt. It is intentionally independent of the per-command shell cap because a gate may run an entire suite.",
        example: "KAOS_GATE_TIMEOUT_S = 1200",
    },
    ConfigDoc {
        key: "KAOS_QUIET",
        kind: ValueKind::Boolean,
        summary: "Suppress live agentic conclave progress.",
        details: "With `1`, leaf and gate status is not printed while the myth runs. It does not suppress the final verdict or diagnostics.",
        example: "KAOS_QUIET = true",
    },
    ConfigDoc {
        key: "KAOS_UNIT",
        kind: ValueKind::Integer,
        summary: "Twin-ladder charge per transcript step.",
        details: "Controls the character budget granted by one Fibonacci rung when Kaos compacts an agent transcript. Larger values preserve more context at the cost of a larger prompt.",
        example: "KAOS_UNIT = 1000",
    },
    ConfigDoc {
        key: "KAOS_BASE",
        kind: ValueKind::Integer,
        summary: "Twin-ladder middle charge floor.",
        details: "Sets the character budget retained by the rotting middle of a transcript before the two charged ends win. Larger values retain more middle context.",
        example: "KAOS_BASE = 700",
    },
    ConfigDoc {
        key: "KAOS_RUNGS",
        kind: ValueKind::Integer,
        summary: "Number of meaningful twin-ladder rungs.",
        details: "Controls how far the Fibonacci charge curve reaches from the first intent and newest observation. Extra transcript entries fall back to the middle charge.",
        example: "KAOS_RUNGS = 7",
    },
    ConfigDoc {
        key: "KAOS_REBIS_MAX_EXPANSIONS",
        kind: ValueKind::Integer,
        summary: "Maximum structural macro expansions per Rebis run.",
        details: "Counts calls to Rebis macros, including recursive expansion. The default protects the host from non-terminating definitions; `0` disables macro expansion rather than making it unlimited.",
        example: "KAOS_REBIS_MAX_EXPANSIONS = 512",
    },
    ConfigDoc {
        key: "KAOS_REBIS_MAX_MODULES",
        kind: ValueKind::Integer,
        summary: "Maximum distinct imported Rebis modules per run.",
        details: "Counts uncached module loads through `(# name)`. `0` disables imports; already-resolved definitions are not repeatedly charged as new module loads.",
        example: "KAOS_REBIS_MAX_MODULES = 128",
    },
    ConfigDoc {
        key: "KAOS_REBIS_MAX_CALLS",
        kind: ValueKind::Integer,
        summary: "Maximum model calls per Rebis process.",
        details: "Counts model-backed prompts across the whole orchestration, including branches and nested structures. `0` makes execution model-silent; it does not mean unlimited. Hosted TUI runs use this as a renewable slice.",
        example: "KAOS_REBIS_MAX_CALLS = 2048",
    },
    ConfigDoc {
        key: "KAOS_REBIS_MAX_CONCURRENCY",
        kind: ValueKind::Integer,
        summary: "Maximum parallel Rebis square branches.",
        details: "Only parallel orchestration uses this bound; sequential execution remains in program order. `1` and `0` both mean sequential, while nested squares each obey the same per-square bound.",
        example: "KAOS_REBIS_MAX_CONCURRENCY = 8",
    },
    ConfigDoc {
        key: "KAOS_REBIS_TIMEOUT_S",
        kind: ValueKind::DurationSeconds,
        summary: "Timeout for one model-backed Rebis turn.",
        details: "Bounds one prompt/answer exchange, not the entire recursive Rebis process. A run can contain many turns until its model-call limit or structural limits are reached.",
        example: "KAOS_REBIS_TIMEOUT_S = 900",
    },
    ConfigDoc {
        key: "KAOS_DEBUG",
        kind: ValueKind::Boolean,
        summary: "Enable diagnostic logging in shared components.",
        details: "With `1`, selected provider, conductor, and familiar paths emit extra diagnostics. It does not turn on model authority or change runtime limits.",
        example: "KAOS_DEBUG = true",
    },
];

/// Return the documentation for one persistent setting.
pub fn documentation(key: &str) -> Option<&'static ConfigDoc> {
    CONFIG_DOCS.iter().find(|doc| doc.key == key)
}

/// The complete file written when no config exists. Every key has a nearby
/// explanation so the file remains useful when edited from a terminal. Empty
/// values are meaningful defaults: they leave an optional override disabled.
/// `${KAOS_K}` is expanded while loading, so changing the quorum also changes
/// the default myth.
pub const DEFAULT_CONFIG: &str = r#"# Kaos configuration
# Uppercase shell environment variables override this file. Provider API keys
# belong in ~/.config/kaos/credentials and are managed with `kaos auth`.

# Rebis editor palette. `theme` accepts dark or light.
theme = dark
# Use Vim-style bindings in newly opened Rebis editors.
vim_mode = false

# Mind and provider selection. `sim` is offline; use provider:model for a live
# backend, for example `ollama:qwen3:4b` or `openrouter:provider/model`.
KAOS_MODEL = sim
# Seconds for one-shot completions such as `/cast`, not a whole agent run.
KAOS_TIMEOUT_S = 120
# Seconds for one model turn inside a tool-using chat or coding agent.
KAOS_CHAT_TIMEOUT_S = 600
# Maximum generated tokens for OpenAI-compatible providers.
KAOS_MAX_TOKENS = 8192
# Optional Ollama `num_predict`; empty keeps Ollama's own default.
KAOS_NUM_PREDICT =
# Optional Claude model used when the Fable route refuses a request.
KAOS_FABLE_FALLBACK_MODEL =
# Optional OpenRouter routing sort: throughput, latency, or price.
KAOS_PROVIDER_SORT =
# Optional comma-separated OpenRouter provider allow-list; no fallbacks.
KAOS_PROVIDER_ONLY =
# OpenAI-compatible, OpenRouter, and Ollama endpoint roots.
OPENAI_BASE_URL = https://api.openai.com
OPENROUTER_BASE_URL = https://openrouter.ai/api
OLLAMA_HOST = http://127.0.0.1:11434

# Agent working. This is the maximum action count for one coding session.
KAOS_MAX_STEPS = 14
# Native HTTP tool calling: 1 on, 0 off.
KAOS_HAND = 0
# Comma-separated path fragments whose file-tool writes are refused.
KAOS_PROTECT =
# Disable automatic reproduction-script forging.
KAOS_NO_FORGE = 0
# Disable the between-attempt hypothesis call.
KAOS_NO_DREAM = 0
# Disable paradigm rotation between retries.
KAOS_NO_PARADIGM = 0
# Claude authority: empty asks, 1 grants shell, 0 accepts edits without shell.
KAOS_CLAUDE_YOLO =

# Conclave and myth. `${KAOS_K}` follows the quorum below when expanded.
KAOS_K = 5
# Rebis orchestration expression used by `/conclave`.
KAOS_MYTH = (gather vote (spread ${KAOS_K} fire))
# Make myth leaves full read/edit/bash agents instead of completions.
KAOS_AGENTIC = 0
# Root copied for agentic leaves.
KAOS_ARENA = .
# Maximum concurrent agentic leaves; values below one become one.
KAOS_MAX_CONCURRENCY = 3
# Per-shell-action and validation-gate wall limits, in seconds.
KAOS_BASH_TIMEOUT_S = 600
KAOS_GATE_TIMEOUT_S = 300
# Suppress live agentic progress, not the final verdict.
KAOS_QUIET = 0

# Twin-ladder transcript compaction: per-rung charge, middle floor, and rung count.
KAOS_UNIT = 700
KAOS_BASE = 500
KAOS_RUNGS = 5

# Rebis runtime limits. Zero disables macro expansion, imports, or model calls;
# zero concurrency is sequential and zero timeout falls back to its safe default.
KAOS_REBIS_MAX_EXPANSIONS = 256
KAOS_REBIS_MAX_MODULES = 64
KAOS_REBIS_MAX_CALLS = 1024
KAOS_REBIS_MAX_CONCURRENCY = 4
# Seconds for one Rebis model turn, not the complete recursive process.
KAOS_REBIS_TIMEOUT_S = 600

# Shared-component diagnostics: 1 on, 0 off.
KAOS_DEBUG = 0
"#;

/// `~/.config/kaos/config`, honouring `XDG_CONFIG_HOME`.
pub fn path() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("kaos/config")
}

/// Create the complete defaults file if absent, then seed unset environment
/// variables from it. An existing file is read exactly as the user left it.
pub fn load() -> io::Result<PathBuf> {
    let path = path();
    ensure_config(&path)?;
    let values = read_path(&path)?;
    for key in CONFIG_KEYS.iter().copied().filter(|key| {
        key.starts_with("KAOS_")
            || matches!(
                *key,
                "OPENAI_BASE_URL" | "OPENROUTER_BASE_URL" | "OLLAMA_HOST"
            )
    }) {
        let Some(value) = values.get(key).filter(|value| !value.is_empty()) else {
            continue;
        };
        if std::env::var_os(key).is_none() {
            std::env::set_var(key, expand(value, &values));
        }
    }
    Ok(path)
}

/// Read a boolean setting. Environment values win for uppercase settings; this
/// also works before [`load`], which keeps Rebis workspace tests and embedders
/// independent of the binary entry point.
pub fn enabled(key: &str) -> bool {
    let value = if key.bytes().any(|byte| byte.is_ascii_uppercase()) {
        std::env::var(key).ok().or_else(|| read_value(key))
    } else {
        read_value(key)
    };
    value.as_deref().is_some_and(truthy)
}

/// Persist one setting while preserving the comments, order, and all unrelated
/// values in the file. Creating a setting also creates the full defaults file.
pub fn set_value(key: &str, value: &str) -> Result<PathBuf, String> {
    let path = path();
    ensure_config(&path)
        .map_err(|error| format!("could not create {}: {error}", path.display()))?;
    set_value_at(&path, key, value)
        .map_err(|error| format!("could not write {}: {error}", path.display()))?;
    Ok(path)
}

/// Replace the complete non-secret configuration with the documented defaults.
/// Provider credentials are stored separately and are never touched.
pub fn restore_defaults() -> Result<PathBuf, String> {
    let path = path();
    restore_defaults_at(&path)
        .map_err(|error| format!("could not restore {}: {error}", path.display()))?;
    Ok(path)
}

fn truthy(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// Read a string setting from the config file.
pub fn value(key: &str) -> Option<String> {
    read_value(key)
}

/// Read the complete persistent configuration as editable key/value data.
///
/// The visual settings surface uses this rather than parsing the file a
/// second time, so comments remain the file editor's concern while both
/// frontends agree on the effective set of configurable keys.
pub fn values() -> io::Result<BTreeMap<String, String>> {
    let path = path();
    ensure_config(&path)?;
    read_path(&path)
}

/// The documented default for one setting.
pub fn default_value(key: &str) -> Option<String> {
    parse(DEFAULT_CONFIG).remove(key)
}

fn read_value(key: &str) -> Option<String> {
    read_path(&path()).ok()?.remove(key)
}

fn read_path(path: &Path) -> io::Result<BTreeMap<String, String>> {
    fs::read_to_string(path).map(|text| parse(&text))
}

fn parse(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let (key, value) = line.split_once('=')?;
            let key = key.trim();
            (!key.is_empty()).then(|| (key.to_string(), unquote(value.trim()).to_string()))
        })
        .collect()
}

fn unquote(value: &str) -> &str {
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        if matches!(
            (bytes[0], bytes[value.len() - 1]),
            (b'"', b'"') | (b'\'', b'\'')
        ) {
            return &value[1..value.len() - 1];
        }
    }
    value
}

fn expand(value: &str, values: &BTreeMap<String, String>) -> String {
    let mut result = value.to_string();
    for _ in 0..CONFIG_KEYS.len() {
        let Some(start) = result.find("${") else {
            break;
        };
        let Some(relative_end) = result[start + 2..].find('}') else {
            break;
        };
        let end = start + 2 + relative_end;
        let key = &result[start + 2..end];
        let replacement = std::env::var(key)
            .ok()
            .or_else(|| values.get(key).cloned())
            .unwrap_or_default();
        result.replace_range(start..=end, &replacement);
    }
    result
}

fn ensure_config(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(mut file) => file.write_all(DEFAULT_CONFIG.as_bytes()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error),
    }
}

fn restore_defaults_at(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, DEFAULT_CONFIG)
}

fn set_value_at(path: &Path, key: &str, value: &str) -> io::Result<()> {
    let text = fs::read_to_string(path)?;
    let mut found = false;
    let mut lines = Vec::new();
    for line in text.lines() {
        let parsed_key = line
            .trim()
            .split_once('=')
            .map(|(candidate, _)| candidate.trim());
        let is_legacy_vim = key == "vim_mode" && parsed_key == Some("vim");
        if parsed_key == Some(key) {
            if !found {
                lines.push(format!("{key} = {value}"));
                found = true;
            }
        } else if !is_legacy_vim {
            lines.push(line.to_string());
        }
    }
    if !found {
        lines.push(format!("{key} = {value}"));
    }
    fs::write(path, format!("{}\n", lines.join("\n")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "kaos-config-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn defaults_name_every_supported_setting() {
        let values = parse(DEFAULT_CONFIG);
        assert_eq!(values.len(), CONFIG_KEYS.len());
        for key in CONFIG_KEYS {
            assert!(values.contains_key(*key), "missing default for {key}");
        }
        assert_eq!(
            expand(values.get("KAOS_MYTH").unwrap(), &values),
            "(gather vote (spread 5 fire))"
        );
        assert_eq!(
            values.get("KAOS_REBIS_TIMEOUT_S").map(String::as_str),
            Some("600")
        );
        assert_eq!(
            values.get("KAOS_CHAT_TIMEOUT_S").map(String::as_str),
            Some("600")
        );
        assert_eq!(values.get("theme").map(String::as_str), Some("dark"));
    }

    #[test]
    fn documentation_covers_exactly_the_persistent_inventory() {
        use std::collections::BTreeSet;

        let documented = CONFIG_DOCS
            .iter()
            .map(|doc| doc.key)
            .collect::<BTreeSet<_>>();
        let configured = CONFIG_KEYS.iter().copied().collect::<BTreeSet<_>>();
        assert_eq!(documented, configured);
        for doc in CONFIG_DOCS {
            assert!(!doc.summary.is_empty(), "missing summary for {}", doc.key);
            assert!(!doc.details.is_empty(), "missing details for {}", doc.key);
            assert!(!doc.example.is_empty(), "missing example for {}", doc.key);
        }
    }

    #[test]
    fn first_run_writes_defaults_but_existing_config_is_untouched() {
        let root = temp_path("first-run");
        let path = root.join("kaos/config");
        ensure_config(&path).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), DEFAULT_CONFIG);
        fs::write(&path, "KAOS_MODEL = claude\n").unwrap();
        ensure_config(&path).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "KAOS_MODEL = claude\n");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn setting_vim_preserves_the_complete_file_and_removes_legacy_alias() {
        let root = temp_path("set-value");
        let path = root.join("kaos/config");
        ensure_config(&path).unwrap();
        fs::write(&path, format!("vim = true\n{DEFAULT_CONFIG}")).unwrap();
        set_value_at(&path, "vim_mode", "true").unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(!text.lines().any(|line| line.starts_with("vim =")));
        assert_eq!(
            parse(&text).get("vim_mode").map(String::as_str),
            Some("true")
        );
        assert!(text.contains("KAOS_REBIS_MAX_CALLS = 1024"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn selected_model_replaces_the_remembered_default() {
        let root = temp_path("remember-model");
        let path = root.join("kaos/config");
        ensure_config(&path).unwrap();
        set_value_at(&path, "KAOS_MODEL", "ollama:qwen3:14b").unwrap();
        assert_eq!(
            read_path(&path)
                .unwrap()
                .get("KAOS_MODEL")
                .map(String::as_str),
            Some("ollama:qwen3:14b")
        );
        assert_eq!(
            fs::read_to_string(&path)
                .unwrap()
                .lines()
                .filter(|line| line.starts_with("KAOS_MODEL ="))
                .count(),
            1
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn restoring_replaces_custom_values_with_the_complete_defaults() {
        let root = temp_path("restore");
        let path = root.join("kaos/config");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "KAOS_MODEL = claude:opus\ncustom = retained?\n").unwrap();

        restore_defaults_at(&path).unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), DEFAULT_CONFIG);
        assert_eq!(parse(DEFAULT_CONFIG).len(), CONFIG_KEYS.len());
        let _ = fs::remove_dir_all(root);
    }
}
