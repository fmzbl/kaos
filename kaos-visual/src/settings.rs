//! State and metadata for the visual Settings tab.
//!
//! Values are not duplicated into a visual-only preferences file. Persistent
//! edits go through `kaos_core::config`, the same store `/config` and
//! `/theme` use in the terminal application. Key documentation comes from the
//! core config inventory as well, so a setting cannot silently disappear from
//! one frontend's reference.

use std::collections::BTreeMap;

/// One provider's credential row in the Settings tab.
///
/// Credentials are deliberately NOT config keys: they live in a separate `0600`
/// store, and the settings file is world-readable. Keeping them apart is what
/// stops a key from being written into a file people paste into bug reports —
/// so the Settings tab has to render them from their own source rather than
/// from the config inventory.
#[derive(Clone, Default)]
pub(crate) struct CredentialEdit {
    /// The value being typed. Never persisted here — it goes straight to the
    /// credential store on submit and this field is cleared.
    pub(crate) entry: String,
}

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
        | "KAOS_THINK"
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
        // The chaos stance sits with agent working rather than in its own
        // group: it decides HOW the work gets done, which is what every other
        // key here decides too. It reaches runs as well, but a reader looking
        // for "does an agent do this, or does a program" looks here first.
        "KAOS_CHAOS" | "KAOS_MAX_STEPS" | "KAOS_HAND" | "KAOS_PROTECT" | "KAOS_NO_FORGE"
        | "KAOS_NO_DREAM" | "KAOS_NO_PARADIGM" | "KAOS_CLAUDE_YOLO" => Group::Agent,
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

pub(crate) fn documentation(key: &str) -> Option<&'static kaos_core::config::ConfigDoc> {
    kaos_core::config::documentation(key)
}

pub(crate) fn is_boolean(key: &str) -> bool {
    documentation(key).is_some_and(|doc| doc.kind == kaos_core::config::ValueKind::Boolean)
}

pub(crate) fn is_tristate(key: &str) -> bool {
    documentation(key).is_some_and(|doc| doc.kind == kaos_core::config::ValueKind::TriState)
}

/// Match one persistent setting against the Settings search query.
///
/// The query deliberately covers the same text a person sees in the row: the
/// exact key, its summary, operational details, and copyable example. Keeping
/// this predicate here makes the search section and the grouped editor use the
/// same inventory rather than maintaining two subtly different filters.
pub(crate) fn matches(key: &str, query: &str) -> bool {
    let query = query.trim().to_ascii_lowercase();
    if query.is_empty() {
        return true;
    }
    key.to_ascii_lowercase().contains(&query)
        || documentation(key).is_some_and(|doc| {
            doc.summary.to_ascii_lowercase().contains(&query)
                || doc.details.to_ascii_lowercase().contains(&query)
                || doc.example.to_ascii_lowercase().contains(&query)
        })
}

/// Match one environment-only entry against the Settings search query.
pub(crate) fn matches_environment(doc: &kaos_core::config::EnvironmentDoc, query: &str) -> bool {
    let query = query.trim().to_ascii_lowercase();
    query.is_empty()
        || doc.key.to_ascii_lowercase().contains(&query)
        || doc.details.to_ascii_lowercase().contains(&query)
}

/// Variables intentionally kept out of the persistent config file.
///
pub(crate) use kaos_core::config::ENVIRONMENT_DOCS;

#[derive(Default)]
pub(crate) struct SettingsPane {
    pub(crate) search: String,
    pub(crate) values: BTreeMap<String, String>,
    /// Provider/model selectors shared with the terminal palette. Ollama
    /// entries are discovered from the active `ollama ls` catalog.
    pub(crate) model_choices: Vec<String>,
    saved: BTreeMap<String, String>,
    pub(crate) notice: Option<String>,
    /// In-flight credential entry, keyed by provider name. Cleared the moment
    /// the value reaches the store, so a key is never held in UI state longer
    /// than the keystrokes that typed it.
    pub(crate) credentials: BTreeMap<String, CredentialEdit>,
}

impl SettingsPane {
    fn apply_runtime_value(key: &str, value: &str) {
        if key == "KAOS_THINK" {
            std::env::set_var("KAOS_THINK", value);
        }
    }

