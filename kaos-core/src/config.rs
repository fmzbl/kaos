//! Persistent, non-secret Kaos configuration.
//!
//! On the first run Kaos creates `~/.config/kaos/config` (honouring
//! `XDG_CONFIG_HOME`) with every supported setting and its effective default.
//! Existing files are never replaced or automatically rewritten. Values seed
//! the process environment without overriding an explicit shell export.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Every persistent, user-facing setting understood by Kaos. Runtime transport
/// variables (`KAOS_SESSION`, `KAOS_RESUME`, `KAOS_FOLD`) and provider secrets
/// intentionally live elsewhere.
pub const CONFIG_KEYS: &[&str] = &[
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

/// The complete file written when no config exists. Empty values are meaningful
/// defaults: they leave an optional override disabled. `${KAOS_K}` is expanded
/// while loading, so changing the quorum also changes the default myth.
pub const DEFAULT_CONFIG: &str = r#"# Kaos configuration
# Shell environment variables override this file. Provider API keys belong in
# ~/.config/kaos/credentials and are managed with `kaos auth`.

# Rebis editor
vim_mode = false

# Mind and provider
KAOS_MODEL = sim
# One-shot completions such as /cast.
KAOS_TIMEOUT_S = 120
# One model turn inside a tool-using chat/code agent.
KAOS_CHAT_TIMEOUT_S = 600
KAOS_MAX_TOKENS = 8192
KAOS_NUM_PREDICT =
KAOS_FABLE_FALLBACK_MODEL =
KAOS_PROVIDER_SORT =
KAOS_PROVIDER_ONLY =
OPENAI_BASE_URL = https://api.openai.com
OPENROUTER_BASE_URL = https://openrouter.ai/api
OLLAMA_HOST = http://127.0.0.1:11434

# Agent working
KAOS_MAX_STEPS = 14
KAOS_HAND = 0
KAOS_PROTECT =
KAOS_NO_FORGE = 0
KAOS_NO_DREAM = 0
KAOS_NO_PARADIGM = 0
# Empty means ask on the first coding task; 1 grants shell, 0 allows edits only.
KAOS_CLAUDE_YOLO =

# Conclave and myth
KAOS_K = 5
KAOS_MYTH = (gather vote (spread ${KAOS_K} fire))
KAOS_AGENTIC = 0
KAOS_ARENA = .
KAOS_MAX_CONCURRENCY = 3
KAOS_BASH_TIMEOUT_S = 600
KAOS_GATE_TIMEOUT_S = 300
KAOS_QUIET = 0

# Twin ladders
KAOS_UNIT = 700
KAOS_BASE = 500
KAOS_RUNGS = 5

# Rebis runtime limits; 0 disables the corresponding limit.
KAOS_REBIS_MAX_EXPANSIONS = 256
KAOS_REBIS_MAX_MODULES = 64
KAOS_REBIS_MAX_CALLS = 1024
KAOS_REBIS_MAX_CONCURRENCY = 4
# One model turn may include planning a sequence of file and command actions.
KAOS_REBIS_TIMEOUT_S = 600

# Diagnostics
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
