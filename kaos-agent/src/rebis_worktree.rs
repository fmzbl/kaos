//! Optional Git-worktree isolation for parallel Rebis square branches.
//!
//! Rebis owns branch scheduling but deliberately knows nothing about files.
//! This host-side adapter gives every concurrent branch a detached worktree,
//! snapshots its final working tree, and reconciles all branch snapshots back
//! into the parent workspace in source order before the square mediator runs.

use rebis_lang::ExecutionScope;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Persistent setting that opts tool-using Rebis runs into Git isolation.
pub const CONFIG_KEY: &str = "KAOS_REBIS_GIT_WORKTREES";

#[derive(Clone, Debug)]
struct Checkout {
    worktree_root: PathBuf,
    agent_root: PathBuf,
    base_commit: String,
    base_tree: String,
}

#[derive(Debug)]
struct State {
    checkouts: BTreeMap<ExecutionScope, Checkout>,
    managed: BTreeSet<PathBuf>,
    counter: u64,
}

#[derive(Debug)]
struct Snapshot {
    repository_root: PathBuf,
    relative_agent_root: PathBuf,
    commit: String,
    tree: String,
}

/// A run-local manager for detached worktrees used by Rebis branch scopes.
///
/// Construction is a capability probe: it fails without Git, outside a
/// repository, or on a Git build without `worktree`. Callers should report the
/// returned guidance and continue with Rebis's sequential entry point.
pub struct GitWorktrees {
    original_root: PathBuf,
    original_repository: PathBuf,
    temporary_root: PathBuf,
    state: Mutex<State>,
}

impl GitWorktrees {
    /// Probe Git and prepare a run-local worktree area.
    ///
    /// # Errors
    ///
    /// Returns actionable text when Git or repository support is unavailable.
    pub fn new(root: &Path) -> Result<Self, String> {
        match Command::new("git").arg("--version").output() {
            Ok(output) if output.status.success() => {}
            Ok(output) => {
                return Err(format!(
                    "`git --version` failed: {}. Upgrade or reinstall Git to use \
                     {CONFIG_KEY}",
                    retained_error(&output.stderr)
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(format!(
                    "Git is not installed. Install it with your system package \
                     manager (for example `apt install git`, `brew install git`, \
                     or `winget install Git.Git`), then enable {CONFIG_KEY}=1"
                ));
            }
            Err(error) => {
                return Err(format!(
                    "Git could not be started ({error}). Install or repair Git, then \
                     enable {CONFIG_KEY}=1"
                ));
            }
        }

        let original_root = fs::canonicalize(root)
            .map_err(|error| format!("could not resolve {}: {error}", root.display()))?;
        let original_repository = repository_root(&original_root).map_err(|error| {
            format!(
                "{} is not an available Git working tree ({error})",
                original_root.display()
            )
        })?;
        git_text(
            &original_repository,
            ["rev-parse", "--verify", "HEAD^{commit}"],
        )
        .map_err(|error| format!("the Git working tree has no usable HEAD commit ({error})"))?;
        git_status(&original_repository, ["worktree", "list", "--porcelain"]).map_err(|error| {
            format!(
                "this Git installation does not provide usable worktrees ({error}); \
                     upgrade Git to use {CONFIG_KEY}"
            )
        })?;

        let temporary_root = unique_temp_dir()?;
        Ok(Self {
            original_root,
            original_repository,
            temporary_root,
            state: Mutex::new(State {
                checkouts: BTreeMap::new(),
                managed: BTreeSet::new(),
                counter: 0,
            }),
        })
    }

    /// Prepare one detached worktree per source-ordered child scope.
    ///
    /// Every checkout starts from the exact parent working tree, including
    /// tracked modifications and non-ignored untracked files. The user's Git
    /// index and branch are not changed.
    pub fn begin(
        &self,
        parent: &ExecutionScope,
        children: &[ExecutionScope],
    ) -> Result<(), String> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if children.is_empty() {
            return Ok(());
        }
        let parent_root = self.workspace_locked(&state, parent)?;
        let base = self.snapshot(&mut state, &parent_root, None)?;
        let mut created = Vec::new();
        let mut created_scopes = Vec::new();

        for child in children {
            if state.checkouts.contains_key(child) {
                self.cleanup_paths_locked(&mut state, &created);
                for scope in &created_scopes {
                    state.checkouts.remove(scope);
                }
                return Err(format!(
                    "parallel Rebis scope {child} already has a worktree"
                ));
            }
            let path = self.next_path(&mut state, "branch");
            if let Err(error) = git_status(
                &base.repository_root,
                [
                    OsStr::new("worktree"),
                    OsStr::new("add"),
                    OsStr::new("--detach"),
                    path.as_os_str(),
                    OsStr::new(&base.commit),
                ],
            ) {
                self.cleanup_paths_locked(&mut state, &created);
                for scope in &created_scopes {
                    state.checkouts.remove(scope);
                }
                return Err(format!("could not create worktree for {child}: {error}"));
            }
            state.managed.insert(path.clone());
            created.push(path.clone());
            state.checkouts.insert(
                child.clone(),
                Checkout {
                    agent_root: path.join(&base.relative_agent_root),
                    worktree_root: path,
                    base_commit: base.commit.clone(),
                    base_tree: base.tree.clone(),
                },
            );
            created_scopes.push(child.clone());
        }
        Ok(())
    }

    /// Return the filesystem root assigned to a prompt's execution scope.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown non-root scope.
    pub fn workspace(&self, scope: &ExecutionScope) -> Result<PathBuf, String> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.workspace_locked(&state, scope)
    }

