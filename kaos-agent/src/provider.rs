//! Providers — the minds the Pact can summon, and how to switch between them.
//!
//! A [`Spec`] is `(kind, model)`: which backend, running which model. It is the one
//! place model selection lives, so the REPL, the TUI, and the agent all speak the
//! same language. Five kinds:
//!
//! | kind          | how it runs                       | credential                 |
//! |---------------|-----------------------------------|----------------------------|
//! | `Simulated`   | offline, no model                 | none                       |
//! | `ClaudeCli`   | shells out to the `claude` CLI    | the CLI's own auth (a Claude subscription) |
//! | `ClaudeApi`   | Anthropic Messages HTTP API       | `ANTHROPIC_API_KEY`        |
//! | `OpenAi`      | OpenAI Chat Completions HTTP API  | `OPENAI_API_KEY`           |
//! | `OpenRouter`  | openrouter.ai (OpenAI-compatible) | `OPENROUTER_API_KEY`       |
//! | `Ollama`      | shells out to local `ollama`      | none (local)               |
//!
//! On OpenAI: there is no separate "subscription" API — paid ChatGPT access is used
//! through the same `OPENAI_API_KEY`, which is what `OpenAi` uses. `base_url` can be
//! pointed at any OpenAI-compatible endpoint via `OPENAI_BASE_URL`.
//!
//! OpenRouter is one key to every hosted model; ids are `vendor/model`
//! (e.g. `openrouter:anthropic/claude-sonnet-4.5`). [`openrouter_models`] fetches
//! the live catalog. `OPENROUTER_BASE_URL` overrides the endpoint.
//!
//! A canonical string form round-trips through [`Spec::parse`]/[`Spec::canonical`]
//! (e.g. `openai:gpt-4o`, `claude-api:claude-sonnet-4-5`, `ollama:qwen2.5:3b`,
//! `claude`, `sim`) so the TUI can hand the choice to a one-shot subprocess via the
//! `KAOS_MODEL` env var.

use std::collections::BTreeSet;
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// Anthropic's public API model ID. This remains available through the explicit
/// `anthropic:` namespace; `claude:fable` instead uses the Claude CLI.
pub const FABLE_5_MODEL: &str = "claude-fable-5";

/// The one Opus. Every spelling — `opus`, `opus5`, `claude:opus`,
/// `anthropic:opus` — resolves here, on both the CLI and the API route, and it
/// is the recommended `KAOS_FABLE_FALLBACK_MODEL`. The CLI's own moving
/// `--model opus` alias is not reachable by design.
pub const OPUS_5_MODEL: &str = "claude-opus-5";

/// The model selectors shared by the terminal palette and the visual Settings
/// picker. Live Ollama names are appended by [`model_choices`].
pub const KNOWN_MODEL_CHOICES: &[&str] = &[
    "claude",
    "claude:sonnet",
    "claude:opus",
    "claude:opus5",
    "claude:haiku",
    "claude:fable",
    "anthropic",
    "openai",
    "openrouter",
    "ollama",
    "sim",
];

static OLLAMA_MODEL_CACHE: OnceLock<Mutex<Option<Vec<String>>>> = OnceLock::new();

fn ollama_model_cache() -> &'static Mutex<Option<Vec<String>>> {
    OLLAMA_MODEL_CACHE.get_or_init(|| Mutex::new(None))
}

/// Resolve the Ollama endpoint for every frontend and backend path. The main
/// binary normally seeds the environment from the config file, but the visual
/// standalone binary and library callers do not necessarily run that loader.
pub(crate) fn configured_ollama_host() -> Option<String> {
    std::env::var("OLLAMA_HOST")
        .ok()
        .filter(|host| !host.trim().is_empty())
        .or_else(|| kaos_core::config::value("OLLAMA_HOST").filter(|host| !host.trim().is_empty()))
}

fn configured_sampling(
    sampling: Option<crate::backend::Sampling>,
) -> Option<crate::backend::Sampling> {
    sampling.map(|mut sampling| {
        // An explicit `--think` run sets the environment before the child
        // starts. The config is the default for every other model call.
        sampling.think |= kaos_core::config::enabled("KAOS_THINK");
        sampling
    })
}

/// Parse the table printed by `ollama ls`. Keeping this separate from process
/// discovery makes the model picker deterministic to test and tolerant of the
/// CLI adding more columns later.
fn parse_ollama_ls(text: &str) -> Vec<String> {
    let mut names = BTreeSet::new();
    for line in text.lines() {
        let Some(name) = line.split_whitespace().next() else {
            continue;
        };
        if name.eq_ignore_ascii_case("name") {
            continue;
        }
        names.insert(name.to_string());
    }
    names.into_iter().collect()
}

/// Ask the active Ollama installation for its current local model list.
///
/// `ollama ls` inherits `OLLAMA_HOST`, so the same configured endpoint used by
/// model calls is also the endpoint shown in both frontends. When the setting
/// exists only in Kaos's config file (for example in the standalone visual
/// binary), it is passed explicitly to the child command.
pub fn ollama_models() -> Result<Vec<String>, String> {
    let mut command = Command::new("ollama");
    if let Some(host) = configured_ollama_host() {
        command.env("OLLAMA_HOST", host);
    }
    let output = command
        .arg("ls")
        .output()
        .map_err(|error| format!("could not run `ollama ls`: {error}"))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if detail.is_empty() {
            format!("`ollama ls` exited with {}", output.status)
        } else {
            format!("`ollama ls`: {detail}")
        });
    }
    Ok(parse_ollama_ls(&String::from_utf8_lossy(&output.stdout)))
}

/// Return the cached Ollama list, filling it once on first use. Autocomplete
/// runs during every frame/key event, so it must not launch a child repeatedly.
pub fn cached_ollama_models() -> Vec<String> {
    let cache = ollama_model_cache();
    let mut cached = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(models) = cached.as_ref() {
        return models.clone();
    }
    let models = ollama_models().unwrap_or_default();
    *cached = Some(models.clone());
    models
}

/// Refresh the cached Ollama catalog after a pull or endpoint change.
pub fn refresh_ollama_models() -> Result<Vec<String>, String> {
    let models = ollama_models()?;
    *ollama_model_cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(models.clone());
    Ok(models)
}

