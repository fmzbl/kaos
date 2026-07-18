//! The agent — the Conclave doing *real* work, gated by a *real* verifier.
//!
//! Everything before this was prompts and simulation. Here the Pact actually edits
//! files and runs tests. The design is the one thing the benchmark proved worth
//! keeping — **the Conclave (self-consistency)** — but applied to verified work
//! instead of answers:
//!
//! 1. Convene `k` adepts on a coding task.
//! 2. Each adept works in its **own isolated copy** of the target (a banished,
//!    private context — one adept's mess never touches another's).
//! 3. Each proposes a [`Patch`]; we apply it and run the **verifier** (the project's
//!    tests). This is Ma'at: the heart is weighed against the feather, and *nothing
//!    ships unverified*.
//! 4. Among the patches that **pass**, the conclave ships the consensus one (a
//!    majority vote over verified diffs). If none pass, nothing ships — honestly.
//!
//! The edge is structural and real: P(at least one of k verified) ≥ P(one verified),
//! and shipping only verified work means the gate never passes a false positive.
//! The model backend is behind the [`AdeptAgent`] seam, so the same loop runs with a
//! scripted adept (deterministic, for tests), a local ollama model, or claude.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::rng::Rng;

// ───────────────────────────── patches ─────────────────────────────

/// A single edit to the workspace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Edit {
    /// Overwrite (or create) a file with exact contents.
    Write { path: String, contents: String },
    /// Replace the first occurrence of `find` with `replace` in an existing file.
    Replace {
        path: String,
        find: String,
        replace: String,
    },
}

/// A proposed change: an ordered set of edits. The unit an adept ships.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Patch {
    pub edits: Vec<Edit>,
}

impl Patch {
    pub fn new() -> Patch {
        Patch { edits: Vec::new() }
    }

    pub fn is_empty(&self) -> bool {
        self.edits.is_empty()
    }

    /// A canonical key for voting: two patches with the same effect share a key.
    pub fn key(&self) -> String {
        let mut parts: Vec<String> = self
            .edits
            .iter()
            .map(|e| match e {
                Edit::Write { path, contents } => format!("W|{path}|{contents}"),
                Edit::Replace {
                    path,
                    find,
                    replace,
                } => format!("R|{path}|{find}=>{replace}"),
            })
            .collect();
        parts.sort();
        parts.join("\n")
    }
}

// ─────────────────────────── workspace ─────────────────────────────

static WS_COUNTER: AtomicU64 = AtomicU64::new(0);

/// An isolated copy of a target directory in which one adept works. Removed on drop
/// — a context, banished when the working ends.
pub struct Workspace {
    pub root: PathBuf,
}

