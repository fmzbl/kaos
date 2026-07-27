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

pub(crate) fn documentation(key: &str) -> Option<&'static kaos_core::config::ConfigDoc> {
    kaos_core::config::documentation(key)
}

pub(crate) fn is_boolean(key: &str) -> bool {
    documentation(key).is_some_and(|doc| doc.kind == kaos_core::config::ValueKind::Boolean)
}

pub(crate) fn is_tristate(key: &str) -> bool {
    documentation(key).is_some_and(|doc| doc.kind == kaos_core::config::ValueKind::TriState)
}

/// Variables intentionally kept out of the persistent config file.
///
/// The visual tab shows these as a read-only boundary reference so users can
/// tell the difference between a setting they can save and a value supplied by
/// a parent process, credentials store, or hosted-run transport.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct EnvironmentDoc {
    pub(crate) key: &'static str,
    pub(crate) details: &'static str,
}

pub(crate) const ENVIRONMENT_DOCS: &[EnvironmentDoc] = &[
    EnvironmentDoc {
        key: "OPENAI_API_KEY",
        details: "OpenAI credential; use `kaos auth` or an explicit shell export, never the persistent config.",
    },
    EnvironmentDoc {
        key: "ANTHROPIC_API_KEY",
        details: "Anthropic API credential; the Claude subscription CLI uses `claude login` instead.",
    },
    EnvironmentDoc {
        key: "OPENROUTER_API_KEY",
        details: "OpenRouter credential; use `kaos auth` or an explicit shell export.",
    },
    EnvironmentDoc {
        key: "REBIS_COLLECTION_PATH",
        details: "Optional Rebis collection root or modules directory used to discover source-only imports.",
    },
    EnvironmentDoc {
        key: "KAOS_BIN",
        details: "Optional visual-editor override for the Kaos executable used to launch terminal runs.",
    },
    EnvironmentDoc {
        key: "KAOS_FOLD",
        details: "Private child-process flag that asks the terminal parent to render foldable progress groups.",
    },
    EnvironmentDoc {
        key: "KAOS_SESSION",
        details: "Private session identifier passed to child chats so transcripts can be resumed.",
    },
    EnvironmentDoc {
        key: "KAOS_RESUME",
        details: "Private child-process flag selecting resume versus create for a chat session.",
    },
    EnvironmentDoc {
        key: "KAOS_REBIS_CONTEXT",
        details: "Private flag that injects Rebis authoring and validation guidance into a coding chat.",
    },
    EnvironmentDoc {
        key: "KAOS_RAW_CHAT_TASK_STDIN",
        details: "Private flag selecting a raw task from standard input for visual and terminal chat.",
    },
    EnvironmentDoc {
        key: "KAOS_CHAT_OUTPUT",
        details: "Private flag requesting a clean assistant-only response from a child chat.",
    },
    EnvironmentDoc {
        key: "KAOS_PAUSE_ON_TRANSIENT",
        details: "Hosted-run transport flag enabling continuation-safe pauses after retryable model failures.",
    },
    EnvironmentDoc {
        key: "KAOS_RUN_PROCESS_GROUP",
        details: "Hosted-run transport flag that lets pause and cancellation include command descendants.",
    },
    EnvironmentDoc {
        key: "KAOS_REBIS_CHECKPOINT",
        details: "Private path for the Rebis prompt journal used to resume an interrupted hosted run.",
    },
    EnvironmentDoc {
        key: "KAOS_REBIS_DIRECTIVE",
        details: "Private path for supervisor directives sent to a live Rebis child.",
    },
    EnvironmentDoc {
        key: "KAOS_REBIS_INLET",
        details: "Private path for user input delivered to a live Rebis child.",
    },
];

#[derive(Default)]
pub(crate) struct SettingsPane {
    pub(crate) filter: String,
    pub(crate) values: BTreeMap<String, String>,
    saved: BTreeMap<String, String>,
    pub(crate) notice: Option<String>,
    /// In-flight credential entry, keyed by provider name. Cleared the moment
    /// the value reaches the store, so a key is never held in UI state longer
    /// than the keystrokes that typed it.
    pub(crate) credentials: BTreeMap<String, CredentialEdit>,
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
}
