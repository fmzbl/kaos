//! State and metadata for the visual Settings tab.
//!
//! Values are not duplicated into a visual-only preferences file. Persistent
//! edits go through `kaos_core::config`, the same store `/config` and
//! `/theme` use in the terminal application.

use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) enum Group {
    Appearance,
    Mind,
    Agent,
    Conclave,
    Runtime,
    Diagnostics,
}

impl Group {
    pub(crate) const ALL: [Self; 6] = [
        Self::Appearance,
        Self::Mind,
        Self::Agent,
        Self::Conclave,
        Self::Runtime,
        Self::Diagnostics,
    ];

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Appearance => "APPEARANCE & EDITOR",
            Self::Mind => "MIND & PROVIDERS",
            Self::Agent => "AGENT WORKING",
            Self::Conclave => "CONCLAVE & MYTH",
            Self::Runtime => "REBIS & RUNTIME",
            Self::Diagnostics => "DIAGNOSTICS",
        }
    }
}

pub(crate) fn group(key: &str) -> Group {
    match key {
        "theme" | "vim_mode" => Group::Appearance,
        "KAOS_MODEL"
        | "KAOS_TIMEOUT_S"
        | "KAOS_CHAT_TIMEOUT_S"
        | "KAOS_MAX_TOKENS"
        | "KAOS_NUM_PREDICT"
        | "KAOS_FABLE_FALLBACK_MODEL"
        | "KAOS_PROVIDER_SORT"
        | "KAOS_PROVIDER_ONLY"
        | "OPENAI_BASE_URL"
        | "OPENROUTER_BASE_URL"
        | "OLLAMA_HOST" => Group::Mind,
        "KAOS_MAX_STEPS" | "KAOS_HAND" | "KAOS_PROTECT" | "KAOS_NO_FORGE" | "KAOS_NO_DREAM"
        | "KAOS_NO_PARADIGM" | "KAOS_CLAUDE_YOLO" => Group::Agent,
        "KAOS_MYTH"
        | "KAOS_K"
        | "KAOS_AGENTIC"
        | "KAOS_ARENA"
        | "KAOS_MAX_CONCURRENCY"
        | "KAOS_BASH_TIMEOUT_S"
        | "KAOS_GATE_TIMEOUT_S"
        | "KAOS_QUIET" => Group::Conclave,
        "KAOS_DEBUG" => Group::Diagnostics,
        _ => Group::Runtime,
    }
}

pub(crate) fn description(key: &str) -> &'static str {
    match key {
        "theme" => "Shared terminal and visual palette: dark or light.",
        "vim_mode" => "Default terminal and visual Rebis editor bindings.",
        "KAOS_MODEL" => "Provider and model used by chat, agents, and live Rebis runs.",
        "KAOS_TIMEOUT_S" => "Timeout for one-shot completions.",
        "KAOS_CHAT_TIMEOUT_S" => "Timeout for one tool-using model turn.",
        "KAOS_MAX_TOKENS" => "Maximum response-token budget.",
        "KAOS_NUM_PREDICT" => "Optional local-model prediction limit.",
        "KAOS_FABLE_FALLBACK_MODEL" => "Optional fallback when the Fable route is unavailable.",
        "KAOS_PROVIDER_SORT" => "OpenRouter provider ordering.",
        "KAOS_PROVIDER_ONLY" => "Comma-separated OpenRouter provider allow-list.",
        "OPENAI_BASE_URL" => "OpenAI-compatible API base URL.",
        "OPENROUTER_BASE_URL" => "OpenRouter API base URL.",
        "OLLAMA_HOST" => "Ollama server URL.",
        "KAOS_MAX_STEPS" => "Maximum tool/action loop steps.",
        "KAOS_HAND" => "Use a provider's native tool-calling hand.",
        "KAOS_PROTECT" => "Comma-separated path fragments protected from edits.",
        "KAOS_NO_FORGE" => "Disable the forge stage.",
        "KAOS_NO_DREAM" => "Disable the dream stage.",
        "KAOS_NO_PARADIGM" => "Disable paradigm context.",
        "KAOS_CLAUDE_YOLO" => "Default Claude CLI authority: empty asks, 1 shell, 0 edits only.",
        "KAOS_MYTH" => "Conclave orchestration expression.",
        "KAOS_K" => "Default conclave quorum.",
        "KAOS_AGENTIC" => "Make myth leaves tool-using agents.",
        "KAOS_ARENA" => "Working tree used by agentic myths.",
        "KAOS_MAX_CONCURRENCY" => "Maximum concurrent conclave leaves.",
        "KAOS_BASH_TIMEOUT_S" => "Shell-action timeout.",
        "KAOS_GATE_TIMEOUT_S" => "Validation-gate timeout.",
        "KAOS_QUIET" => "Suppress live agentic progress.",
        "KAOS_UNIT" => "Twin-ladder charge per step.",
        "KAOS_BASE" => "Twin-ladder middle charge.",
        "KAOS_RUNGS" => "Twin-ladder length.",
        "KAOS_REBIS_MAX_EXPANSIONS" => "Maximum macro expansions; 0 is unlimited.",
        "KAOS_REBIS_MAX_MODULES" => "Maximum imported modules; 0 is unlimited.",
        "KAOS_REBIS_MAX_CALLS" => "Maximum model calls per Rebis run; 0 is unlimited.",
        "KAOS_REBIS_MAX_CONCURRENCY" => "Maximum parallel Rebis branches.",
        "KAOS_REBIS_TIMEOUT_S" => "Timeout for one Rebis model turn.",
        "KAOS_DEBUG" => "Enable diagnostic output.",
        _ => "Persistent Kaos setting.",
    }
}

