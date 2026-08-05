//! The dream: what a run decided to remember after it ended.
//!
//! Rebis has one form that reads memory and one that writes it. `(? topic)`
//! recalls, and it recalls from the *record* — which every accepted answer
//! enters as it is produced, and which is thrown away when the run finishes.
//! `(! A)` marks one answer to survive that. The language does not say what
//! surviving means; it reports the marked answers in `Orchestration::kept` and
//! stops there. This is Kaos saying what it means: a text file in the project,
//! read back into the seed record of every later run.
//!
//! It belongs to the **project**, not to the machine. A memory is only useful
//! because a later run can recall it, and recall is topical — so the answers
//! worth carrying into a run are the ones about the thing that run is about.
//! One global memory would mix every project a person has ever worked on into
//! one corpus and make every recall vaguer; the store therefore lives in the
//! repository Kaos was opened in, beside the code the answers are about, and
//! moves with it.
//!
//! **Forgetting is the default, and that is the design.** A memory that keeps
//! every answer of every run is a memory recall cannot find anything in — `?`
//! scores topical overlap, so each stale line makes the next recall noisier.
//! Two things hold that line here: only marked answers are written at all, and
//! the file keeps the most recent [`CAPACITY`] of them. A dream is a claim that
//! something is worth carrying, and the store is allowed to stop believing the
//! oldest claims.
//!
//! The format is the same shape as the run-nesting sidecar: entries separated
//! by a byte the language cannot produce, each carrying when it was kept. A
//! text file, because the whole point is that a person can read what their
//! project is remembering, and delete a line of it with an editor.

use std::path::{Path, PathBuf};

/// How many kept answers the store carries before the oldest fall off.
///
/// Not a size limit — a *recall* limit. A flashback broadens a topic through
/// the co-occurrence graph of everything it can see, so the cost of a large
/// memory is not disk, it is that every recall gets vaguer. Five hundred
/// deliberately kept answers is already a long-lived body of evidence; past
/// that, keeping more makes the memory worse at its job.
pub const CAPACITY: usize = 500;

/// Separates one kept answer from the next.
///
/// A line that cannot occur in Rebis source or in a model's prose: `\x1e` is
/// the ASCII record separator, so an answer cannot split itself in two.
const SEPARATOR: &str = "\x1e\n";

/// Separates an entry's provenance from its text.
const FIELD: &str = "\x1f\n";

/// One answer a run asked to keep.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Kept {
    /// Unix seconds when it was kept. Provenance, so a recall that surprises
    /// someone can be traced to the run that put it there rather than being
    /// mysterious.
    pub at: u64,
    /// The answer, exactly as it was given.
    pub text: String,
}

/// The durable memory: one file, read whole, written whole.
#[derive(Clone, Debug)]
pub struct Dream {
    path: PathBuf,
}

impl Default for Dream {
    fn default() -> Self {
        Self::here()
    }
}

impl Dream {
    /// A store at an explicit path — what tests and alternative hosts use.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The store for the project containing `from`: `<repo>/.kaos/dream`.
    ///
    /// The repository is the first ancestor holding a `.git`, because that is
    /// what "the project" means to everyone who will read the file, and it is
    /// stable under `cd` into a subdirectory — a run started in `src/` and one
    /// started at the top must remember the same things. Outside a repository
    /// the directory itself is the project, which is the honest answer for a
    /// scratch folder.
    #[must_use]
    pub fn for_project(from: impl AsRef<Path>) -> Self {
        Self::new(project_root(from.as_ref()).join(".kaos").join("dream"))
    }