/// All model selectors shown by the shared visual picker. The current Ollama
/// names are prefixed exactly as `/model` expects.
pub fn model_choices() -> Vec<String> {
    let mut choices = KNOWN_MODEL_CHOICES
        .iter()
        .map(|choice| (*choice).to_string())
        .collect::<Vec<_>>();
    choices.extend(
        cached_ollama_models()
            .into_iter()
            .map(|model| format!("ollama:{model}")),
    );
    choices
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Simulated,
    ClaudeCli,
    ClaudeApi,
    OpenAi,
    OpenRouter,
    Ollama,
}

impl Kind {
    pub fn default_model(&self) -> &'static str {
        match self {
            Kind::Simulated => "-",
            Kind::ClaudeCli => "claude",
            Kind::ClaudeApi => "claude-sonnet-4-5",
            Kind::OpenAi => "gpt-4o",
            Kind::OpenRouter => "openrouter/auto",
            Kind::Ollama => "qwen2.5:3b",
        }
    }

    pub fn needs_key(&self) -> Option<&'static str> {
        match self {
            Kind::ClaudeApi => Some("ANTHROPIC_API_KEY"),
            Kind::OpenAi => Some("OPENAI_API_KEY"),
            Kind::OpenRouter => Some("OPENROUTER_API_KEY"),
            _ => None,
        }
    }
}

/// A fully-resolved choice of mind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spec {
    pub kind: Kind,
    pub model: String,
}

/// Where an ollama selector is served from.
///
/// A model may carry its own host — `ollama:llama3.2\\192.168.1.20:11434` — so
/// one Kaos can address several machines at once. Without it there is a single
/// global `OLLAMA_HOST`, and a rack of boxes with different models on them
/// cannot be reached from one program.
///
/// `\\` marks the host because it is the one punctuation character Rebis leaves
/// as an ordinary word character — `@` is the invariant operator and `:` and
/// `/` already appear inside selectors. It is the cheapest mark available and
/// this is the first thing to need one.
///
/// Precedence: the host written on the selector, then `OLLAMA_HOST`, then
/// localhost. The selector wins because it is the most specific thing said, and
/// a program that names a machine means that machine.
///
/// A bare `host:port` is fine; the scheme is filled in.
fn ollama_base(model: &str) -> String {
    let written = model.rsplit_once('\\').map(|(_, host)| host.to_string());
    let host = written
        .or_else(configured_ollama_host)
        .unwrap_or_else(|| "http://127.0.0.1:11434".into());
    if host.starts_with("http") {
        host
    } else {
        format!("http://{host}")
    }
}

/// The model name with any `\\host` stripped, which is what ollama is asked for.
fn ollama_model(model: &str) -> &str {
    model.rsplit_once('\\').map_or(model, |(name, _)| name)
}

impl Spec {
    pub fn new(kind: Kind, model: impl Into<String>) -> Spec {
        Spec {
            kind,
            model: model.into(),
        }
    }

    /// The default offline mind.
    pub fn simulated() -> Spec {
        Spec::new(Kind::Simulated, "-")
    }

    /// Parse a user/env token into a Spec. Accepts:
    ///   `sim` | `claude` (CLI) | `claude:fable` | `claude:opus` |
    ///   `claude-api[:model]` |
    ///   `anthropic[:model]` (`:fable` and `:opus` name the current ids) |
    ///   `openai[:model]` | `gpt-4o` (bare gpt* ⇒ openai) |
    ///   `openrouter[:vendor/model]` | `ollama[:model]` |
    ///   a bare model name ⇒ ollama with that model.
    pub fn parse(s: &str) -> Spec {
        let s = s.trim();
        let (head, rest) = match s.split_once(':') {
            Some((h, r)) => (h, Some(r)),
            None => (s, None),
        };
        let with = |kind: Kind| Spec::new(kind, rest.unwrap_or(kind.default_model()).to_string());
        let anthropic = || {
            let model = match rest.map(str::to_ascii_lowercase).as_deref() {
                Some("fable") | Some("fable5") | Some("fable-5") => FABLE_5_MODEL,
                Some("opus") | Some("opus5") | Some("opus-5") => OPUS_5_MODEL,
                _ => rest.unwrap_or(Kind::ClaudeApi.default_model()),
            };
            Spec::new(Kind::ClaudeApi, model)
        };
        match head.to_lowercase().as_str() {
            "sim" | "simulated" | "offline" | "" => Spec::simulated(),
            // `claude` and all its named variants use the subscription-backed
            // CLI. Historical Fable spellings normalize to its short CLI tag.
            "claude" => match rest.map(str::to_ascii_lowercase).as_deref() {
                Some("fable") | Some("fable5") | Some("fable-5") => {
                    Spec::new(Kind::ClaudeCli, "fable")
                }
                // There is one Opus: `opus`, `opus5` and `opus-5` all mean
                // Opus 5. The CLI's moving `--model opus` alias is deliberately
                // no longer reachable — a name that silently changes model
                // under a pinned config is not worth the spelling it saves.
                // `claude_tag` is where the short form becomes the exact id.
                Some("opus") | Some("opus5") | Some("opus-5") => {
                    Spec::new(Kind::ClaudeCli, "opus5")
                }
                _ => Spec::new(Kind::ClaudeCli, rest.unwrap_or("claude")),
            },
            "claude-cli" | "cli" => Spec::new(Kind::ClaudeCli, rest.unwrap_or("claude")),
            "fable" | "fable5" | "fable-5" if rest.is_none() => Spec::new(Kind::ClaudeCli, "fable"),
            "opus" | "opus5" | "opus-5" if rest.is_none() => Spec::new(Kind::ClaudeCli, "opus5"),
            "claude-api" | "anthropic" => anthropic(),
            "openai" | "chatgpt" | "gpt" => with(Kind::OpenAi),
            "openrouter" | "router" => with(Kind::OpenRouter),
            "ollama" | "local" => with(Kind::Ollama),
            other => {
                // Bare tokens: gpt*/o1*/o3* ⇒ OpenAI; claude-* ⇒ Anthropic API;
                // anything else ⇒ a local ollama model tag.
                if other.starts_with("gpt") || other.starts_with("o1") || other.starts_with("o3") {
                    Spec::new(Kind::OpenAi, s.to_string())
                } else if other.starts_with("claude-") {
                    Spec::new(Kind::ClaudeApi, s.to_string())
                } else {
                    Spec::new(Kind::Ollama, s.to_string())
                }
            }
        }
    }

