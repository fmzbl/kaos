//! Durable prompt boundaries for resumable hosted Rebis runs.
//!
//! Rebis does not expose its recursive interpreter stack as a serializable
//! value.  A host can nevertheless reconstruct that stack without repeating
//! completed model/tool work: replay each completed prompt's exact answer while
//! evaluating from the captured source, then call the model at the first prompt
//! that has no checkpoint.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// Passed by the TUI to every hosted Rebis child.
pub const PATH_ENV: &str = "KAOS_REBIS_CHECKPOINT";

const HEADER: &str = "KAOS_REBIS_PROMPTS_V1";
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Eq, PartialEq)]
struct PromptCheckpoint {
    prompt: String,
    answer: Option<String>,
}

/// Result of looking for one already-completed prompt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Replay {
    /// This is the first unfinished prompt and must be tried normally.
    Miss,
    /// Return this answer without calling the model or repeating its tools.
    Hit(Option<String>),
}

/// A prompt journal shared by all calls in one child.
pub struct PromptJournal {
    path: Option<PathBuf>,
    entries: Mutex<Vec<PromptCheckpoint>>,
}

impl PromptJournal {
    /// Open the journal selected by [`PATH_ENV`]. Without that environment
    /// variable the journal is inert, which preserves ordinary CLI behavior.
    #[must_use]
    pub fn from_env() -> Self {
        let path = std::env::var_os(PATH_ENV).map(PathBuf::from);
        let entries = path
            .as_deref()
            .and_then(|path| load(path).ok())
            .unwrap_or_default();
        Self {
            path,
            entries: Mutex::new(entries),
        }
    }

    /// Replay an exact prompt at a completed sequence position. A structural
    /// divergence invalidates this point and everything after it; earlier
    /// completed calls remain safe to replay.
    #[must_use]
    pub fn replay(&self, index: usize, prompt: &str) -> Replay {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(entry) = entries.get(index) else {
            return Replay::Miss;
        };
        if entry.prompt == prompt {
            return Replay::Hit(entry.answer.clone());
        }
        entries.truncate(index);
        let _ = self.persist(&entries);
        Replay::Miss
    }

    /// Commit a completed prompt before its answer returns to the interpreter.
    /// The write is an atomic replacement, so a disappearing child leaves either
    /// the previous complete boundary or the new one, never a partial entry.
    pub fn record(&self, index: usize, prompt: &str, answer: Option<&str>) -> Result<(), String> {
        let Some(_) = self.path else {
            return Ok(());
        };
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if entries.len() > index {
            entries.truncate(index);
        }
        if entries.len() != index {
            return Err(format!(
                "prompt checkpoint sequence is discontinuous at step {}",
                index + 1
            ));
        }
        entries.push(PromptCheckpoint {
            prompt: prompt.to_string(),
            answer: answer.map(str::to_string),
        });
        self.persist(&entries)
    }

    /// Replay a prompt at a deterministic branch-local position.
    ///
    /// Root calls retain the original linear journal. Concurrent branch calls
    /// use separate sidecar entries keyed by structural scope, so completion
    /// order cannot change checkpoint identity.
    #[must_use]
    pub fn replay_scoped(
        &self,
        scope: &rebis_lang::ExecutionScope,
        index: usize,
        prompt: &str,
    ) -> Replay {
        if scope.is_root() {
            return self.replay(index, prompt);
        }
        let Some(path) = self.scoped_path(scope, index) else {
            return Replay::Miss;
        };
        match load(&path) {
            Ok(entries) if entries.first().is_some_and(|entry| entry.prompt == prompt) => {
                Replay::Hit(entries[0].answer.clone())
            }
            _ => Replay::Miss,
        }
    }

    /// Commit one deterministic branch-local prompt boundary.
    pub fn record_scoped(
        &self,
        scope: &rebis_lang::ExecutionScope,
        index: usize,
        prompt: &str,
        answer: Option<&str>,
    ) -> Result<(), String> {
        if scope.is_root() {
            return self.record(index, prompt, answer);
        }
        let Some(path) = self.scoped_path(scope, index) else {
            return Ok(());
        };
        persist_path(
            &path,
            &[PromptCheckpoint {
                prompt: prompt.to_string(),
                answer: answer.map(str::to_string),
            }],
        )
    }

    fn scoped_path(&self, scope: &rebis_lang::ExecutionScope, index: usize) -> Option<PathBuf> {
        let base = self.path.as_ref()?;
        let directory = PathBuf::from(format!("{}.branches", base.display()));
        Some(directory.join(format!(
            "{}-{index}.checkpoint",
            hex_encode(scope.to_string().as_bytes())
        )))
    }

    fn persist(&self, entries: &[PromptCheckpoint]) -> Result<(), String> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        persist_path(path, entries)
    }
}