    /// The store for the current working directory.
    #[must_use]
    pub fn here() -> Self {
        Self::for_project(std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    }

    /// Where the memory lives, for the status line and for `/dream forget`.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Everything kept, oldest first.
    ///
    /// A missing or unreadable file is an empty memory rather than an error:
    /// the first run in a project has nothing to remember, and that is the
    /// normal case, not a failure.
    #[must_use]
    pub fn all(&self) -> Vec<Kept> {
        let Ok(contents) = std::fs::read_to_string(&self.path) else {
            return Vec::new();
        };
        contents
            .split(SEPARATOR)
            .filter_map(|entry| {
                let (at, text) = entry.split_once(FIELD)?;
                let text = text.trim();
                (!text.is_empty()).then(|| Kept {
                    at: at.trim().parse().unwrap_or_default(),
                    text: text.to_string(),
                })
            })
            .collect()
    }

    /// Just the texts, which is what a record is seeded from.
    #[must_use]
    pub fn texts(&self) -> Vec<String> {
        self.all().into_iter().map(|kept| kept.text).collect()
    }

    /// How many answers are being carried.
    #[must_use]
    pub fn len(&self) -> usize {
        self.all().len()
    }

    /// Whether the project remembers nothing yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.all().is_empty()
    }

    /// Keep these answers, oldest of the existing first, dropping whatever
    /// falls past [`CAPACITY`].
    ///
    /// An answer already in the store is not written twice: a program run
    /// nightly would otherwise fill the memory with copies of one sentence and
    /// crowd out everything else. Empty answers are skipped — a dream over a
    /// declined answer marks nothing.
    ///
    /// # Errors
    ///
    /// Returns a message when the file cannot be written.
    pub fn keep<S: AsRef<str>>(&self, answers: &[S]) -> Result<usize, String> {
        let fresh: Vec<&str> = answers
            .iter()
            .map(|answer| answer.as_ref().trim())
            .filter(|answer| !answer.is_empty())
            .collect();
        if fresh.is_empty() {
            return Ok(0);
        }
        let mut entries = self.all();
        let now = now_seconds();
        let mut added = 0;
        for text in fresh {
            if entries.iter().any(|kept| kept.text == text) {
                continue;
            }
            entries.push(Kept {
                at: now,
                text: text.to_string(),
            });
            added += 1;
        }
        if added == 0 {
            return Ok(0);
        }
        if entries.len() > CAPACITY {
            entries.drain(..entries.len() - CAPACITY);
        }
        self.write(&entries)?;
        Ok(added)
    }

    /// Forget one answer by its position in [`Self::all`], or everything.
    ///
    /// # Errors
    ///
    /// Returns a message when the file cannot be written, or when the position
    /// names nothing.
    pub fn forget(&self, which: Option<usize>) -> Result<usize, String> {
        let mut entries = self.all();
        match which {
            None => {
                let count = entries.len();
                let _ = std::fs::remove_file(&self.path);
                Ok(count)
            }
            Some(index) if index < entries.len() => {
                entries.remove(index);
                self.write(&entries)?;
                Ok(1)
            }
            Some(index) => Err(format!(
                "dream · nothing is remembered at {index} (there {} {})",
                if entries.len() == 1 { "is" } else { "are" },
                entries.len()
            )),
        }
    }

    /// Write the whole file, atomically.
    ///
    /// Through a temporary and a rename, like the session store: a run that
    /// dies mid-write must not leave a machine with half a memory.
    fn write(&self, entries: &[Kept]) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            ignore_by_default(parent);
        }
        let body = entries
            .iter()
            .map(|kept| format!("{}{FIELD}{}", kept.at, kept.text))
            .collect::<Vec<_>>()
            .join(SEPARATOR);
        let temporary = self.path.with_extension("tmp");
        std::fs::write(&temporary, body).map_err(|error| {
            format!("could not write {}: {error}", temporary.display())
        })?;
        std::fs::rename(&temporary, &self.path)
            .map_err(|error| format!("could not write {}: {error}", self.path.display()))
    }
}

/// The repository `from` belongs to, or `from` itself outside one.
fn project_root(from: &Path) -> PathBuf {
    let mut cursor = if from.is_absolute() {
        from.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(from)
    };
    loop {
        let git = cursor.join(".git");
        if git.is_dir() {
            return cursor;
        }
        if git.is_file() {
            return main_worktree(&git).unwrap_or(cursor);
        }
        if !cursor.pop() {
            break;
        }
    }
    from.to_path_buf()
}