    /// The canonical string form (round-trips through `parse`).
    pub fn canonical(&self) -> String {
        match self.kind {
            Kind::Simulated => "sim".to_string(),
            Kind::ClaudeCli => {
                if self.model == "claude" {
                    "claude".to_string()
                } else {
                    format!("claude:{}", self.model)
                }
            }
            Kind::ClaudeApi => format!("claude-api:{}", self.model),
            Kind::OpenAi => format!("openai:{}", self.model),
            Kind::OpenRouter => format!("openrouter:{}", self.model),
            Kind::Ollama => format!("ollama:{}", self.model),
        }
    }

    /// A short human label for the status bar.
    pub fn label(&self) -> String {
        match self.kind {
            Kind::Simulated => "sim (offline)".to_string(),
            Kind::ClaudeCli => {
                if self.model == "claude" {
                    "claude (cli)".to_string()
                } else {
                    format!("claude:{} (cli)", self.model)
                }
            }
            Kind::ClaudeApi => format!("anthropic:{}", self.model),
            Kind::OpenAi => format!("openai:{}", self.model),
            Kind::OpenRouter => format!("openrouter:{}", self.model),
            Kind::Ollama => format!("ollama:{}", self.model),
        }
    }

    /// The CLI --model tag: None for the CLI's own default, Some for a pinned
    /// inner model (`sonnet`, `opus`, `haiku`, or a full id).
    pub fn claude_tag(&self) -> Option<&str> {
        if self.kind != Kind::ClaudeCli || self.model == "claude" {
            return None;
        }
        // The one place a `--model` value is produced, and therefore the one
        // place a short spelling is translated. Every Opus spelling resolves to
        // the pinned id: the CLI would accept its own moving `opus` alias, but
        // passing it through would let the model change under a config that
        // names a version.
        Some(match self.model.as_str() {
            "opus" | "opus5" | "opus-5" => OPUS_5_MODEL,
            other => other,
        })
    }

    /// Can this mind actually run live? Reports the missing credential if not.
    pub fn readiness(&self) -> Result<(), String> {
        if let Some(var) = self.kind.needs_key() {
            if std::env::var(var).ok().filter(|v| !v.is_empty()).is_none() {
                return Err(format!("{var} is not set"));
            }
        }
        Ok(())
    }

    /// Like [`Spec::complete`], but with explicit sampling control where the mind
    /// supports it. Ollama honours temperature + seed via
    /// [`crate::backend::ollama_generate`]; the OpenAI-compatible HTTP kinds
    /// (OpenAI, OpenRouter) honour temperature + seed in the request body — this
    /// is what makes the spiral's POLARITY (solar/lunar universes) real on hosted
    /// minds. Other kinds — and `None` — fall back to [`Spec::complete`].
    /// A completion carrying files, for a prompt fired inside a Rebis `+`
    /// that framed with something loaded by `&:`.
    ///
    /// Everything about the request except the user message is what
    /// [`Self::complete_sampled`] would build, so this is that call with one
    /// substitution — see [`crate::attach`] for the substitution.
    ///
    /// A file the provider cannot carry is reported, never dropped: the
    /// refusals come back so the caller can show them, and the request is sent
    /// with what remains. An answer about a picture the model never saw is a
    /// worse outcome than an error.
    ///
    /// # Errors
    ///
    /// Whatever the underlying provider call returns.
    pub fn complete_attached(
        &self,
        system: &str,
        user: &str,
        files: &[rebis_lang::Attachment],
        timeout: Duration,
        sampling: Option<crate::backend::Sampling>,
    ) -> (Result<String, String>, Vec<String>) {
        let sampling = configured_sampling(sampling);
        let refusals = crate::attach::Wire::of(self.kind).refusals(files);
        if files.is_empty() {
            return (
                self.complete_sampled(system, user, timeout, sampling),
                refusals,
            );
        }
        let answer = match (self.kind, sampling) {
            #[cfg(feature = "api")]
            (Kind::Ollama, Some(s)) => {
                let prompt = format!("{system}\n\n{user}");
                crate::backend::ollama_generate_attached(&self.model, &prompt, files, timeout, s)
            }
            #[cfg(feature = "api")]
            (Kind::OpenAi, s) => self.openai_api_sampled_attached(system, user, files, timeout, s),
            #[cfg(feature = "api")]
            (Kind::OpenRouter, s) => {
                self.openrouter_api_sampled_attached(system, user, files, timeout, s)
            }
            #[cfg(feature = "api")]
            (Kind::ClaudeApi, _) => self.claude_api_attached(system, user, files, timeout),
            // The CLI reads the path the program named, and a simulated mind
            // reads nothing — neither needs the bytes on a wire.
            _ => self.complete_sampled(system, user, timeout, sampling),
        };
        (answer, refusals)
    }

    pub fn complete_sampled(
        &self,
        system: &str,
        user: &str,
        timeout: Duration,
        sampling: Option<crate::backend::Sampling>,
    ) -> Result<String, String> {
        let sampling = configured_sampling(sampling);
        match (self.kind, sampling) {
            (Kind::Ollama, Some(s)) => {
                let prompt = format!("{system}\n\n{user}");
                crate::backend::ollama_generate(&self.model, &prompt, timeout, s)
            }
            (Kind::OpenAi, Some(s)) => self.openai_api_sampled(system, user, timeout, Some(s)),
            (Kind::OpenRouter, Some(s)) => {
                self.openrouter_api_sampled(system, user, timeout, Some(s))
            }
            _ => self.complete(system, user, timeout),
        }
    }