    /// Reconcile child worktrees into their parent and remove them.
    ///
    /// Synthetic commits and an integration worktree let Git perform a
    /// three-way merge without touching the user's branch or index. Children
    /// are applied in source order; when the same hunk conflicts, the later
    /// child wins. The final combined patch is applied to the parent working
    /// tree without staging it.
    pub fn finish(
        &self,
        parent: &ExecutionScope,
        children: &[ExecutionScope],
    ) -> Result<(), String> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if children.is_empty() {
            return Ok(());
        }
        let parent_root = self.workspace_locked(&state, parent)?;
        let checkouts = children
            .iter()
            .map(|scope| {
                state
                    .checkouts
                    .get(scope)
                    .cloned()
                    .ok_or_else(|| format!("parallel Rebis scope {scope} has no worktree"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let base_commit = checkouts[0].base_commit.clone();
        let base_tree = checkouts[0].base_tree.clone();
        if checkouts
            .iter()
            .any(|checkout| checkout.base_commit != base_commit)
        {
            return Err("parallel Rebis branches do not share one Git base".to_string());
        }

        let result = (|| {
            let mut commits = Vec::new();
            for checkout in &checkouts {
                let snapshot =
                    self.snapshot(&mut state, &checkout.agent_root, Some(&base_commit))?;
                if snapshot.tree != base_tree {
                    commits.push(snapshot.commit);
                }
            }
            if commits.is_empty() {
                return Ok(());
            }

            let integration = self.next_path(&mut state, "join");
            git_status(
                &self.original_repository,
                [
                    OsStr::new("worktree"),
                    OsStr::new("add"),
                    OsStr::new("--detach"),
                    integration.as_os_str(),
                    OsStr::new(&base_commit),
                ],
            )
            .map_err(|error| format!("could not create reconciliation worktree: {error}"))?;
            state.managed.insert(integration.clone());

            for commit in commits {
                git_status(
                    &integration,
                    [
                        OsStr::new("cherry-pick"),
                        OsStr::new("--no-commit"),
                        OsStr::new("-X"),
                        OsStr::new("theirs"),
                        OsStr::new(&commit),
                    ],
                )
                .map_err(|error| {
                    format!("could not reconcile parallel branches in source order: {error}")
                })?;
            }

            let patch = git_bytes(
                &integration,
                [
                    OsStr::new("diff"),
                    OsStr::new("--binary"),
                    OsStr::new("--full-index"),
                    OsStr::new(&base_commit),
                    OsStr::new("--"),
                ],
            )?;
            if !patch.is_empty() {
                let parent_repository = repository_root(&parent_root)?;
                git_with_input(
                    &parent_repository,
                    [
                        OsStr::new("apply"),
                        OsStr::new("--binary"),
                        OsStr::new("--whitespace=nowarn"),
                        OsStr::new("-"),
                    ],
                    &patch,
                )
                .map_err(|error| {
                    format!(
                        "parallel branches finished, but their combined patch no longer \
                         applies to {}: {error}",
                        parent_root.display()
                    )
                })?;
            }
            Ok(())
        })();

        let paths = checkouts
            .iter()
            .map(|checkout| checkout.worktree_root.clone())
            .collect::<Vec<_>>();
        for child in children {
            state.checkouts.remove(child);
        }
        self.cleanup_paths_locked(&mut state, &paths);
        // The integration path is not part of `paths`; remove every managed
        // join checkout that no live scope owns.
        let live = state
            .checkouts
            .values()
            .map(|checkout| checkout.worktree_root.clone())
            .collect::<BTreeSet<_>>();
        let joins = state
            .managed
            .iter()
            .filter(|path| !live.contains(*path))
            .cloned()
            .collect::<Vec<_>>();
        self.cleanup_paths_locked(&mut state, &joins);
        result
    }

    fn workspace_locked(&self, state: &State, scope: &ExecutionScope) -> Result<PathBuf, String> {
        if scope.is_root() {
            return Ok(self.original_root.clone());
        }
        state
            .checkouts
            .get(scope)
            .map(|checkout| checkout.agent_root.clone())
            .ok_or_else(|| format!("parallel Rebis scope {scope} has no worktree"))
    }

    fn snapshot(
        &self,
        state: &mut State,
        agent_root: &Path,
        parent: Option<&str>,
    ) -> Result<Snapshot, String> {
        let repository_root = repository_root(agent_root)?;
        let canonical_agent = fs::canonicalize(agent_root)
            .map_err(|error| format!("could not resolve {}: {error}", agent_root.display()))?;
        let canonical_repository = fs::canonicalize(&repository_root).map_err(|error| {
            format!(
                "could not resolve Git root {}: {error}",
                repository_root.display()
            )
        })?;
        let relative_agent_root = canonical_agent
            .strip_prefix(&canonical_repository)
            .map(Path::to_path_buf)
            .map_err(|_| {
                format!(
                    "{} is outside Git root {}",
                    canonical_agent.display(),
                    canonical_repository.display()
                )
            })?;
        let parent = match parent {
            Some(parent) => parent.to_string(),
            None => git_text(&repository_root, ["rev-parse", "--verify", "HEAD^{commit}"])?,
        };
        let index = self.next_path(state, "index");
        let index_env = [("GIT_INDEX_FILE", index.as_os_str())];
        let result = (|| {
            git_status_env(
                &repository_root,
                [OsStr::new("read-tree"), OsStr::new(&parent)],
                &index_env,
            )?;
            git_status_env(
                &repository_root,
                [OsStr::new("add"), OsStr::new("-A"), OsStr::new("--")],
                &index_env,
            )?;
            let tree = git_text_env(&repository_root, [OsStr::new("write-tree")], &index_env)?;
            let commit = commit_tree(&repository_root, &tree, &parent)?;
            Ok(Snapshot {
                repository_root,
                relative_agent_root,
                commit,
                tree,
            })
        })();
        let _ = fs::remove_file(index);
        result
    }

    fn next_path(&self, state: &mut State, kind: &str) -> PathBuf {
        let id = state.counter;
        state.counter += 1;
        self.temporary_root.join(format!("{kind}-{id}"))
    }

    fn cleanup_paths_locked(&self, state: &mut State, paths: &[PathBuf]) {
        for path in paths {
            if !path.starts_with(&self.temporary_root) || path == &self.temporary_root {
                continue;
            }
            let _ = git_status(
                &self.original_repository,
                [
                    OsStr::new("worktree"),
                    OsStr::new("remove"),
                    OsStr::new("--force"),
                    path.as_os_str(),
                ],
            );
            state.managed.remove(path);
            if path.exists() {
                let _ = fs::remove_dir_all(path);
            }
        }
        let _ = git_status(
            &self.original_repository,
            [OsStr::new("worktree"), OsStr::new("prune")],
        );
    }
}

impl Drop for GitWorktrees {
    fn drop(&mut self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let paths = state.managed.iter().cloned().collect::<Vec<_>>();
        self.cleanup_paths_locked(&mut state, &paths);
        if self.temporary_root.starts_with(std::env::temp_dir()) {
            let _ = fs::remove_dir_all(&self.temporary_root);
        }
    }
}

fn unique_temp_dir() -> Result<PathBuf, String> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    for attempt in 0..100u32 {
        let path = std::env::temp_dir().join(format!(
            "kaos-rebis-worktrees-{}-{nanos}-{attempt}",
            std::process::id()
        ));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(format!(
                    "could not create temporary worktree directory {}: {error}",
                    path.display()
                ));
            }
        }
    }
    Err("could not allocate a unique temporary worktree directory".to_string())
}