/// The repository a linked worktree belongs to.
///
/// Kaos runs parallel `[]` branches in throwaway worktrees when they are
/// allowed tools, and a worktree's `.git` is a *file* reading
/// `gitdir: <repo>/.git/worktrees/<name>`. Left alone, each branch would keep
/// its dreams inside a directory that is deleted the moment the branch
/// finishes — the memory would be written, reported, and then thrown away,
/// which is worse than not having kept it. What a branch learns belongs to the
/// repository the branch is a branch OF.
///
/// A submodule's `.git` file points into `<super>/.git/modules/…` instead, and
/// that is deliberately not redirected: a submodule has its own history and its
/// own code, so it is its own project and keeps its own memory.
fn main_worktree(pointer: &Path) -> Option<PathBuf> {
    let contents = std::fs::read_to_string(pointer).ok()?;
    let gitdir = contents.lines().find_map(|line| {
        line.strip_prefix("gitdir:")
            .map(|rest| PathBuf::from(rest.trim()))
    })?;
    // `<repo>/.git/worktrees/<name>` — three steps back up is the repository.
    let mut root = gitdir.clone();
    let name = root.file_name().map(|name| name.to_owned());
    if !root.pop() || root.file_name() != Some(std::ffi::OsStr::new("worktrees")) {
        return None;
    }
    let _ = name;
    if !root.pop() || root.file_name() != Some(std::ffi::OsStr::new(".git")) {
        return None;
    }
    root.pop().then_some(root)
}

/// Keep the store out of commits unless someone decides otherwise.
///
/// The file holds model output, written by programs, into a directory inside
/// somebody's repository — and `git add -A` does not ask. So the first write
/// leaves a `.gitignore` beside it that ignores the whole directory. Deleting
/// that one line is how a team opts into sharing what their programs learned,
/// which is a decision worth making on purpose rather than by default.
///
/// Best effort: a store outside a repository, or one whose directory cannot be
/// written to, simply has no guard, and the memory still works.
fn ignore_by_default(directory: &Path) {
    let guard = directory.join(".gitignore");
    if guard.exists() {
        return;
    }
    let _ = std::fs::write(
        &guard,
        "# Kaos keeps this project's dream here — what `(! A)` decided to
# remember. It is model output, so it is ignored by default. Delete this
# file to share the memory with everyone who clones the repository.
*
",
    );
}