impl Workspace {
    /// Copy `src` into a fresh temp directory. `target/` and `.git/` are skipped so
    /// we never duplicate build artifacts or history.
    pub fn isolate(src: &Path) -> io::Result<Workspace> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let n = WS_COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("kaos-ws-{}-{nanos}-{n}", std::process::id()));
        fs::create_dir_all(&root)?;
        copy_tree(src, &root)?;
        Ok(Workspace { root })
    }

    /// Read the named files (relative paths) into a map; missing files are skipped.
    pub fn read_files(&self, rels: &[&str]) -> BTreeMap<String, String> {
        let mut map = BTreeMap::new();
        for rel in rels {
            if let Ok(s) = fs::read_to_string(self.root.join(rel)) {
                map.insert((*rel).to_string(), s);
            }
        }
        map
    }

    /// Apply a patch. A `Replace` whose `find` is absent is reported as an error so
    /// a hallucinated edit cannot silently "succeed".
    pub fn apply(&self, patch: &Patch) -> io::Result<()> {
        for edit in &patch.edits {
            match edit {
                Edit::Write { path, contents } => {
                    let p = self.root.join(path);
                    if let Some(parent) = p.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    fs::write(p, contents)?;
                }
                Edit::Replace {
                    path,
                    find,
                    replace,
                } => {
                    let p = self.root.join(path);
                    let cur = fs::read_to_string(&p)?;
                    if let Some(pos) = cur.find(find) {
                        let mut next = String::with_capacity(cur.len());
                        next.push_str(&cur[..pos]);
                        next.push_str(replace);
                        next.push_str(&cur[pos + find.len()..]);
                        fs::write(p, next)?;
                    } else {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("replace target not found in {path}"),
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    /// The set of files in this workspace whose contents differ from `original`,
    /// as rel-path → `Some(new contents)` for changed/created files and `None` for
    /// files the adept DELETED (present in `original`, gone from the workspace).
    /// `target/` and `.git/` are skipped; unreadable/binary files are ignored (on
    /// both sides — a binary can be neither diffed nor reported deleted). This is
    /// the *whole diff an adept produced* after a full agent session ran in the
    /// isolated copy — the unit the conclave votes on and ships back. Deletions
    /// must ride along, or the shipped tree differs from the verified one.
    pub fn changed_files(&self, original: &Path) -> io::Result<BTreeMap<String, Option<String>>> {
        let now = collect_files(&self.root)?;
        let before = collect_files(original)?;
        let mut changed = BTreeMap::new();
        for (rel, prior) in &before {
            match now.get(rel) {
                Some(cur) if cur == prior => {}
                Some(cur) => {
                    changed.insert(rel.clone(), Some(cur.clone()));
                }
                None => {
                    changed.insert(rel.clone(), None);
                }
            }
        }
        for (rel, cur) in now {
            if !before.contains_key(&rel) {
                changed.insert(rel, Some(cur));
            }
        }
        Ok(changed)
    }

    /// Run the verifier (a shell command) in the workspace. Success == exit 0.
    /// This is the Weighing — the project's own tests decide truth. The wait is
    /// bounded (300s): a verifier that hangs weighs false instead of hanging the
    /// conclave forever.
    pub fn verify(&self, cmd: &str) -> (bool, String) {
        self.verify_within(cmd, 300)
    }

    /// [`Workspace::verify`] with an explicit timeout. Pipes are drained on reader
    /// threads WHILE waiting (a verbose test suite would otherwise fill the OS pipe
    /// buffer and deadlock), and the child is killed on timeout — the same
    /// discipline as the conductor's `run_bash`. Public so a caller with a slow gate
    /// (a full build, a benchmark) can widen the wait past [`Workspace::verify`]'s
    /// default (`KAOS_GATE_TIMEOUT_S` drives this in the agentic myth).
    pub fn verify_within(&self, cmd: &str, timeout_s: u64) -> (bool, String) {
        let mut child = match Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .current_dir(&self.root)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => return (false, format!("verifier failed to launch: {e}")),
        };

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

        let start = std::time::Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let mut log = out_reader.join().unwrap_or_default();
                    log.push_str(&err_reader.join().unwrap_or_default());
                    return (status.success(), log);
                }
                Ok(None) => {
                    if start.elapsed() > std::time::Duration::from_secs(timeout_s) {
                        let _ = child.kill();
                        let _ = child.wait();
                        // Do NOT join the drain threads here: a grandchild (e.g. the
                        // `sleep` under `sh -c`) can outlive the killed shell and keep
                        // the pipe open, so joining would block until it exits — the
                        // exact wall-time stall the timeout exists to prevent. Detach
                        // them; they end on their own when the pipe finally closes.
                        drop(out_reader);
                        drop(err_reader);
                        return (false, format!("gate timed out after {timeout_s}s"));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(e) => return (false, format!("verifier wait failed: {e}")),
            }
        }
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Collect every readable text file under `root` as rel-path → contents, skipping
/// `target/` and `.git/` and anything that isn't valid UTF-8 (binaries). Rel paths
/// use `/` and are stable across platforms enough for our diffing.
fn collect_files(root: &Path) -> io::Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    collect_into(root, root, &mut out)?;
    Ok(out)
}

fn collect_into(base: &Path, dir: &Path, out: &mut BTreeMap<String, String>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == "target" || name == ".git" {
            continue;
        }
        let path = entry.path();
        let ft = entry.file_type()?;
        if ft.is_dir() {
            collect_into(base, &path, out)?;
        } else if ft.is_file() {
            if let Ok(contents) = fs::read_to_string(&path) {
                if let Ok(rel) = path.strip_prefix(base) {
                    out.insert(rel.to_string_lossy().replace('\\', "/"), contents);
                }
            }
        }
    }
    Ok(())
}

/// Write a change-set (rel-path → `Some(contents)` to write, `None` to delete) back
/// into `target` (creating parent dirs). This is how a verified conclave ships the
/// winning adept's diff into the real project — deletions included, so the shipped
/// tree matches the tree the gate weighed.
pub fn write_files_into(target: &Path, files: &BTreeMap<String, Option<String>>) -> io::Result<()> {
    for (rel, contents) in files {
        let p = target.join(rel);
        match contents {
            Some(c) => {
                if let Some(parent) = p.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(p, c)?;
            }
            None => match fs::remove_file(&p) {
                Ok(()) => {}
                // Already gone is the state we wanted; anything else is a real error.
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            },
        }
    }
    Ok(())
}

/// A canonical signature of a change-set for consensus voting: two adepts that
/// produced byte-identical diffs (writes AND deletions) share a key, so the conclave
/// can pick the *modal* verified fix (self-consistency), not just any passing one.
pub fn changeset_key(files: &BTreeMap<String, Option<String>>) -> String {
    // BTreeMap already iterates in sorted key order, so this is deterministic.
    let mut s = String::new();
    for (rel, contents) in files {
        s.push_str(rel);
        s.push('\u{1f}');
        // Tag the kind so a deletion can never collide with any written contents.
        match contents {
            Some(c) => {
                s.push('W');
                s.push_str(c);
            }
            None => s.push('D'),
        }
        s.push('\u{1e}');
    }
    s
}

/// Recursively copy a directory tree, skipping `target/` and `.git/`.
fn copy_tree(src: &Path, dst: &Path) -> io::Result<()> {
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == "target" || name == ".git" {
            continue;
        }
        let from = entry.path();
        let to = dst.join(&name);
        let ft = entry.file_type()?;
        if ft.is_dir() {
            fs::create_dir_all(&to)?;
            copy_tree(&from, &to)?;
        } else if ft.is_file() {
            fs::copy(&from, &to)?;
        }
        // symlinks are intentionally skipped
    }
    Ok(())
}

// ──────────────────────────── adepts ───────────────────────────────

/// A worker that proposes a patch for a task given the current files. Behind this
/// seam sit the scripted adept (tests), an ollama adept, or a claude adept.
pub trait AdeptAgent {
    fn name(&self) -> &str;
    /// Propose a patch. `files` maps each file-of-interest to its current contents.
    fn attempt(&self, task: &str, files: &BTreeMap<String, String>, rng: &mut Rng) -> Patch;
}

/// A deterministic adept that always returns a fixed patch — for testing the loop
/// without a model.
pub struct ScriptedAgent {
    pub name: String,
    pub patch: Patch,
}

impl AdeptAgent for ScriptedAgent {
    fn name(&self) -> &str {
        &self.name
    }
    fn attempt(&self, _task: &str, _files: &BTreeMap<String, String>, _rng: &mut Rng) -> Patch {
        self.patch.clone()
    }
}

/// A real adept backed by a model. It is shown the task and the current file, and
/// must return the COMPLETE corrected file between `<FILE>`/`</FILE>` markers. The
/// conclave's diversity comes from the model's own sampling: k calls, k attempts,
/// majority-voted — the same self-consistency the benchmark proved.
pub struct ModelAgent {
    pub name: String,
    pub backend: ModelBackend,
    pub edit_path: String,
}

/// Which model an [`ModelAgent`] calls. Both shell out (zero-dep).
#[derive(Clone, Debug)]
pub enum ModelBackend {
    Ollama {
        model: String,
        timeout_s: u64,
    },
    /// The `claude` CLI; `model` is an optional `--model` tag (`sonnet`, `opus`,
    /// a full id) — `None` keeps the CLI's default.
    Claude {
        model: Option<String>,
    },
}

/// The file-surgeon variant of the adept persona: the same sworn name and ray as
/// [`crate::backend::adept_system_prompt`], but where that prompt demands a final
/// `ANSWER:` line, this Weighing reads a whole file — the instruction must match
/// what [`extract_file`] parses, or the system prompt fights the user prompt.
fn model_agent_system_prompt(adept_name: &str, ray_name: &str, ray_sphere: &str) -> String {
    format!(
        "You are {adept_name}, a sworn adept of the Pact, working \
         the {ray_name} ray ({ray_sphere}). Return the COMPLETE corrected file between \
         <FILE> and </FILE> markers and nothing else \u{2014} no explanation, no ANSWER line."
    )
}

impl AdeptAgent for ModelAgent {
    fn name(&self) -> &str {
        &self.name
    }

    fn attempt(&self, task: &str, files: &BTreeMap<String, String>, rng: &mut Rng) -> Patch {
        let current = files.get(&self.edit_path).cloned().unwrap_or_default();
        let system = model_agent_system_prompt(&self.name, "Octarine", "code");
        let user = format!(
            "TASK: {task}\n\nFile `{}` currently reads:\n<FILE>\n{current}\n</FILE>\n\n\
             Return the COMPLETE corrected contents of `{}` between <FILE> and </FILE> \
             markers, and nothing else. Do not explain.",
            self.edit_path, self.edit_path
        );
        let reply = match &self.backend {
            ModelBackend::Ollama { model, timeout_s } => {
                let prompt = format!("{system}\n\n{user}");
                // A per-attempt seed drawn from the conclave's rng: the k attempts are
                // now genuinely diverse (distinct seeds) yet the whole run reproduces
                // from one master seed. This is what makes best-of-k honest.
                let sampling = crate::backend::Sampling::seeded(rng.next_u64());
                crate::backend::ollama_generate(
                    model,
                    &prompt,
                    std::time::Duration::from_secs(*timeout_s),
                    sampling,
                )
            }
            ModelBackend::Claude { model } => {
                crate::backend::fire_claude_as(model.as_deref(), &user, &system)
            }
        };
        match reply {
            Ok(text) => match extract_file(&text) {
                Some(contents) => Patch {
                    edits: vec![Edit::Write {
                        path: self.edit_path.clone(),
                        contents,
                    }],
                },
                None => Patch::new(), // unparseable → empty patch → counts as a miss
            },
            Err(_) => Patch::new(),
        }
    }
}

/// Pull file contents out of a model reply: prefer `<FILE>…</FILE>`, then a fenced
/// ```` ``` ```` block, else the whole trimmed reply.
fn extract_file(text: &str) -> Option<String> {
    if let (Some(a), Some(b)) = (text.find("<FILE>"), text.rfind("</FILE>")) {
        if b > a {
            let inner = &text[a + "<FILE>".len()..b];
            return Some(inner.trim_matches('\n').to_string());
        }
    }
    if let Some(start) = text.find("```") {
        let after = &text[start + 3..];
        // skip an optional language tag line
        let after = after.split_once('\n').map(|x| x.1).unwrap_or(after);
        if let Some(end) = after.find("```") {
            return Some(after[..end].trim_matches('\n').to_string());
        }
    }
    let t = text.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

// ──────────────────────────── conclave ─────────────────────────────

/// One adept's record within a solve.
#[derive(Clone, Debug)]
pub struct AttemptRec {
    pub adept: String,
    pub passed: bool,
    pub patch_key: String,
    pub note: String,
}

/// The outcome of a verified-conclave solve.
#[derive(Clone, Debug)]
pub struct Verdict {
    pub shipped: bool,
    pub k: usize,
    pub passed: usize,
    pub winner: Option<String>,
    pub attempts: Vec<AttemptRec>,
}

/// Convene the conclave on a task. Each agent works in isolation; we apply and
/// verify each patch; among the **verified** patches we ship the consensus one back
/// into the real `target`. Nothing ships unverified.
pub fn solve(
    target: &Path,
    task: &str,
    files_of_interest: &[&str],
    verify_cmd: &str,
    agents: &[Box<dyn AdeptAgent>],
    rng: &mut Rng,
) -> io::Result<Verdict> {
    let mut attempts = Vec::new();
    // Verified patches, keyed for voting.
    let mut passing: Vec<Patch> = Vec::new();

    for agent in agents {
        let ws = Workspace::isolate(target)?;
        let files = ws.read_files(files_of_interest);
        let patch = agent.attempt(task, &files, rng);

        let (passed, note) = if patch.is_empty() {
            (false, "empty patch".to_string())
        } else {
            match ws.apply(&patch) {
                Ok(()) => {
                    let (ok, log) = ws.verify(verify_cmd);
                    (ok, summarize(&log))
                }
                Err(e) => (false, format!("apply failed: {e}")),
            }
        };

        attempts.push(AttemptRec {
            adept: agent.name().to_string(),
            passed,
            patch_key: patch.key(),
            note,
        });
        if passed {
            passing.push(patch);
        }
        // ws dropped here → isolated copy removed
    }

    // Vote among verified patches by canonical key; ship the modal one.
    let winner = majority_patch(&passing);
    let shipped = winner.is_some();
    if let Some(p) = &winner {
        // Apply the winning, verified patch to the real target.
        let real = Workspace {
            root: target.to_path_buf(),
        };
        let res = real.apply(p);
        std::mem::forget(real); // do NOT delete the user's target on drop
        res?;
    }

    Ok(Verdict {
        shipped,
        k: agents.len(),
        passed: passing.len(),
        winner: winner.map(|p| p.key()),
        attempts,
    })
}

/// The modal patch among the verified set (ties → first seen).
fn majority_patch(passing: &[Patch]) -> Option<Patch> {
    let mut best: Option<(&Patch, usize)> = None;
    for p in passing {
        let count = passing.iter().filter(|q| q.key() == p.key()).count();
        match &best {
            Some((_, bc)) if *bc >= count => {}
            _ => best = Some((p, count)),
        }
    }
    best.map(|(p, _)| p.clone())
}

fn summarize(log: &str) -> String {
    let line = log
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("");
    let line = line.trim();
    if line.len() > 80 {
        line.chars().take(80).collect()
    } else {
        line.to_string()
    }
}

// ─────────────────────────── demo arena ────────────────────────────

/// Write a tiny broken Python project into `dir` and return (files_of_interest,
/// verify_cmd). The bug: `add` subtracts. The verifier runs the project's check.
pub fn write_demo_arena(dir: &Path) -> io::Result<(Vec<String>, String)> {
    fs::create_dir_all(dir)?;
    fs::write(
        dir.join("sol.py"),
        "def add(a, b):\n    return a - b  # BUG: should add\n",
    )?;
    fs::write(
        dir.join("test_sol.py"),
        "from sol import add\n\n\
         def test_add():\n    \
         assert add(2, 3) == 5\n    \
         assert add(10, 5) == 15\n    \
         assert add(0, 0) == 0\n",
    )?;
    // Prefer pytest; fall back to a plain assertion runner if pytest is absent.
    let verify = "python3 -m pytest -q test_sol.py >/dev/null 2>&1 || \
                  python3 -c \"from sol import add; assert add(2,3)==5 and add(10,5)==15 and add(0,0)==0\""
        .to_string();
    Ok((vec!["sol.py".to_string()], verify))
}

/// The patch that fixes the demo (for the scripted/offline solve path).
pub fn demo_fix_patch() -> Patch {
    Patch {
        edits: vec![Edit::Replace {
            path: "sol.py".to_string(),
            find: "return a - b".to_string(),
            replace: "return a + b".to_string(),
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scripted(name: &str, patch: Patch) -> Box<dyn AdeptAgent> {
        Box::new(ScriptedAgent {
            name: name.to_string(),
            patch,
        })
    }

    /// A self-contained target dir in temp: one file + a grep verifier. Returns its
    /// path (cleaned up by the test via a Workspace-less manual remove).
    fn make_target(contents: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        // A per-call atomic counter guarantees uniqueness even when two tests hash the
        // same nanosecond (parallel test runners do collide otherwise).
        let n = WS_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("kaos-test-{}-{nanos}-{n}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("answer.txt"), contents).unwrap();
        dir
    }

    #[test]
    fn isolate_does_not_mutate_source() {
        let target = make_target("ORIGINAL");
        let ws = Workspace::isolate(&target).unwrap();
        ws.apply(&Patch {
            edits: vec![Edit::Write {
                path: "answer.txt".into(),
                contents: "CHANGED".into(),
            }],
        })
        .unwrap();
        // Source untouched; workspace changed.
        assert_eq!(
            fs::read_to_string(target.join("answer.txt")).unwrap(),
            "ORIGINAL"
        );
        assert_eq!(
            fs::read_to_string(ws.root.join("answer.txt")).unwrap(),
            "CHANGED"
        );
        let _ = fs::remove_dir_all(&target);
    }

    #[test]
    fn workspace_dropped_is_removed() {
        let target = make_target("X");
        let root = {
            let ws = Workspace::isolate(&target).unwrap();
            ws.root.clone()
        };
        assert!(!root.exists(), "workspace should be removed on drop");
        let _ = fs::remove_dir_all(&target);
    }

    #[test]
    fn verify_runs_the_command() {
        let target = make_target("WRONG");
        let ws = Workspace::isolate(&target).unwrap();
        let (bad, _) = ws.verify("grep -q RIGHT answer.txt");
        assert!(!bad);
        ws.apply(&Patch {
            edits: vec![Edit::Write {
                path: "answer.txt".into(),
                contents: "RIGHT".into(),
            }],
        })
        .unwrap();
        let (good, _) = ws.verify("grep -q RIGHT answer.txt");
        assert!(good);
        let _ = fs::remove_dir_all(&target);
    }

    #[test]
    fn verify_kills_a_hanging_gate() {
        // A gate that never returns must weigh false in bounded time, not hang the
        // conclave forever.
        let target = make_target("X");
        let ws = Workspace::isolate(&target).unwrap();
        let (ok, log) = ws.verify_within("sleep 30", 1);
        assert!(!ok, "a hung gate must weigh false");
        assert!(log.contains("gate timed out after 1s"), "log was: {log}");
        let _ = fs::remove_dir_all(&target);
    }

    #[test]
    fn conclave_ships_only_verified_and_writes_back() {
        let target = make_target("WRONG");
        let verify = "grep -q RIGHT answer.txt";
        let wrong = Patch {
            edits: vec![Edit::Write {
                path: "answer.txt".into(),
                contents: "STILL WRONG".into(),
            }],
        };
        let right = Patch {
            edits: vec![Edit::Write {
                path: "answer.txt".into(),
                contents: "RIGHT".into(),
            }],
        };
        let agents = vec![
            scripted("Soror Wrong", wrong),
            scripted("Frater Right", right),
        ];
        let mut rng = Rng::new(1);
        let v = solve(
            &target,
            "make it right",
            &["answer.txt"],
            verify,
            &agents,
            &mut rng,
        )
        .unwrap();
        assert!(v.shipped);
        assert_eq!(v.passed, 1);
        // The verified patch was written back to the real target.
        assert_eq!(
            fs::read_to_string(target.join("answer.txt")).unwrap(),
            "RIGHT"
        );
        let _ = fs::remove_dir_all(&target);
    }

    #[test]
    fn conclave_ships_nothing_when_all_fail() {
        let target = make_target("WRONG");
        let verify = "grep -q RIGHT answer.txt";
        let a = Patch {
            edits: vec![Edit::Write {
                path: "answer.txt".into(),
                contents: "NOPE".into(),
            }],
        };
        let b = Patch {
            edits: vec![Edit::Write {
                path: "answer.txt".into(),
                contents: "ALSO NO".into(),
            }],
        };
        let agents = vec![scripted("A", a), scripted("B", b)];
        let mut rng = Rng::new(2);
        let v = solve(&target, "t", &["answer.txt"], verify, &agents, &mut rng).unwrap();
        assert!(!v.shipped);
        assert_eq!(v.passed, 0);
        // Target left untouched — never ship unverified.
        assert_eq!(
            fs::read_to_string(target.join("answer.txt")).unwrap(),
            "WRONG"
        );
        let _ = fs::remove_dir_all(&target);
    }

    #[test]
    fn conclave_prefers_consensus_among_verified() {
        let target = make_target("v");
        // Two distinct *verified* patches: consensus (2 votes) must win.
        let verify = "grep -Eq 'RIGHT|FINE' answer.txt";
        let consensus = Patch {
            edits: vec![Edit::Write {
                path: "answer.txt".into(),
                contents: "RIGHT".into(),
            }],
        };
        let lone = Patch {
            edits: vec![Edit::Write {
                path: "answer.txt".into(),
                contents: "FINE".into(),
            }],
        };
        let agents = vec![
            scripted("A", consensus.clone()),
            scripted("B", lone),
            scripted("C", consensus.clone()),
        ];
        let mut rng = Rng::new(3);
        let v = solve(&target, "t", &["answer.txt"], verify, &agents, &mut rng).unwrap();
        assert!(v.shipped);
        assert_eq!(v.passed, 3);
        assert_eq!(v.winner.as_deref(), Some(consensus.key()).as_deref());
        assert_eq!(
            fs::read_to_string(target.join("answer.txt")).unwrap(),
            "RIGHT"
        );
        let _ = fs::remove_dir_all(&target);
    }

    #[test]
    fn replace_missing_target_is_an_error_not_silent() {
        let target = make_target("hello");
        let ws = Workspace::isolate(&target).unwrap();
        let r = ws.apply(&Patch {
            edits: vec![Edit::Replace {
                path: "answer.txt".into(),
                find: "ABSENT".into(),
                replace: "x".into(),
            }],
        });
        assert!(r.is_err());
        let _ = fs::remove_dir_all(&target);
    }

    #[test]
    fn extract_file_handles_markers_and_fences() {
        assert_eq!(
            extract_file("junk <FILE>\nhello\n</FILE> trailing").as_deref(),
            Some("hello")
        );
        assert_eq!(
            extract_file("```python\ncode here\n```").as_deref(),
            Some("code here")
        );
        assert_eq!(extract_file("   bare text  ").as_deref(), Some("bare text"));
        assert_eq!(extract_file("   "), None);
    }

    #[test]
    fn demo_arena_fix_verifies() {
        let target = make_target("placeholder");
        let _ = fs::remove_file(target.join("answer.txt"));
        let (foi, verify) = write_demo_arena(&target).unwrap();
        let foi_refs: Vec<&str> = foi.iter().map(|s| s.as_str()).collect();
        // Before the fix, the verifier fails.
        let ws = Workspace::isolate(&target).unwrap();
        assert!(!ws.verify(&verify).0, "broken arena should fail its tests");
        drop(ws);
        // The conclave with the fix patch ships and the arena now passes.
        let agents = vec![scripted("Frater Fix", demo_fix_patch())];
        let mut rng = Rng::new(4);
        let v = solve(&target, "fix add", &foi_refs, &verify, &agents, &mut rng).unwrap();
        assert!(v.shipped, "fix should verify and ship");
        let after = Workspace {
            root: target.clone(),
        };
        let ok = after.verify(&verify).0;
        std::mem::forget(after);
        assert!(ok, "target should pass after the fix is shipped");
        let _ = fs::remove_dir_all(&target);
    }
}
