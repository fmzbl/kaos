//! Persistent provider auth. API keys live in a 0600 credentials file so you set
//! them once (`kaos auth <provider> <key>`) instead of exporting env vars every
//! session. [`load`] seeds the process env from that file at startup — but never
//! over an env var you set explicitly, so an export still wins. The `claude` CLI
//! is not here: it authenticates through your claude.ai login (`claude login`),
//! not a key.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::PathBuf;

/// The providers kaos can hold a key for, in display order: (name, env var).
pub const PROVIDERS: &[(&str, &str)] = &[
    ("openrouter", "OPENROUTER_API_KEY"),
    ("openai", "OPENAI_API_KEY"),
    ("anthropic", "ANTHROPIC_API_KEY"),
];

/// Map a friendly provider name (or an alias) to the env var the backends read.
pub fn var_for(provider: &str) -> Option<&'static str> {
    match provider.to_lowercase().as_str() {
        "openrouter" | "or" => Some("OPENROUTER_API_KEY"),
        "openai" | "oai" | "gpt" => Some("OPENAI_API_KEY"),
        "anthropic" | "claude-api" => Some("ANTHROPIC_API_KEY"),
        _ => None,
    }
}

/// `~/.config/kaos/credentials` (honours `XDG_CONFIG_HOME`).
fn cred_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("kaos").join("credentials"))
}

/// Parse the credentials file into KEY→value (blank/`#` lines skipped).
fn read_all() -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    if let Some(p) = cred_path() {
        if let Ok(s) = fs::read_to_string(p) {
            for line in s.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some((k, v)) = line.split_once('=') {
                    map.insert(k.trim().to_string(), v.trim().to_string());
                }
            }
        }
    }
    map
}

fn write_all(map: &BTreeMap<String, String>) -> io::Result<PathBuf> {
    let path = cred_path()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no HOME or XDG_CONFIG_HOME"))?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let body: String = map.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
    fs::write(&path, body)?;
    // Keys are secrets: owner-only.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    Ok(path)
}

/// Seed the process env from stored credentials at startup — but only for vars not
/// already set, so an explicit `export` overrides the stored default.
pub fn load() {
    for (k, v) in read_all() {
        if !v.is_empty() && std::env::var_os(&k).is_none() {
            std::env::set_var(&k, v);
        }
    }
}

/// Store `key` for `provider`: persist to the 0600 file AND set it live in this
/// process. Returns the env var written and the file path.
pub fn store(provider: &str, key: &str) -> io::Result<(&'static str, PathBuf)> {
    let var = var_for(provider).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unknown provider '{provider}' — try openrouter | openai | anthropic"),
        )
    })?;
    let mut map = read_all();
    map.insert(var.to_string(), key.to_string());
    let path = write_all(&map)?;
    std::env::set_var(var, key);
    Ok((var, path))
}

/// Forget a stored key for `provider` (and clear it from this process).
pub fn forget(provider: &str) -> io::Result<&'static str> {
    let var = var_for(provider).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unknown provider '{provider}' — try openrouter | openai | anthropic"),
        )
    })?;
    let mut map = read_all();
    map.remove(var);
    write_all(&map)?;
    std::env::remove_var(var);
    Ok(var)
}

/// One status row per provider: (name, env var, whether a non-empty key is live,
/// whether that key is stored in the credentials file).
pub fn status() -> Vec<(&'static str, &'static str, bool, bool)> {
    let stored = read_all();
    PROVIDERS
        .iter()
        .map(|(name, var)| {
            let live = std::env::var(var).ok().filter(|v| !v.is_empty()).is_some();
            let saved = stored.get(*var).map(|v| !v.is_empty()).unwrap_or(false);
            (*name, *var, live, saved)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_load_forget_roundtrip() {
        // isolate the config dir so we don't touch a real one
        let dir = std::env::temp_dir().join(format!(
            "kaos-auth-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        std::env::remove_var("OPENROUTER_API_KEY");

        let (var, path) = store("openrouter", "sk-test-123").unwrap();
        assert_eq!(var, "OPENROUTER_API_KEY");
        assert!(path.exists());
        assert_eq!(std::env::var("OPENROUTER_API_KEY").unwrap(), "sk-test-123");

        // a fresh process (env cleared) would repopulate from the file via load()
        std::env::remove_var("OPENROUTER_API_KEY");
        load();
        assert_eq!(std::env::var("OPENROUTER_API_KEY").unwrap(), "sk-test-123");

        // an explicit env var must win over the stored one
        std::env::set_var("OPENROUTER_API_KEY", "sk-explicit");
        load();
        assert_eq!(std::env::var("OPENROUTER_API_KEY").unwrap(), "sk-explicit");

        forget("openrouter").unwrap();
        assert!(std::env::var_os("OPENROUTER_API_KEY").is_none());

        assert!(var_for("nope").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