    /// Like [`complete_sampled`](Self::complete_sampled), but for a local ollama
    /// mind it uses ONLY the HTTP endpoint — never the `ollama run` CLI fallback.
    /// A caller that must not spawn subprocesses (a web request handler) uses this
    /// so a degraded server returns a clean error instead of a pile of `ollama
    /// run` processes — and so the call's `Sampling` (token cap, think, seed) is
    /// actually honoured, since the CLI path ignores it. Non-ollama minds behave
    /// exactly as `complete_sampled`.
    #[cfg(feature = "api")]
    pub fn complete_sampled_http(
        &self,
        system: &str,
        user: &str,
        timeout: Duration,
        sampling: Option<crate::backend::Sampling>,
    ) -> Result<String, String> {
        let sampling = configured_sampling(sampling);
        match (self.kind, sampling) {
            (Kind::Ollama, Some(s)) => {
                let prompt = format!("{system}\n\n{user}");
                crate::backend::ollama_http(&self.model, &prompt, timeout, s)
            }
            _ => self.complete_sampled(system, user, timeout, sampling),
        }
    }

    /// Summon this mind for a single (system, user) completion.
    pub fn complete(&self, system: &str, user: &str, timeout: Duration) -> Result<String, String> {
        match self.kind {
            Kind::Simulated => Err(
                "simulated provider has no live model — /model claude:fable|claude|openai|ollama"
                    .into(),
            ),
            Kind::ClaudeCli => crate::backend::fire_claude_as(self.claude_tag(), user, system),
            Kind::Ollama => {
                let prompt = format!("{system}\n\n{user}");
                let sampling = configured_sampling(Some(crate::backend::Sampling::default()))
                    .expect("configured default sampling is present");
                crate::backend::ollama_generate(&self.model, &prompt, timeout, sampling)
            }
            Kind::ClaudeApi => self.claude_api(system, user, timeout),
            Kind::OpenAi => self.openai_api(system, user, timeout),
            Kind::OpenRouter => self.openrouter_api_sampled(
                system,
                user,
                timeout,
                configured_sampling(Some(crate::backend::Sampling::default())),
            ),
        }
    }

    /// One NATIVE tool-calling completion: structured messages + tool schemas,
    /// in the mind's own dialect. OpenAI-compatible hosts get `tools` on
    /// `/v1/chat/completions`; ollama gets `tools` on `/api/chat`. Returns the
    /// raw assistant message for [`crate::hand::parse_reply`].
    #[cfg(feature = "api")]
    pub fn complete_native(
        &self,
        messages: &serde_json::Value,
        tools: &serde_json::Value,
        timeout: Duration,
        sampling: Option<crate::backend::Sampling>,
    ) -> Result<serde_json::Value, String> {
        let sampling = configured_sampling(sampling);
        match self.kind {
            Kind::OpenRouter | Kind::OpenAi => {
                let (who, base, key) = if self.kind == Kind::OpenRouter {
                    let key = std::env::var("OPENROUTER_API_KEY")
                        .map_err(|_| "OPENROUTER_API_KEY is not set".to_string())?;
                    ("openrouter", openrouter_base(), key)
                } else {
                    let key = std::env::var("OPENAI_API_KEY")
                        .map_err(|_| "OPENAI_API_KEY is not set".to_string())?;
                    let base = std::env::var("OPENAI_BASE_URL")
                        .unwrap_or_else(|_| "https://api.openai.com".into());
                    ("openai", base, key)
                };
                let mut body = serde_json::json!({
                    "model": self.model,
                    "max_tokens": 8192,
                    "messages": messages,
                    "tools": tools,
                });
                if let Some(s) = sampling {
                    body["temperature"] = serde_json::json!(s.temperature);
                    if let Some(seed) = s.seed {
                        // Clamp to a non-negative i32: some OpenRouter providers
                        // (e.g. Phala) reject larger seeds with a 400, which would
                        // otherwise fail EVERY call. Same clamp as the ollama path.
                        body["seed"] = serde_json::json!((seed % (i32::MAX as u64)) as i64);
                    }
                    if who == "openrouter" {
                        body["reasoning"] = serde_json::json!({ "enabled": s.think });
                        apply_routing(&mut body);
                    }
                }
                let mut last_err = String::new();
                for attempt in 0..2 {
                    // Stall shear: a hung provider connection eats the FULL
                    // timeout, and a naive retry hangs again — the measured
                    // failure was whole walls burned at near-zero spend. The
                    // retry gets half the window: a live host answers well
                    // inside it; a dead one fails the attempt twice as fast.
                    let t = if attempt == 0 { timeout } else { timeout / 2 };
                    let resp = match ureq::agent()
                        .post(&format!("{base}/v1/chat/completions"))
                        .timeout(t)
                        .set("Authorization", &format!("Bearer {key}"))
                        .send_json(body.clone())
                    {
                        Ok(r) => r,
                        Err(ureq::Error::Status(code, resp)) => {
                            let detail: String = resp
                                .into_string()
                                .unwrap_or_default()
                                .chars()
                                .take(200)
                                .collect();
                            last_err = format!("{who}: HTTP {code}: {detail}");
                            if code == 400
                                && body["reasoning"]["enabled"] == serde_json::json!(false)
                            {
                                body["reasoning"] = serde_json::json!({ "effort": "low" });
                                continue;
                            }
                            if (400..500).contains(&code) && code != 429 {
                                return Err(last_err);
                            }
                            continue;
                        }
                        Err(e) => {
                            last_err = http_err(who, e);
                            continue;
                        }
                    };
                    match resp.into_json::<serde_json::Value>() {
                        Ok(v) => {
                            let msg = v["choices"][0]["message"].clone();
                            if msg.is_null() {
                                return Err(format!("{who}: no message in response: {v}"));
                            }
                            return Ok(msg);
                        }
                        Err(e) => {
                            last_err = format!("{who}: bad json: {e}");
                            continue;
                        }
                    }
                }
                Err(last_err)
            }
            Kind::Ollama => {
                let base = ollama_base(&self.model);
                let mut options = serde_json::json!({});
                let mut think = false;
                if let Some(s) = sampling {
                    options["temperature"] = serde_json::json!(s.temperature);
                    if let Some(seed) = s.seed {
                        options["seed"] = serde_json::json!((seed % (i32::MAX as u64)) as i64);
                    }
                    if let Some(n) = s.num_ctx {
                        options["num_ctx"] = serde_json::json!(n);
                    }
                    think = s.think;
                }
                let body = serde_json::json!({
                    "model": ollama_model(&self.model),
                    "messages": messages,
                    "tools": tools,
                    "stream": false,
                    "think": think,
                    "options": options,
                });
                let resp = ureq::agent()
                    .post(&format!("{base}/api/chat"))
                    .timeout(timeout)
                    .send_json(body)
                    // Name the host. A remote inference server that is off,
                    // unreachable or listening elsewhere is the commonest way
                    // this fails, and `ollama chat: …` alone does not say which
                    // machine was asked.
                    .map_err(|e| format!("ollama chat at {base}: {e}"))?;
                let v: serde_json::Value = resp
                    .into_json()
                    .map_err(|e| format!("ollama: bad json: {e}"))?;
                let msg = v["message"].clone();
                if msg.is_null() {
                    return Err(format!("ollama: no message in response: {v}"));
                }
                Ok(msg)
            }
            _ => Err("native tool-calling needs an HTTP mind (openrouter/openai/ollama)".into()),
        }
    }