fn repository_root(root: &Path) -> Result<PathBuf, String> {
    git_text(root, ["rev-parse", "--show-toplevel"]).map(PathBuf::from)
}

fn commit_tree(repository: &Path, tree: &str, parent: &str) -> Result<String, String> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(repository)
        .args(["commit-tree", tree, "-p", parent])
        .env("GIT_AUTHOR_NAME", "Kaos Rebis")
        .env("GIT_AUTHOR_EMAIL", "rebis@localhost")
        .env("GIT_COMMITTER_NAME", "Kaos Rebis")
        .env("GIT_COMMITTER_EMAIL", "rebis@localhost")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("could not snapshot Git tree: {error}"))?;
    if let Some(mut input) = child.stdin.take() {
        input
            .write_all(b"kaos: snapshot parallel Rebis branch\n")
            .map_err(|error| format!("could not describe Git snapshot: {error}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("could not finish Git snapshot: {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(retained_error(&output.stderr))
    }
}

fn git_status<I, S>(root: &Path, args: I) -> Result<(), String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    git_status_env(root, args, &[])
}

fn git_status_env<I, S>(root: &Path, args: I, environment: &[(&str, &OsStr)]) -> Result<(), String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new("git");
    command.arg("-C").arg(root).args(args);
    for (key, value) in environment {
        command.env(key, value);
    }
    let output = command
        .output()
        .map_err(|error| format!("could not start Git: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(retained_error(&output.stderr))
    }
}

fn git_text<I, S>(root: &Path, args: I) -> Result<String, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    git_text_env(root, args, &[])
}