fn persist_path(path: &Path, entries: &[PromptCheckpoint]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("could not create prompt checkpoint directory: {error}"))?;
    }
    let mut encoded = String::from(HEADER);
    encoded.push('\n');
    for entry in entries {
        encoded.push_str(&hex_encode(entry.prompt.as_bytes()));
        encoded.push('\t');
        match &entry.answer {
            Some(answer) => {
                encoded.push('S');
                encoded.push_str(&hex_encode(answer.as_bytes()));
            }
            None => encoded.push('N'),
        }
        encoded.push('\n');
    }
    let temporary = PathBuf::from(format!(
        "{}.tmp.{}.{}",
        path.display(),
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&temporary, encoded)
        .map_err(|error| format!("could not write prompt checkpoint: {error}"))?;
    std::fs::rename(&temporary, path)
        .map_err(|error| format!("could not commit prompt checkpoint: {error}"))
}

fn load(path: &Path) -> Result<Vec<PromptCheckpoint>, String> {
    let encoded = match std::fs::read_to_string(path) {
        Ok(encoded) => encoded,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("could not read prompt checkpoint: {error}")),
    };
    let mut lines = encoded.lines();
    if lines.next() != Some(HEADER) {
        return Err("unrecognized prompt checkpoint format".to_string());
    }
    lines
        .enumerate()
        .map(|(index, line)| {
            let (prompt, answer) = line
                .split_once('\t')
                .ok_or_else(|| format!("invalid prompt checkpoint entry {}", index + 1))?;
            let prompt = String::from_utf8(hex_decode(prompt)?)
                .map_err(|_| format!("prompt checkpoint entry {} is not UTF-8", index + 1))?;
            let answer = match answer.as_bytes().first() {
                Some(b'N') if answer.len() == 1 => None,
                Some(b'S') => {
                    Some(String::from_utf8(hex_decode(&answer[1..])?).map_err(|_| {
                        format!("prompt checkpoint answer {} is not UTF-8", index + 1)
                    })?)
                }
                _ => return Err(format!("invalid prompt checkpoint answer {}", index + 1)),
            };
            Ok(PromptCheckpoint { prompt, answer })
        })
        .collect()
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

fn hex_decode(encoded: &str) -> Result<Vec<u8>, String> {
    if !encoded.len().is_multiple_of(2) {
        return Err("odd-length data in prompt checkpoint".to_string());
    }
    encoded
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let digits = std::str::from_utf8(pair).expect("hex digits are ASCII");
            u8::from_str_radix(digits, 16)
                .map_err(|_| "non-hex data in prompt checkpoint".to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "kaos-rebis-checkpoint-{label}-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("thread")
        ))
    }

    #[test]
    fn completed_prompts_survive_a_restarted_journal() {
        let path = test_path("restart");
        let journal = PromptJournal {
            path: Some(path.clone()),
            entries: Mutex::new(Vec::new()),
        };
        journal
            .record(0, "first\nprompt", Some("first\nanswer"))
            .unwrap();
        journal.record(1, "second", None).unwrap();

        let reopened = PromptJournal {
            entries: Mutex::new(load(&path).unwrap()),
            path: Some(path.clone()),
        };
        assert_eq!(
            reopened.replay(0, "first\nprompt"),
            Replay::Hit(Some("first\nanswer".to_string()))
        );
        assert_eq!(reopened.replay(1, "second"), Replay::Hit(None));
        assert_eq!(reopened.replay(2, "failed prompt"), Replay::Miss);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn changed_execution_truncates_only_the_divergent_tail() {
        let path = test_path("diverge");
        let journal = PromptJournal {
            path: Some(path.clone()),
            entries: Mutex::new(Vec::new()),
        };
        journal.record(0, "stable", Some("kept")).unwrap();
        journal.record(1, "old path", Some("discarded")).unwrap();

        assert_eq!(
            journal.replay(0, "stable"),
            Replay::Hit(Some("kept".into()))
        );
        assert_eq!(journal.replay(1, "new path"), Replay::Miss);
        journal.record(1, "new path", Some("replacement")).unwrap();

        let entries = load(&path).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].prompt, "new path");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn parallel_scopes_checkpoint_independently_of_completion_order() {
        let path = test_path("scoped");
        let journal = PromptJournal {
            path: Some(path.clone()),
            entries: Mutex::new(Vec::new()),
        };
        let root = rebis_lang::ExecutionScope::root();
        let scopes = [root.branch(0, 0), root.branch(0, 1)];
        std::thread::scope(|threads| {
            for (index, scope) in scopes.iter().enumerate().rev() {
                let journal = &journal;
                threads.spawn(move || {
                    journal
                        .record_scoped(
                            scope,
                            0,
                            &format!("prompt-{index}"),
                            Some(&format!("answer-{index}")),
                        )
                        .unwrap();
                });
            }
        });

        for (index, scope) in scopes.iter().enumerate() {
            assert_eq!(
                journal.replay_scoped(scope, 0, &format!("prompt-{index}")),
                Replay::Hit(Some(format!("answer-{index}")))
            );
        }
        let _ = std::fs::remove_dir_all(format!("{}.branches", path.display()));
    }
}