    #[cfg(not(feature = "api"))]
    pub fn complete_native(
        &self,
        _messages: &(),
        _tools: &(),
        _timeout: Duration,
        _sampling: Option<crate::backend::Sampling>,
    ) -> Result<(), String> {
        Err("built without the `api` feature — native tool-calling unavailable".into())
    }

    // ── HTTP providers (only with the `api` feature) ────────────────

    #[cfg(feature = "api")]
    fn openai_api(&self, system: &str, user: &str, timeout: Duration) -> Result<String, String> {
        self.openai_api_sampled(system, user, timeout, None)
    }

    #[cfg(feature = "api")]
    fn openai_api_sampled(
        &self,
        system: &str,
        user: &str,
        timeout: Duration,
        sampling: Option<crate::backend::Sampling>,
    ) -> Result<String, String> {
        let key =
            std::env::var("OPENAI_API_KEY").map_err(|_| "OPENAI_API_KEY is not set".to_string())?;
        let base =
            std::env::var("OPENAI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com".into());
        self.chat_completions("openai", &base, &key, system, user, timeout, sampling)
    }

    #[cfg(feature = "api")]
    fn openrouter_api_sampled(
        &self,
        system: &str,
        user: &str,
        timeout: Duration,
        sampling: Option<crate::backend::Sampling>,
    ) -> Result<String, String> {
        self.openrouter_api_sampled_attached(system, user, &[], timeout, sampling)
    }

    #[cfg(feature = "api")]
    fn openrouter_api_sampled_attached(
        &self,
        system: &str,
        user: &str,
        files: &[rebis_lang::Attachment],
        timeout: Duration,
        sampling: Option<crate::backend::Sampling>,
    ) -> Result<String, String> {
        let key = std::env::var("OPENROUTER_API_KEY")
            .map_err(|_| "OPENROUTER_API_KEY is not set".to_string())?;
        let base = openrouter_base();
        self.chat_completions_attached(
            "openrouter",
            &base,
            &key,
            system,
            user,
            files,
            timeout,
            sampling,
        )
    }

    #[cfg(feature = "api")]
    fn openai_api_sampled_attached(
        &self,
        system: &str,
        user: &str,
        files: &[rebis_lang::Attachment],
        timeout: Duration,
        sampling: Option<crate::backend::Sampling>,
    ) -> Result<String, String> {
        let key =
            std::env::var("OPENAI_API_KEY").map_err(|_| "OPENAI_API_KEY is not set".to_string())?;
        self.chat_completions_attached(
            "openai",
            "https://api.openai.com",
            &key,
            system,
            user,
            files,
            timeout,
            sampling,
        )
    }

    /// One OpenAI-compatible Chat Completions call — the shape OpenAI, OpenRouter,
    /// and every "compatible" endpoint share. `base` is the host root; the
    /// `/v1/chat/completions` path is appended here. `sampling`, when given,
    /// pins temperature and (where the host honours it) seed — the spiral's
    /// polarity travels through here.
    #[cfg(feature = "api")]
    #[allow(clippy::too_many_arguments)]
    fn chat_completions(
        &self,
        who: &'static str,
        base: &str,
        key: &str,
        system: &str,
        user: &str,
        timeout: Duration,
        sampling: Option<crate::backend::Sampling>,
    ) -> Result<String, String> {
        self.chat_completions_attached(who, base, key, system, user, &[], timeout, sampling)
    }

