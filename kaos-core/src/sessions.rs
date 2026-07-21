//! Durable chat sessions: the transcript you can leave and come back to.
//!
//! A session is the *conversation* — an ordered list of turns — not the
//! rendered screen. Folds, colours and spinners are presentation and are
//! deliberately not persisted; replaying a session restores what was said.
//!
//! The on-disk format is a small text record, so this module stays std-only
//! and works in the dependency-free core:
//!
//! ```text
//! id: 20260720-183045-0
//! model: claude:sonnet
//! cwd: /home/me/project
//! created: 1784570000
//! updated: 1784570450
//!
//! user\tInspect the parser
//! model\tThe parser has three stages…
//! ```
//!
//! Header lines are `key: value`; a blank line ends them. Each turn is one
//! line: a role, a tab, then the text with `\`, tab and newline escaped, so a
//! turn containing blank lines or its own `key: value` text can never be
//! mistaken for structure.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Who produced a turn.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    User,
    Model,
}

impl Role {
    fn tag(self) -> &'static str {
        match self {
            Role::User => "user",
            Role::Model => "model",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "user" => Some(Role::User),
            "model" => Some(Role::Model),
            _ => None,
        }
    }
}

/// One thing said.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Turn {
    pub role: Role,
    pub text: String,
}

/// A conversation, with enough context to resume it where it happened.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Session {
    pub id: String,
    pub model: String,
    pub cwd: String,
    pub created: u64,
    pub updated: u64,
    pub turns: Vec<Turn>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Session {
    /// Start a session. The id is time-ordered so listing newest-first is a
    /// reverse sort on the name alone, with no need to open every file.
    pub fn new(model: impl Into<String>, cwd: impl Into<String>) -> Self {
        let t = now();
        Self {
            id: format!("{t}-{}", std::process::id()),
            model: model.into(),
            cwd: cwd.into(),
            created: t,
            updated: t,
            turns: Vec::new(),
        }
    }

    pub fn push(&mut self, role: Role, text: impl Into<String>) {
        let text = text.into();
        if text.trim().is_empty() {
            return;
        }
        self.turns.push(Turn { role, text });
        self.updated = now();
    }

    pub fn is_empty(&self) -> bool {
        self.turns.is_empty()
    }

    /// Whether this is a conversation worth keeping.
    ///
    /// A session with only model output — a command's stream flushed with
    /// nothing actually asked — is not a conversation, and saving it litters
    /// the resume list with untitled entries.
    pub fn is_conversation(&self) -> bool {
        self.turns.iter().any(|t| t.role == Role::User)
    }

    /// A one-line name, taken from the opening message.
    pub fn title(&self) -> String {
        let first = self
            .turns
            .iter()
            .find(|t| t.role == Role::User)
            .map(|t| t.text.as_str())
            .unwrap_or("(empty)");
        let line = first.lines().next().unwrap_or("").trim();
        let mut out: String = line.chars().take(60).collect();
        if line.chars().count() > 60 {
            out.push('…');
        }
        if out.is_empty() {
            out.push_str("(empty)");
        }
        out
    }

    pub fn encode(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!("id: {}\n", self.id));
        s.push_str(&format!("model: {}\n", self.model));
        s.push_str(&format!("cwd: {}\n", self.cwd));
        s.push_str(&format!("created: {}\n", self.created));
        s.push_str(&format!("updated: {}\n", self.updated));
        s.push('\n');
        for t in &self.turns {
            s.push_str(t.role.tag());
            s.push('\t');
            s.push_str(&escape(&t.text));
            s.push('\n');
        }
        s
    }

    pub fn decode(text: &str) -> Result<Self, String> {
        let mut head: BTreeMap<&str, &str> = BTreeMap::new();
        let mut lines = text.lines();
        for line in lines.by_ref() {
            if line.is_empty() {
                break;
            }
            let (k, v) = line
                .split_once(HEADER_SEP)
                .ok_or_else(|| format!("bad header line: {line}"))?;
            head.insert(k.trim(), v.trim());
        }
        let mut turns = Vec::new();
        for line in lines {
            if line.trim().is_empty() {
                continue;
            }
            let (role, body) = line
                .split_once('\t')
                .ok_or_else(|| format!("bad turn line: {line}"))?;
            let role = Role::parse(role).ok_or_else(|| format!("unknown role: {role}"))?;
            turns.push(Turn {
                role,
                text: unescape(body),
            });
        }
        Ok(Session {
            id: head.get("id").unwrap_or(&"").to_string(),
            model: head.get("model").unwrap_or(&"").to_string(),
            cwd: head.get("cwd").unwrap_or(&"").to_string(),
            created: head
                .get("created")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            updated: head
                .get("updated")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            turns,
        })
    }
}