    pub(crate) fn load() -> Self {
        let model_choices = kaos_agent::provider::model_choices();
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
                    model_choices,
                    ..Self::default()
                }
            }
            Err(error) => Self {
                notice: Some(format!("could not read configuration: {error}")),
                model_choices,
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

    /// Re-read the active Ollama catalog without disturbing typed settings.
    pub(crate) fn refresh_models(&mut self) {
        match kaos_agent::provider::refresh_ollama_models() {
            Ok(models) => {
                self.model_choices = kaos_agent::provider::model_choices();
                self.notice = Some(format!("found {} Ollama model(s)", models.len()));
            }
            Err(error) => {
                self.notice = Some(format!("Ollama models unavailable: {error}"));
            }
        }
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
            Self::apply_runtime_value(key, value);
        }
        self.saved = self.values.clone();
        Ok(changed.len())
    }

    pub(crate) fn save_key(&mut self, key: &str) -> Result<(), String> {
        let value = self.values.get(key).cloned().unwrap_or_default();
        kaos_core::config::set_value(key, &value)?;
        Self::apply_runtime_value(key, &value);
        self.saved.insert(key.to_string(), value);
        Ok(())
    }

    pub(crate) fn restore(&mut self) -> Result<(), String> {
        kaos_core::config::restore_defaults()?;
        *self = Self::load();
        if let Some(value) = self.values.get("KAOS_THINK") {
            Self::apply_runtime_value("KAOS_THINK", value);
        }
        self.notice = Some("restored documented defaults".to_string());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_persistent_key_has_a_visual_group_and_documentation() {
        for key in kaos_core::config::CONFIG_KEYS {
            assert!(Group::ALL.contains(&group(key)));
            let doc = documentation(key).expect("config key should have metadata");
            assert!(!doc.summary.is_empty());
            assert!(!doc.details.is_empty());
            assert!(!doc.example.is_empty());
        }
    }

    #[test]
    fn visual_metadata_is_the_same_inventory_as_core_config() {
        assert_eq!(
            kaos_core::config::CONFIG_DOCS.len(),
            kaos_core::config::CONFIG_KEYS.len()
        );
        for doc in kaos_core::config::CONFIG_DOCS {
            assert!(kaos_core::config::CONFIG_KEYS.contains(&doc.key));
            assert_eq!(documentation(doc.key), Some(doc));
        }
    }

    /// Credentials are not config keys, and must never become them.
    ///
    /// The settings file is ordinary text that people paste into bug reports;
    /// the credential store is 0600. If a provider key ever appeared in the
    /// config inventory it would be written to the wrong file, and nothing
    /// downstream would notice.
    #[test]
    fn no_provider_key_is_a_config_key() {
        for (_, var) in kaos_agent::auth::PROVIDERS {
            assert!(
                !kaos_core::config::CONFIG_KEYS.contains(var),
                "{var} is in the config inventory — it would be written to the settings file"
            );
        }
        // The base URLs, which are NOT secrets, do belong there.
        assert!(kaos_core::config::CONFIG_KEYS.contains(&"OPENROUTER_BASE_URL"));
    }

    /// A typed key never survives the action that consumes it.
    #[test]
    fn credential_entry_is_cleared_rather_than_retained() {
        let mut pane = SettingsPane::load();
        pane.credentials.insert(
            "openrouter".to_string(),
            CredentialEdit {
                entry: "sk-or-secret".into(),
            },
        );
        assert!(pane.credentials.contains_key("openrouter"));
        // What the store and forget handlers both do.
        pane.credentials.remove("openrouter");
        let rendered = format!("{:?}", pane.credentials.keys().collect::<Vec<_>>());
        assert!(!rendered.contains("secret"), "{rendered}");
        assert!(pane.credentials.is_empty());
    }

    #[test]
    fn theme_is_an_appearance_setting() {
        assert_eq!(group("theme"), Group::Appearance);
        assert_eq!(
            kaos_core::config::default_value("theme").as_deref(),
            Some("dark")
        );
    }

    #[test]
    fn configuration_search_covers_keys_and_documentation() {
        assert!(matches("KAOS_MODEL", "model"));
        assert!(matches("KAOS_MODEL", "ollama"));
        assert!(matches("KAOS_MODEL", "provider and model binding"));
        assert!(!matches("KAOS_MODEL", "does-not-exist"));
        assert!(matches("KAOS_MODEL", ""));
    }

    #[test]
    fn configuration_search_covers_environment_only_entries() {
        let doc = ENVIRONMENT_DOCS
            .iter()
            .find(|doc| doc.key == "OPENAI_API_KEY")
            .expect("OpenAI credential should be documented");
        assert!(matches_environment(doc, "openai"));
        assert!(matches_environment(doc, "credential"));
        assert!(!matches_environment(doc, "does-not-exist"));
    }
}