    /// The real builder. OpenAI and OpenRouter speak one shape, so they share
    /// one request and one attachment path.
    #[cfg(feature = "api")]
    #[allow(clippy::too_many_arguments)]
    fn chat_completions_attached(
        &self,
        who: &'static str,
        base: &str,
        key: &str,
        system: &str,
        user: &str,
        files: &[rebis_lang::Attachment],
        timeout: Duration,
        sampling: Option<crate::backend::Sampling>,
    ) -> Result<String, String> {
        // The cap matters beyond cost: without it a degenerate long generation
        // buffers server-side past any read timeout and the call never returns.
        let max_tokens: u64 = std::env::var("KAOS_MAX_TOKENS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(8192);
        let mut body = serde_json::json!({
            "model": self.model,
            "max_tokens": max_tokens,
            "messages": [
                {"role": "system", "content": system},
                crate::attach::Wire::OpenAi.user_message(user, files)
            ],
        });
        if let Some(s) = sampling {
            body["temperature"] = serde_json::json!(s.temperature);
            if let Some(seed) = s.seed {
                // Clamp to a non-negative i32: some OpenRouter providers (e.g.
                // Phala) reject larger seeds with a 400 that fails EVERY call.
                // Same clamp as the ollama path.
                body["seed"] = serde_json::json!((seed % (i32::MAX as u64)) as i64);
            }
            // Reasoning control, SAME semantics as the ollama backend's
            // `"think": sampling.think`: the project's central measured lesson
            // (reasoning-budget control is load-bearing for agent loops) must
            // hold on every backend identically. Default Sampling is think:false,
            // so agent loops suppress deliberation everywhere; `.thinking()`
            // opts a draw back in. Only sent on openrouter — the unified knob;
            // bare OpenAI rejects unknown fields on some models.
            if who == "openrouter" {
                body["reasoning"] = serde_json::json!({ "enabled": s.think });
                apply_routing(&mut body);
            }
        }
        // One transient retry: a routed pool (OpenRouter especially) can stall a
        // single request; without a retry one slow read aborts a whole session.
        // The retry gets HALF the window (stall shear — see complete_native).
        let mut last_err = String::new();
        for attempt in 0..2 {
            let t = if attempt == 0 { timeout } else { timeout / 2 };
            let mut req = ureq::agent()
                .post(&format!("{base}/v1/chat/completions"))
                .timeout(t)
                .set("Authorization", &format!("Bearer {key}"));
            if who == "openrouter" {
                // Optional attribution headers OpenRouter asks apps to send.
                req = req.set("X-Title", "kaos");
            }
            let resp = match req.send_json(body.clone()) {
                Ok(r) => r,
                Err(ureq::Error::Status(code, resp)) => {
                    // 4xx will not improve on retry; 5xx/429 might.
                    let detail: String = resp
                        .into_string()
                        .unwrap_or_default()
                        .chars()
                        .take(200)
                        .collect();
                    last_err = format!("{who}: HTTP {code}: {detail}");
                    // Reasoning-mandatory models (kimi-k2.7-code) reject
                    // {"enabled": false} with a 400. Degrade to the weakest
                    // legal suppression — effort "low" — and retry once.
                    if code == 400 && body["reasoning"]["enabled"] == serde_json::json!(false) {
                        body["reasoning"] = serde_json::json!({ "effort": "low" });
                        continue;
                    }
                    if (400..500).contains(&code) && code != 429 {
                        return Err(last_err);
                    }
                    continue;
                }
                Err(e) => {
                    last_err = http_err(who, e);
                    continue;
                }
            };
            match resp.into_json::<serde_json::Value>() {
                Ok(v) => {
                    // strip inline <think> blocks, same as the ollama backend —
                    // some hosts serve reasoning models' deliberation inline.
                    return v["choices"][0]["message"]["content"]
                        .as_str()
                        .map(|s| crate::backend::strip_think(s).trim().to_string())
                        .ok_or_else(|| format!("{who}: no content in response: {v}"));
                }
                Err(e) => {
                    last_err = format!("{who}: bad json: {e}");
                    continue;
                }
            }
        }
        Err(last_err)
    }

    #[cfg(feature = "api")]
    fn claude_api(&self, system: &str, user: &str, timeout: Duration) -> Result<String, String> {
        self.claude_api_attached(system, user, &[], timeout)
    }

    /// The real builder. `claude_api` is this with nothing attached, which is
    /// every ordinary prompt — so one request shape serves both and a file
    /// changes exactly one line of it.
    #[cfg(feature = "api")]
    fn claude_api_attached(
        &self,
        system: &str,
        user: &str,
        files: &[rebis_lang::Attachment],
        timeout: Duration,
    ) -> Result<String, String> {
        let key = std::env::var("ANTHROPIC_API_KEY")
            .map_err(|_| "ANTHROPIC_API_KEY is not set".to_string())?;
        let mut body = serde_json::json!({
            "model": self.model,
            "max_tokens": 4096,
            "system": system,
            "messages": [crate::attach::Wire::Anthropic.user_message(user, files)],
        });
        let fallback = (self.model == FABLE_5_MODEL)
            .then(|| std::env::var("KAOS_FABLE_FALLBACK_MODEL").ok())
            .flatten()
            .filter(|model| !model.trim().is_empty() && model.trim() != FABLE_5_MODEL);
        if let Some(model) = fallback {
            body["fallbacks"] = serde_json::json!([{ "model": model.trim() }]);
        }
        let mut request = ureq::agent()
            .post("https://api.anthropic.com/v1/messages")
            .timeout(timeout)
            .set("x-api-key", &key)
            .set("anthropic-version", "2023-06-01");
        if body.get("fallbacks").is_some() {
            request = request.set("anthropic-beta", "server-side-fallback-2026-06-01");
        }
        let resp = request
            .send_json(body)
            .map_err(|e| http_err("anthropic", e))?;
        let v: serde_json::Value = resp
            .into_json()
            .map_err(|e| format!("anthropic: bad json: {e}"))?;
        parse_claude_response(&v)
    }

    #[cfg(not(feature = "api"))]
    fn openai_api(&self, _system: &str, _user: &str, _timeout: Duration) -> Result<String, String> {
        Err("built without the `api` feature — OpenAI unavailable".into())
    }

    #[cfg(not(feature = "api"))]
    fn openai_api_sampled(
        &self,
        _system: &str,
        _user: &str,
        _timeout: Duration,
        _sampling: Option<crate::backend::Sampling>,
    ) -> Result<String, String> {
        Err("built without the `api` feature — OpenAI unavailable".into())
    }

    #[cfg(not(feature = "api"))]
    fn claude_api(&self, _system: &str, _user: &str, _timeout: Duration) -> Result<String, String> {
        Err("built without the `api` feature — Anthropic API unavailable".into())
    }

    #[cfg(not(feature = "api"))]
    fn openrouter_api_sampled(
        &self,
        _system: &str,
        _user: &str,
        _timeout: Duration,
        _sampling: Option<crate::backend::Sampling>,
    ) -> Result<String, String> {
        Err("built without the `api` feature — OpenRouter unavailable".into())
    }
}

/// Decode a Messages response and, critically for Fable 5, branch on
/// `stop_reason` rather than mistaking an HTTP-200 refusal for empty output.
#[cfg(feature = "api")]
fn parse_claude_response(v: &serde_json::Value) -> Result<String, String> {
    if v["stop_reason"].as_str() == Some("refusal") {
        let category = v["stop_details"]["category"]
            .as_str()
            .map(|value| format!(" ({value})"))
            .unwrap_or_default();
        let explanation = v["stop_details"]["explanation"]
            .as_str()
            .unwrap_or("the request was declined by a safety classifier");
        return Err(format!(
            "anthropic: {} refused the request{category}: {explanation}; set \
             KAOS_FABLE_FALLBACK_MODEL={OPUS_5_MODEL} to opt into fallback",
            v["model"].as_str().unwrap_or(FABLE_5_MODEL)
        ));
    }

    // Fable fallback responses can contain non-text boundary blocks. Preserve
    // the established Kaos completion seam by concatenating only text blocks.
    let text: String = v["content"]
        .as_array()
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|block| block["text"].as_str())
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();
    if text.is_empty() {
        Err(format!("anthropic: no text in response: {v}"))
    } else {
        Ok(text.trim().to_string())
    }
}