fn git_text_env<I, S>(
    root: &Path,
    args: I,
    environment: &[(&str, &OsStr)],
) -> Result<String, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new("git");
    command.arg("-C").arg(root).args(args);
    for (key, value) in environment {
        command.env(key, value);
    }
    let output = command
        .output()
        .map_err(|error| format!("could not start Git: {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(retained_error(&output.stderr))
    }
}

fn git_bytes<I, S>(root: &Path, args: I) -> Result<Vec<u8>, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|error| format!("could not start Git: {error}"))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(retained_error(&output.stderr))
    }
}

fn git_with_input<I, S>(root: &Path, args: I, input: &[u8]) -> Result<(), String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut child = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("could not start Git: {error}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(input)
            .map_err(|error| format!("could not send patch to Git: {error}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("could not finish Git: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(retained_error(&output.stderr))
    }
}

fn retained_error(stderr: &[u8]) -> String {
    const LIMIT: usize = 8 * 1024;
    let retained = &stderr[..stderr.len().min(LIMIT)];
    let mut text = String::from_utf8_lossy(retained).trim().to_string();
    if stderr.len() > LIMIT {
        text.push_str(" …");
    }
    if text.is_empty() {
        "Git exited unsuccessfully".to_string()
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success())
    }

    fn repo(label: &str) -> Option<PathBuf> {
        if !git_available() {
            return None;
        }
        let root = std::env::temp_dir().join(format!(
            "kaos-rebis-worktree-test-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        git_status(&root, ["init", "-q"]).unwrap();
        fs::write(root.join("base.txt"), "base\n").unwrap();
        git_status(&root, ["add", "base.txt"]).unwrap();
        let mut command = Command::new("git");
        command
            .arg("-C")
            .arg(&root)
            .args(["commit", "-q", "-m", "base"])
            .env("GIT_AUTHOR_NAME", "Kaos Test")
            .env("GIT_AUTHOR_EMAIL", "test@localhost")
            .env("GIT_COMMITTER_NAME", "Kaos Test")
            .env("GIT_COMMITTER_EMAIL", "test@localhost");
        assert!(command.status().unwrap().success());
        Some(root)
    }

    #[test]
    fn rejects_a_non_repository_without_panicking() {
        if !git_available() {
            return;
        }
        let root = unique_temp_dir().unwrap();
        let result = GitWorktrees::new(&root);
        assert!(result.is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn disjoint_children_reconcile_without_staging_the_parent() {
        let Some(root) = repo("disjoint") else {
            return;
        };
        let manager = GitWorktrees::new(&root).unwrap();
        let parent = ExecutionScope::root();
        let children = [parent.branch(0, 0), parent.branch(0, 1)];
        manager.begin(&parent, &children).unwrap();
        let left = manager.workspace(&children[0]).unwrap();
        let right = manager.workspace(&children[1]).unwrap();
        fs::write(left.join("left.txt"), "left\n").unwrap();
        fs::write(right.join("right.txt"), "right\n").unwrap();

        manager.finish(&parent, &children).unwrap();

        assert_eq!(fs::read_to_string(root.join("left.txt")).unwrap(), "left\n");
        assert_eq!(
            fs::read_to_string(root.join("right.txt")).unwrap(),
            "right\n"
        );
        let staged = git_bytes(&root, ["diff", "--cached", "--name-only"]).unwrap();
        assert!(
            staged.is_empty(),
            "reconciliation must not stage user files"
        );
        drop(manager);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn later_source_branch_wins_an_overlapping_hunk() {
        let Some(root) = repo("overlap") else {
            return;
        };
        let manager = GitWorktrees::new(&root).unwrap();
        let parent = ExecutionScope::root();
        let children = [parent.branch(0, 0), parent.branch(0, 1)];
        manager.begin(&parent, &children).unwrap();
        fs::write(
            manager.workspace(&children[0]).unwrap().join("base.txt"),
            "left\n",
        )
        .unwrap();
        fs::write(
            manager.workspace(&children[1]).unwrap().join("base.txt"),
            "right\n",
        )
        .unwrap();

        manager.finish(&parent, &children).unwrap();

        assert_eq!(
            fs::read_to_string(root.join("base.txt")).unwrap(),
            "right\n"
        );
        drop(manager);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn nested_square_snapshots_and_reconciles_its_parent_branch() {
        let Some(root) = repo("nested") else {
            return;
        };
        let manager = GitWorktrees::new(&root).unwrap();
        let top = ExecutionScope::root();
        let outer = [top.branch(0, 0), top.branch(0, 1)];
        manager.begin(&top, &outer).unwrap();
        let left = manager.workspace(&outer[0]).unwrap();
        fs::write(left.join("before-nested.txt"), "visible\n").unwrap();

        let nested = [outer[0].branch(0, 0), outer[0].branch(0, 1)];
        manager.begin(&outer[0], &nested).unwrap();
        for (index, scope) in nested.iter().enumerate() {
            let workspace = manager.workspace(scope).unwrap();
            assert_eq!(
                fs::read_to_string(workspace.join("before-nested.txt")).unwrap(),
                "visible\n"
            );
            fs::write(workspace.join(format!("nested-{index}.txt")), "nested\n").unwrap();
        }
        manager.finish(&outer[0], &nested).unwrap();
        fs::write(
            manager.workspace(&outer[1]).unwrap().join("right.txt"),
            "right\n",
        )
        .unwrap();
        manager.finish(&top, &outer).unwrap();

        assert!(root.join("before-nested.txt").is_file());
        assert!(root.join("nested-0.txt").is_file());
        assert!(root.join("nested-1.txt").is_file());
        assert!(root.join("right.txt").is_file());
        drop(manager);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn dirty_parent_snapshot_preserves_the_users_index() {
        let Some(root) = repo("dirty") else {
            return;
        };
        fs::write(root.join("base.txt"), "staged-by-user\n").unwrap();
        git_status(&root, ["add", "base.txt"]).unwrap();
        fs::write(root.join("base.txt"), "working-by-user\n").unwrap();
        fs::write(root.join("context.txt"), "untracked context\n").unwrap();

        let manager = GitWorktrees::new(&root).unwrap();
        let parent = ExecutionScope::root();
        let children = [parent.branch(0, 0), parent.branch(0, 1)];
        manager.begin(&parent, &children).unwrap();
        let left = manager.workspace(&children[0]).unwrap();
        assert_eq!(
            fs::read_to_string(left.join("base.txt")).unwrap(),
            "working-by-user\n"
        );
        assert_eq!(
            fs::read_to_string(left.join("context.txt")).unwrap(),
            "untracked context\n"
        );
        fs::write(left.join("base.txt"), "branch result\n").unwrap();
        manager.finish(&parent, &children).unwrap();

        assert_eq!(
            fs::read_to_string(root.join("base.txt")).unwrap(),
            "branch result\n"
        );
        assert_eq!(
            git_text(&root, ["show", ":base.txt"]).unwrap(),
            "staged-by-user"
        );
        drop(manager);
        let _ = fs::remove_dir_all(root);
    }
}