pub(crate) fn is_boolean(key: &str) -> bool {
    matches!(
        key,
        "vim_mode"
            | "KAOS_HAND"
            | "KAOS_NO_FORGE"
            | "KAOS_NO_DREAM"
            | "KAOS_NO_PARADIGM"
            | "KAOS_AGENTIC"
            | "KAOS_QUIET"
            | "KAOS_DEBUG"
    )
}

#[derive(Default)]
pub(crate) struct SettingsPane {
    pub(crate) filter: String,
    pub(crate) values: BTreeMap<String, String>,
    saved: BTreeMap<String, String>,
    pub(crate) notice: Option<String>,
}

impl SettingsPane {
    pub(crate) fn load() -> Self {
        match kaos_core::config::values() {
            Ok(mut values) => {
                for key in kaos_core::config::CONFIG_KEYS {
                    values.entry((*key).to_string()).or_insert_with(|| {
                        kaos_core::config::default_value(key).unwrap_or_default()
                    });
                }
                Self {
                    saved: values.clone(),
                    values,
                    ..Self::default()
                }
            }
            Err(error) => Self {
                notice: Some(format!("could not read configuration: {error}")),
                ..Self::default()
            },
        }
    }

    pub(crate) fn dirty(&self) -> usize {
        self.values
            .iter()
            .filter(|(key, value)| self.saved.get(*key) != Some(*value))
            .count()
    }

    pub(crate) fn reload(&mut self) {
        *self = Self::load();
        self.notice = Some("reloaded persistent configuration".to_string());
    }

    pub(crate) fn save(&mut self) -> Result<usize, String> {
        let changed = self
            .values
            .iter()
            .filter(|(key, value)| self.saved.get(*key) != Some(*value))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<Vec<_>>();
        for (key, value) in &changed {
            kaos_core::config::set_value(key, value)?;
        }
        self.saved = self.values.clone();
        Ok(changed.len())
    }

    pub(crate) fn save_key(&mut self, key: &str) -> Result<(), String> {
        let value = self.values.get(key).cloned().unwrap_or_default();
        kaos_core::config::set_value(key, &value)?;
        self.saved.insert(key.to_string(), value);
        Ok(())
    }

    pub(crate) fn restore(&mut self) -> Result<(), String> {
        kaos_core::config::restore_defaults()?;
        *self = Self::load();
        self.notice = Some("restored documented defaults".to_string());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_persistent_key_has_a_visual_group_and_description() {
        for key in kaos_core::config::CONFIG_KEYS {
            assert!(Group::ALL.contains(&group(key)));
            assert!(!description(key).is_empty());
        }
    }

    #[test]
    fn theme_is_an_appearance_setting() {
        assert_eq!(group("theme"), Group::Appearance);
        assert_eq!(
            kaos_core::config::default_value("theme").as_deref(),
            Some("dark")
        );
    }
}