const HEADER_SEP: &str = ": ";

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// A session on disk, without reading its turns.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Summary {
    pub id: String,
    pub title: String,
    pub updated: u64,
    pub turns: usize,
}

impl fmt::Display for Summary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}  {} turns  {}", self.id, self.turns, self.title)
    }
}

/// Where sessions live.
#[derive(Clone, Debug)]
pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// `~/.kaos/sessions`, alongside the sigil library; falls back to the
    /// working directory when there is no home.
    pub fn default_store() -> Self {
        let base = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        Self::new(base.join(".kaos").join("sessions"))
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn save(&self, session: &Session) -> Result<(), String> {
        if !session.is_conversation() {
            return Ok(()); // nothing was actually said; leave no file behind
        }
        fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        let path = self.path(&session.id);
        // Write then rename, so an interrupted save cannot truncate the
        // previous session.
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, session.encode()).map_err(|e| e.to_string())?;
        fs::rename(&tmp, &path).map_err(|e| e.to_string())
    }

    pub fn load(&self, id: &str) -> Result<Session, String> {
        let text = fs::read_to_string(self.path(id)).map_err(|e| e.to_string())?;
        Session::decode(&text)
    }

    pub fn delete(&self, id: &str) -> Result<(), String> {
        fs::remove_file(self.path(id)).map_err(|e| e.to_string())
    }

    /// Every stored session, newest first.
    pub fn list(&self) -> Vec<Summary> {
        let mut out = Vec::new();
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return out;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("session") {
                continue;
            }
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            let Ok(s) = Session::decode(&text) else {
                continue;
            };
            out.push(Summary {
                id: s.id.clone(),
                title: s.title(),
                updated: s.updated,
                turns: s.turns.len(),
            });
        }
        out.sort_by(|a, b| b.updated.cmp(&a.updated).then(b.id.cmp(&a.id)));
        out
    }

    /// The most recently updated session, if any.
    pub fn latest(&self) -> Option<Summary> {
        self.list().into_iter().next()
    }

    /// Resolve what a user typed after `/chat resume`: nothing means the latest,
    /// a number means that position in the list, otherwise an id prefix.
    pub fn resolve(&self, what: &str) -> Option<Summary> {
        let what = what.trim();
        let list = self.list();
        if what.is_empty() {
            return list.into_iter().next();
        }
        if let Ok(n) = what.parse::<usize>() {
            if n >= 1 {
                return list.into_iter().nth(n - 1);
            }
        }
        list.into_iter().find(|s| s.id.starts_with(what))
    }

    fn path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.session"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(name: &str) -> Store {
        let dir =
            std::env::temp_dir().join(format!("kaos-sessions-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        Store::new(dir)
    }

    fn sample() -> Session {
        let mut s = Session::new("claude:sonnet", "/tmp/project");
        s.push(Role::User, "Inspect the parser");
        s.push(Role::Model, "It has three stages.");
        s
    }

    #[test]
    fn encode_decode_round_trips() {
        let s = sample();
        let back = Session::decode(&s.encode()).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn text_with_structure_characters_survives() {
        // Newlines, tabs, backslashes, and text that looks like a header or a
        // turn line must all come back byte-identical.
        let mut s = Session::new("m", "/c");
        s.push(Role::User, "line one\nline two\n\nid: not-a-header");
        s.push(Role::Model, "col\tcol\\end\r\nuser\tnot-a-turn");
        let back = Session::decode(&s.encode()).unwrap();
        assert_eq!(s.turns, back.turns);
    }

    #[test]
    fn blank_turns_are_not_recorded() {
        let mut s = Session::new("m", "/c");
        s.push(Role::User, "   \n  ");
        assert!(s.is_empty());
    }

    #[test]
    fn title_comes_from_the_first_user_line() {
        let mut s = Session::new("m", "/c");
        s.push(Role::Model, "a greeting nobody asked for");
        s.push(Role::User, "fix the cancellation bug\nmore detail");
        assert_eq!(s.title(), "fix the cancellation bug");
    }

    #[test]
    fn long_titles_are_clipped() {
        let mut s = Session::new("m", "/c");
        s.push(Role::User, "x".repeat(200));
        let t = s.title();
        assert_eq!(t.chars().count(), 61); // 60 + ellipsis
        assert!(t.ends_with('…'));
    }

    #[test]
    fn empty_session_has_a_title() {
        assert_eq!(Session::new("m", "/c").title(), "(empty)");
    }

    #[test]
    fn save_load_delete() {
        let store = temp_store("crud");
        let s = sample();
        store.save(&s).unwrap();
        assert_eq!(store.load(&s.id).unwrap(), s);
        store.delete(&s.id).unwrap();
        assert!(store.load(&s.id).is_err());
        let _ = fs::remove_dir_all(store.dir());
    }

    #[test]
    fn a_session_with_only_model_output_is_not_saved() {
        // A command's stream flushed with nothing asked is not a conversation.
        let store = temp_store("modelonly");
        let mut s = Session::new("m", "/c");
        s.push(Role::Model, "some streamed output");
        assert!(!s.is_conversation());
        store.save(&s).unwrap();
        assert!(store.list().is_empty());
        let _ = fs::remove_dir_all(store.dir());
    }

    #[test]
    fn an_empty_session_leaves_no_file() {
        let store = temp_store("empty");
        let s = Session::new("m", "/c");
        store.save(&s).unwrap();
        assert!(store.list().is_empty());
        let _ = fs::remove_dir_all(store.dir());
    }

    #[test]
    fn list_is_newest_first() {
        let store = temp_store("order");
        for (i, text) in ["oldest", "middle", "newest"].iter().enumerate() {
            let mut s = Session::new("m", "/c");
            s.id = format!("id-{i}");
            s.push(Role::User, *text);
            s.updated = 1000 + i as u64;
            store.save(&s).unwrap();
        }
        let titles: Vec<String> = store.list().into_iter().map(|s| s.title).collect();
        assert_eq!(titles, vec!["newest", "middle", "oldest"]);
        assert_eq!(store.latest().unwrap().title, "newest");
        let _ = fs::remove_dir_all(store.dir());
    }

    #[test]
    fn resolve_accepts_nothing_a_number_or_an_id_prefix() {
        let store = temp_store("resolve");
        for (i, text) in ["a", "b"].iter().enumerate() {
            let mut s = Session::new("m", "/c");
            s.id = format!("session-{i}");
            s.push(Role::User, *text);
            s.updated = 100 + i as u64;
            store.save(&s).unwrap();
        }
        assert_eq!(store.resolve("").unwrap().title, "b"); // latest
        assert_eq!(store.resolve("1").unwrap().title, "b"); // first listed
        assert_eq!(store.resolve("2").unwrap().title, "a");
        assert_eq!(store.resolve("session-0").unwrap().title, "a");
        assert!(store.resolve("nope").is_none());
        assert!(store.resolve("99").is_none());
        let _ = fs::remove_dir_all(store.dir());
    }

    #[test]
    fn listing_a_missing_directory_is_empty_not_an_error() {
        assert!(Store::new("/nonexistent/kaos/sessions").list().is_empty());
    }

    #[test]
    fn unreadable_entries_are_skipped_rather_than_failing_the_list() {
        let store = temp_store("junk");
        fs::create_dir_all(store.dir()).unwrap();
        fs::write(store.dir().join("broken.session"), "not a session\x00").unwrap();
        fs::write(store.dir().join("ignored.txt"), "irrelevant").unwrap();
        let good = sample();
        store.save(&good).unwrap();
        let list = store.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, good.id);
        let _ = fs::remove_dir_all(store.dir());
    }
}