/// The OpenRouter host root (`OPENROUTER_BASE_URL` to override); `/v1/...` paths
/// are appended by the callers.
#[cfg(feature = "api")]
fn openrouter_base() -> String {
    std::env::var("OPENROUTER_BASE_URL").unwrap_or_else(|_| "https://openrouter.ai/api".into())
}

/// OpenRouter provider routing (stall defence): `KAOS_PROVIDER_ONLY=slug[,slug]`
/// hard-pins to those providers with no fallback; else `KAOS_PROVIDER_SORT`
/// (`throughput` | `latency` | `price`) prefers fast providers over the slow
/// pools that stall long agentic calls. Measured motive: k2.7 runs died at
/// full-wall/near-zero-spend when routed to a hanging provider.
#[cfg(feature = "api")]
fn apply_routing(body: &mut serde_json::Value) {
    if let Ok(only) = std::env::var("KAOS_PROVIDER_ONLY") {
        let order: Vec<&str> = only
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        if !order.is_empty() {
            body["provider"] = serde_json::json!({ "order": order, "allow_fallbacks": false });
            return;
        }
    }
    if let Ok(sort) = std::env::var("KAOS_PROVIDER_SORT") {
        if !sort.is_empty() {
            body["provider"] = serde_json::json!({ "sort": sort });
        }
    }
}

/// One model from the OpenRouter catalog.
#[cfg(feature = "api")]
pub struct RouterModel {
    /// The `vendor/model` id — bind it with `/model openrouter:<id>`.
    pub id: String,
    /// Context window in tokens (0 when unreported).
    pub context: u64,
    /// Prompt price in USD per million tokens; None when dynamic or unreported.
    pub prompt_per_m: Option<f64>,
}

/// Fetch the live OpenRouter catalog (`GET /v1/models`, no key required),
/// sorted by id.
#[cfg(feature = "api")]
pub fn openrouter_models(timeout: Duration) -> Result<Vec<RouterModel>, String> {
    let mut req = ureq::agent()
        .get(&format!("{}/v1/models", openrouter_base()))
        .timeout(timeout);
    if let Some(key) = std::env::var("OPENROUTER_API_KEY")
        .ok()
        .filter(|k| !k.is_empty())
    {
        req = req.set("Authorization", &format!("Bearer {key}"));
    }
    let resp = req.call().map_err(|e| http_err("openrouter", e))?;
    let v: serde_json::Value = resp
        .into_json()
        .map_err(|e| format!("openrouter: bad json: {e}"))?;
    let mut models: Vec<RouterModel> = v["data"]
        .as_array()
        .ok_or_else(|| format!("openrouter: no model list in response: {v}"))?
        .iter()
        .filter_map(|m| {
            Some(RouterModel {
                id: m["id"].as_str()?.to_string(),
                context: m["context_length"].as_u64().unwrap_or(0),
                prompt_per_m: m["pricing"]["prompt"]
                    .as_str()
                    .and_then(|p| p.parse::<f64>().ok())
                    .filter(|p| *p >= 0.0)
                    .map(|p| p * 1e6),
            })
        })
        .collect();
    models.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(models)
}