fn now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> Dream {
        let path = std::env::temp_dir().join(format!("kaos-dream-{}-{name}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        Dream::new(path)
    }

    #[test]
    fn a_kept_answer_comes_back_whole() {
        let dream = scratch("whole");
        assert!(dream.is_empty(), "a fresh machine remembers nothing");

        let multiline = "the queue retries three times\nthen it gives up";
        assert_eq!(dream.keep(&[multiline]).unwrap(), 1);
        assert_eq!(dream.texts(), vec![multiline.to_string()]);
        assert_eq!(dream.len(), 1);
        // Provenance is carried, so a surprising recall can be traced.
        assert!(dream.all()[0].at > 0);

        let _ = dream.forget(None);
    }

    #[test]
    fn the_same_answer_is_never_carried_twice() {
        // A program run nightly would otherwise fill the memory with copies of
        // one sentence and crowd out everything else.
        let dream = scratch("once");
        dream.keep(&["one finding"]).unwrap();
        assert_eq!(dream.keep(&["one finding"]).unwrap(), 0);
        dream.keep(&["another finding"]).unwrap();
        assert_eq!(dream.len(), 2);
        // And a declined or empty answer marks nothing.
        assert_eq!(dream.keep(&["", "   "]).unwrap(), 0);
        assert_eq!(dream.len(), 2);

        let _ = dream.forget(None);
    }

    #[test]
    fn the_oldest_claims_fall_off_the_end() {
        let dream = scratch("capacity");
        let many: Vec<String> = (0..CAPACITY + 10).map(|index| format!("finding {index}")).collect();
        dream.keep(&many).unwrap();
        let kept = dream.texts();
        assert_eq!(kept.len(), CAPACITY, "the store grew past its capacity");
        assert_eq!(
            kept.first().map(String::as_str),
            Some("finding 10"),
            "the oldest claims were not the ones dropped"
        );
        assert_eq!(kept.last().map(String::as_str), Some("finding 509"));

        let _ = dream.forget(None);
    }

    #[test]
    fn a_dream_belongs_to_the_repository_it_was_opened_in() {
        let base = std::env::temp_dir().join(format!("kaos-dream-repo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let deep = base.join("crate").join("src");
        std::fs::create_dir_all(&deep).expect("the fixture directories");
        std::fs::create_dir_all(base.join(".git")).expect("the fixture repository");

        // A run started in a subdirectory remembers what a run started at the
        // top remembers: the project is the repository, not the shell's cwd.
        assert_eq!(
            Dream::for_project(&deep).path(),
            Dream::for_project(&base).path(),
            "the same repository gave two different memories"
        );
        assert_eq!(
            Dream::for_project(&base).path(),
            base.join(".kaos").join("dream")
        );

        // Two projects do not share one memory, which is the whole point of
        // attaching it to the repository.
        let other = base.join("elsewhere");
        std::fs::create_dir_all(other.join(".git")).expect("a second repository");
        assert_ne!(
            Dream::for_project(&other).path(),
            Dream::for_project(&base).path()
        );

        // And the first write leaves the store ignored, so model output cannot
        // be swept into a commit by `git add -A`.
        let dream = Dream::for_project(&deep);
        dream.keep(&["a finding worth keeping"]).unwrap();
        let guard = base.join(".kaos").join(".gitignore");
        assert!(guard.is_file(), "the store was left committable");
        assert!(std::fs::read_to_string(&guard).unwrap().contains('*'));
        assert_eq!(Dream::for_project(&base).texts().len(), 1);

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_branch_running_in_a_worktree_keeps_its_findings_in_the_repository() {
        // Kaos runs parallel `[]` branches in throwaway worktrees. A worktree's
        // `.git` is a file pointing back at the repository, and a dream kept
        // inside one would be deleted with it — written, reported, and thrown
        // away, which is worse than never keeping it.
        let base = std::env::temp_dir().join(format!("kaos-dream-tree-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let repo = base.join("project");
        let tree = base.join("branch-work");
        std::fs::create_dir_all(repo.join(".git").join("worktrees").join("branch-work"))
            .expect("the fixture repository");
        std::fs::create_dir_all(&tree).expect("the fixture worktree");
        std::fs::write(
            tree.join(".git"),
            format!(
                "gitdir: {}\n",
                repo.join(".git").join("worktrees").join("branch-work").display()
            ),
        )
        .expect("the worktree pointer");

        assert_eq!(
            Dream::for_project(&tree).path(),
            Dream::for_project(&repo).path(),
            "a branch's memory did not reach the repository it is a branch of"
        );

        // A submodule is its own project and keeps its own memory: its pointer
        // goes to `modules`, not `worktrees`, and is deliberately not followed.
        let submodule = repo.join("vendor").join("thing");
        std::fs::create_dir_all(&submodule).expect("the fixture submodule");
        std::fs::write(
            submodule.join(".git"),
            format!("gitdir: {}\n", repo.join(".git").join("modules").join("thing").display()),
        )
        .expect("the submodule pointer");
        assert_eq!(
            Dream::for_project(&submodule).path(),
            submodule.join(".kaos").join("dream")
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn forgetting_takes_one_or_all() {
        let dream = scratch("forget");
        dream.keep(&["first", "second", "third"]).unwrap();
        assert_eq!(dream.forget(Some(1)).unwrap(), 1);
        assert_eq!(dream.texts(), vec!["first".to_string(), "third".to_string()]);
        assert!(dream.forget(Some(9)).is_err(), "there is no ninth memory");
        assert_eq!(dream.forget(None).unwrap(), 2);
        assert!(dream.is_empty());
    }
}