#[cfg(feature = "api")]
fn http_err(who: &str, e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(code, resp) => {
            let detail = resp.into_string().unwrap_or_default();
            let detail = detail.chars().take(200).collect::<String>();
            format!("{who}: HTTP {code}: {detail}")
        }
        ureq::Error::Transport(t) => format!("{who}: transport error: {t}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_canonical_round_trip() {
        for s in [
            "sim",
            "claude",
            "claude:sonnet",
            "claude:fable",
            "claude-api:claude-sonnet-4-5",
            "claude-api:claude-fable-5",
            "openai:gpt-4o",
            "openrouter:meta-llama/llama-3.3-70b-instruct",
            "ollama:qwen2.5:3b",
        ] {
            let spec = Spec::parse(s);
            assert_eq!(spec.canonical(), s, "round-trip failed for {s}");
        }
    }

    #[test]
    fn parses_ollama_ls_names_without_header_or_duplicates() {
        assert_eq!(
            parse_ollama_ls(
                "NAME               ID            SIZE      MODIFIED\n\
                 qwen3:4b           abc           2 GB      now\n\
                 llama3.2:3b        def           2 GB      now\n\
                 qwen3:4b           abc           2 GB      now\n"
            ),
            vec!["llama3.2:3b".to_string(), "qwen3:4b".to_string()]
        );
    }

    #[test]
    fn model_choices_include_the_shared_selectors() {
        for choice in KNOWN_MODEL_CHOICES {
            assert!(
                model_choices().contains(&choice.to_string()),
                "missing {choice}"
            );
        }
    }

    #[test]
    fn bare_tokens_route_to_the_right_provider() {
        assert_eq!(Spec::parse("gpt-4o").kind, Kind::OpenAi);
        assert_eq!(Spec::parse("o3-mini").kind, Kind::OpenAi);
        assert_eq!(Spec::parse("claude-opus-5").kind, Kind::ClaudeApi);
        assert_eq!(Spec::parse("qwen2.5:3b").kind, Kind::Ollama);
        assert_eq!(Spec::parse("claude").kind, Kind::ClaudeCli); // the CLI, not the API
        assert_eq!(Spec::parse("fable5").kind, Kind::ClaudeCli);
        assert_eq!(Spec::parse("fable5").model, "fable");
        // There is one Opus. Every spelling lands on the pinned id, including
        // the bare `opus` that used to be the CLI's moving alias.
        for alias in [
            "claude:opus",
            "claude:opus5",
            "claude:opus-5",
            "opus",
            "opus5",
            "opus-5",
        ] {
            let spec = Spec::parse(alias);
            assert_eq!(spec.kind, Kind::ClaudeCli, "alias {alias}");
            assert_eq!(spec.canonical(), "claude:opus5", "alias {alias}");
            assert_eq!(
                spec.claude_tag(),
                Some(OPUS_5_MODEL),
                "{alias} must reach the CLI as the exact id it understands"
            );
        }
        // The API route is unaffected: `anthropic:opus5` is still the HTTP path.
        assert_eq!(Spec::parse("anthropic:opus5").kind, Kind::ClaudeApi);
        let fable = Spec::parse("claude:fable");
        assert_eq!(fable.kind, Kind::ClaudeCli);
        assert_eq!(fable.claude_tag(), Some("fable"));
        assert!(fable.readiness().is_ok());
        assert_eq!(Spec::parse("claude").claude_tag(), None);
    }

    #[test]
    fn aliases_and_defaults() {
        assert_eq!(Spec::parse("anthropic").kind, Kind::ClaudeApi);
        assert_eq!(Spec::parse("anthropic").model, "claude-sonnet-4-5");
        for alias in ["fable", "fable5", "fable-5", "claude:fable5"] {
            let spec = Spec::parse(alias);
            assert_eq!(spec.kind, Kind::ClaudeCli, "alias {alias}");
            assert_eq!(spec.model, "fable", "alias {alias}");
            assert_eq!(spec.canonical(), "claude:fable", "alias {alias}");
        }
        let api_fable = Spec::parse("anthropic:fable5");
        assert_eq!(api_fable.kind, Kind::ClaudeApi);
        assert_eq!(api_fable.model, FABLE_5_MODEL);
        for alias in ["anthropic:opus", "anthropic:opus5", "claude-api:opus-5"] {
            let spec = Spec::parse(alias);
            assert_eq!(spec.kind, Kind::ClaudeApi, "alias {alias}");
            assert_eq!(spec.model, OPUS_5_MODEL, "alias {alias}");
        }
        // `claude:opus` routes through the CLI, and its short spelling now
        // pins Opus 5 rather than naming the CLI's moving alias.
        let cli_opus = Spec::parse("claude:opus");
        assert_eq!(cli_opus.kind, Kind::ClaudeCli);
        assert_eq!(cli_opus.model, "opus5");
        assert_eq!(cli_opus.claude_tag(), Some(OPUS_5_MODEL));
        assert_eq!(Spec::parse("chatgpt").kind, Kind::OpenAi);
        assert_eq!(Spec::parse("openai").model, "gpt-4o");
        assert_eq!(Spec::parse("openrouter").kind, Kind::OpenRouter);
        assert_eq!(Spec::parse("openrouter").model, "openrouter/auto");
        assert_eq!(
            Spec::parse("openrouter:deepseek/deepseek-chat").model,
            "deepseek/deepseek-chat"
        );
        assert_eq!(Spec::parse("").kind, Kind::Simulated);
    }

    #[test]
    fn readiness_reports_missing_key() {
        // ClaudeCli / Ollama / Simulated never need a key.
        assert!(Spec::new(Kind::ClaudeCli, "claude").readiness().is_ok());
        // The API kinds require a key; in this env they are unset.
        if std::env::var("OPENAI_API_KEY").is_err() {
            assert!(Spec::parse("openai:gpt-4o").readiness().is_err());
        }
    }

    #[cfg(feature = "api")]
    #[test]
    fn claude_response_reports_fable_refusals_instead_of_empty_output() {
        let refusal = serde_json::json!({
            "model": FABLE_5_MODEL,
            "content": [],
            "stop_reason": "refusal",
            "stop_details": {
                "category": "cyber",
                "explanation": "This request was declined."
            }
        });
        let error = parse_claude_response(&refusal).unwrap_err();
        assert!(error.contains("claude-fable-5 refused"));
        assert!(error.contains("(cyber)"));
        assert!(error.contains("KAOS_FABLE_FALLBACK_MODEL"));
    }

    #[cfg(feature = "api")]
    #[test]
    fn claude_response_ignores_fallback_boundaries_and_joins_text() {
        let response = serde_json::json!({
            "model": OPUS_5_MODEL,
            "stop_reason": "end_turn",
            "content": [
                {"type": "fallback", "from": {"model": FABLE_5_MODEL}, "to": {"model": OPUS_5_MODEL}},
                {"type": "text", "text": " completed "}
            ]
        });
        assert_eq!(parse_claude_response(&response).unwrap(), "completed");
    }
}

#[cfg(test)]
mod endpoints {
    use super::*;

    /// A model may name the machine that serves it, so one Kaos reaches a rack.
    #[test]
    fn a_selector_can_carry_its_own_host() {
        std::env::remove_var("OLLAMA_HOST");
        assert_eq!(
            ollama_base("llama3.2\\192.168.1.20:11434"),
            "http://192.168.1.20:11434"
        );
        // The host is not part of what ollama is asked for.
        assert_eq!(ollama_model("llama3.2\\192.168.1.20:11434"), "llama3.2");
        // A bare host:port gets a scheme; a full URL is left alone.
        assert_eq!(ollama_base("m\\10.0.0.5:11434"), "http://10.0.0.5:11434");
        assert_eq!(
            ollama_base("m\\https://box.local:11434"),
            "https://box.local:11434"
        );
    }

    /// Precedence: what the selector says, then the environment, then localhost.
    /// The selector wins because a program that names a machine means it.
    #[test]
    fn the_selector_outranks_the_environment_which_outranks_localhost() {
        std::env::remove_var("OLLAMA_HOST");
        assert_eq!(ollama_base("llama3.2"), "http://127.0.0.1:11434");

        std::env::set_var("OLLAMA_HOST", "192.168.1.9:11434");
        assert_eq!(ollama_base("llama3.2"), "http://192.168.1.9:11434");
        assert_eq!(
            ollama_base("llama3.2\\192.168.1.20:11434"),
            "http://192.168.1.20:11434",
            "the selector is the most specific thing said"
        );
        std::env::remove_var("OLLAMA_HOST");
    }

    /// A model tag with no host is untouched — every existing selector keeps
    /// meaning exactly what it meant.
    #[test]
    fn an_ordinary_model_tag_is_unchanged() {
        assert_eq!(ollama_model("qwen3:14b"), "qwen3:14b");
        assert_eq!(ollama_model("llama3.2:3b"), "llama3.2:3b");
    }
}
