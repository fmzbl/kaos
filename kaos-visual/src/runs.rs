//! Process-backed Rebis run supervision for the visual Runs tab.
//!
//! The terminal and visual frontends launch the same `kaos rebis run` command.
//! This module owns only frontend-neutral lifecycle state and process control;
//! egui rendering remains in `lib.rs`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::process::{Event, Job, Launch};

pub(crate) use kaos_core::run_model::{Authority, Lane, Mode, Scope, State};

#[derive(Clone, Debug)]
pub(crate) struct Run {
    pub(crate) id: u64,
    pub(crate) source: String,
    pub(crate) input: String,
    pub(crate) scope: Scope,
    pub(crate) lane: Lane,
    pub(crate) mode: Mode,
    pub(crate) state: State,
    pub(crate) output: Vec<String>,
    pub(crate) expanded: bool,
    pub(crate) queued_at: Instant,
    pub(crate) started_at: Option<Instant>,
    pub(crate) elapsed: Option<Duration>,
    pub(crate) paused: bool,
    pub(crate) pause_reason: Option<String>,
    paused_at: Option<Instant>,
    paused_total: Duration,
    temp_source: Option<PathBuf>,
}

impl Run {
    pub(crate) const fn parallel(&self) -> bool {
        self.lane.parallel()
    }

    pub(crate) fn preview(&self) -> String {
        self.source
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("(empty)")
            .trim()
            .chars()
            .take(72)
            .collect()
    }

    pub(crate) fn elapsed(&self) -> Duration {
        if let Some(elapsed) = self.elapsed {
            return elapsed;
        }
        let Some(started) = self.started_at else {
            return self.queued_at.elapsed();
        };
        let end = self.paused_at.unwrap_or_else(Instant::now);
        end.saturating_duration_since(started)
            .saturating_sub(self.paused_total)
    }

    pub(crate) fn timer(&self) -> String {
        let duration = self.elapsed();
        let total = duration.as_secs();
        format!(
            "{:02}:{:02}.{:01}",
            total / 60,
            total % 60,
            duration.subsec_millis() / 100
        )
    }
}

/// Shared state shown by one singleton Runs tab.
pub(crate) struct Desk {
    pub(crate) runs: Vec<Run>,
    jobs: Vec<Job>,
    next_id: u64,
    pub(crate) selected: Option<u64>,
    pub(crate) input: String,
    pub(crate) draft_source: String,
    pub(crate) scope: Scope,
    pub(crate) mode: Mode,
    pub(crate) lane: Lane,
    pub(crate) authority: Authority,
    pub(crate) authority_remembered: bool,
    pub(crate) notice: Option<String>,
    pub(crate) output_path: String,
}

impl Default for Desk {
    fn default() -> Self {
        Self {
            runs: Vec::new(),
            jobs: Vec::new(),
            next_id: 1,
            selected: None,
            input: String::new(),
            draft_source: String::new(),
            scope: Scope::Program,
            // A visual gesture must not unexpectedly spend provider tokens or
            // edit files. Live mode is one explicit toggle away.
            mode: Mode::Dry,
            lane: Lane::Serial,
            authority: Authority::Ask,
            authority_remembered: false,
            notice: None,
            output_path: String::new(),
        }
    }
}

impl Drop for Desk {
    fn drop(&mut self) {
        for job in self.jobs.drain(..) {
            job.kill();
        }
        for run in &self.runs {
            if let Some(path) = &run.temp_source {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

impl Desk {
    pub(crate) fn submit(
        &mut self,
        source: String,
        lane_override: Option<Lane>,
        cwd: &Path,
    ) -> u64 {
        self.draft_source.clone_from(&source);
        let id = self.next_id;
        self.next_id += 1;
        let lane = lane_override.unwrap_or(self.lane);
        let needs_permission =
            self.mode.live() && !self.authority_remembered && self.authority == Authority::Ask;
        let state = if needs_permission {
            State::AwaitingPermission
        } else {
            State::Queued
        };
        self.runs.push(Run {
            id,
            source,
            input: self.input.clone(),
            scope: self.scope,
            lane,
            mode: self.mode,
            state,
            output: Vec::new(),
            expanded: true,
            queued_at: Instant::now(),
            started_at: None,
            elapsed: None,
            paused: false,
            pause_reason: None,
            paused_at: None,
            paused_total: Duration::ZERO,
            temp_source: None,
        });
        self.selected = Some(id);
        if needs_permission {
            self.notice = Some(format!(
                "run #{id} needs authority before a live model can work"
            ));
        } else {
            self.start_ready_in(cwd);
        }
        id
    }

    pub(crate) fn grant_selected(&mut self, authority: Authority, cwd: &Path) {
        let Some(id) = self.selected else {
            return;
        };
        let Some(run) = self.runs.iter_mut().find(|run| run.id == id) else {
            return;
        };
        if run.state != State::AwaitingPermission {
            return;
        }
        let remember = authority == Authority::Session;
        if remember {
            self.authority_remembered = true;
            self.authority = Authority::Session;
        } else {
            self.authority = Authority::Once;
        }
        run.output.push(if remember {
            "permission  granted · remembered for this visual session".to_string()
        } else {
            "permission  granted once".to_string()
        });
        run.state = State::Queued;
        self.start_ready_in(cwd);
    }

    pub(crate) fn deny_selected(&mut self) {
        let Some(run) = self.selected_run_mut() else {
            return;
        };
        if run.state == State::AwaitingPermission {
            run.state = State::Cancelled;
            run.elapsed = Some(run.queued_at.elapsed());
            run.output.push("permission  denied".to_string());
        }
    }

    /// Start all ready parallel runs and at most one serial run.
    pub(crate) fn start_ready_in(&mut self, cwd: &Path) {
        let serial_busy = self
            .runs
            .iter()
            .any(|run| !run.parallel() && run.state == State::Running);
        let mut serial_claimed = serial_busy;
        let ready = self
            .runs
            .iter()
            .filter(|run| run.state == State::Queued)
            .filter_map(|run| {
                if run.parallel() {
                    Some(run.id)
                } else if !serial_claimed {
                    serial_claimed = true;
                    Some(run.id)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        for id in ready {
            self.start(id, cwd);
        }
    }

    fn start(&mut self, id: u64, cwd: &Path) {
        let Some(index) = self.runs.iter().position(|run| run.id == id) else {
            return;
        };
        let (source, input, mode) = {
            let run = &self.runs[index];
            (run.source.clone(), run.input.clone(), run.mode)
        };
        let path =
            std::env::temp_dir().join(format!("kaos-visual-run-{}-{id}.rebis", std::process::id()));
        if let Err(error) = std::fs::write(&path, &source) {
            let run = &mut self.runs[index];
            run.state = State::Cancelled;
            run.output
                .push(format!("could not create source snapshot: {error}"));
            return;
        }

        let mut args = vec!["rebis".to_string(), "run".to_string()];
        match mode {
            Mode::Dry => {
                args.push("--dry".to_string());
            }
            Mode::Direct => {
                args.push("--allow-tools".to_string());
            }
            Mode::Chaos => {
                args.push("--allow-tools".to_string());
                args.push("--chaos".to_string());
            }
        }
        args.push(path.display().to_string());
        let launch = Launch {
            program: kaos_executable(),
            args,
            cwd: cwd.to_path_buf(),
            env: vec![(
                "KAOS_MODEL".to_string(),
                kaos_core::config::value("KAOS_MODEL").unwrap_or_else(|| "sim".to_string()),
            )],
            stdin: Some(input),
            process_group: true,
        };

        match Job::spawn(id, launch) {
            Ok(job) => {
                self.jobs.push(job);
                let run = &mut self.runs[index];
                run.temp_source = Some(path);
                run.state = State::Running;
                run.started_at.get_or_insert_with(Instant::now);
                run.elapsed = None;
                run.paused = false;
                run.pause_reason = None;
                run.output.push(match mode {
                    Mode::Dry => {
                        "mode        DRY · deterministic, no provider or tools".to_string()
                    }
                    Mode::Direct => "mode        DIRECT · one tool agent per prompt".to_string(),
                    Mode::Chaos => {
                        "mode        CHAOS · Kaos tool-agent expansion enabled".to_string()
                    }
                });
                self.notice = Some(format!("run #{id} started"));
            }
            Err(error) => {
                self.fail_start(index, path, &format!("could not launch kaos: {error}"));
            }
        }
    }

    fn fail_start(&mut self, index: usize, path: PathBuf, message: &str) {
        let _ = std::fs::remove_file(path);
        let run = &mut self.runs[index];
        run.state = State::Cancelled;
        run.elapsed = Some(run.queued_at.elapsed());
        run.output.push(message.to_string());
        self.notice = Some(message.to_string());
    }

    pub(crate) fn poll(&mut self, cwd: &Path) -> bool {
        let mut changed = false;
        let mut finished = Vec::new();
        for job in &self.jobs {
            for event in job.drain() {
                match event {
                    Event::Line(line) => {
                        if let Some(run) = self.runs.iter_mut().find(|run| run.id == job.id) {
                            run.output.push(line);
                            changed = true;
                        }
                    }
                    Event::Done(code) => {
                        finished.push((job.id, code));
                        changed = true;
                        break;
                    }
                }
            }
        }
        finished.sort_unstable();
        finished.dedup();
        for (id, code) in finished {
            self.jobs.retain(|job| job.id != id);
            if let Some(run) = self.runs.iter_mut().find(|run| run.id == id) {
                finish_clock(run);
                if code == 0 {
                    run.state = State::Complete;
                    run.output.push("complete    ✓ run finished".to_string());
                } else {
                    // Match terminal recovery semantics: a non-success exit is
                    // inspectable and resumable rather than silently becoming
                    // a terminal "failed" state.
                    run.state = State::Running;
                    run.paused = true;
                    run.pause_reason = Some(format!("process exited {code}"));
                    run.paused_at = Some(Instant::now());
                    run.output.push(format!(
                        "paused      process exited {code} · Resume retries from the captured source"
                    ));
                }
                if let Some(path) = run.temp_source.take() {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
        if changed {
            self.start_ready_in(cwd);
        }
        changed
    }

    pub(crate) fn toggle_pause_selected(&mut self, cwd: &Path) {
        let Some(id) = self.selected else {
            return;
        };
        let Some(run_index) = self.runs.iter().position(|run| run.id == id) else {
            return;
        };
        if self.runs[run_index].state != State::Running {
            self.notice = Some("only a running run can be paused".to_string());
            return;
        }
        let resume = self.runs[run_index].paused;
        if let Some(job) = self.jobs.iter().find(|job| job.id == id) {
            let signal = if resume { "-CONT" } else { "-STOP" };
            if !job.signal(signal) {
                self.notice = Some(format!("could not send {signal} to run #{id}"));
                return;
            }
            let run = &mut self.runs[run_index];
            if resume {
                resume_clock(run);
                run.output.push("resumed     ▶ run continues".to_string());
            } else {
                pause_clock(run, "paused manually");
                run.output.push("paused      ⏸ run suspended".to_string());
            }
        } else if resume {
            let run = &mut self.runs[run_index];
            resume_clock(run);
            run.output
                .push("resumed     ▶ retrying captured source".to_string());
            run.state = State::Queued;
            self.start_ready_in(cwd);
        } else {
            self.notice = Some("this run has no live process to pause".to_string());
        }
    }

    pub(crate) fn cancel_selected(&mut self, cwd: &Path) {
        let Some(id) = self.selected else {
            return;
        };
        if let Some(job) = self.jobs.iter().find(|job| job.id == id) {
            job.kill();
        }
        self.jobs.retain(|job| job.id != id);
        if let Some(run) = self.runs.iter_mut().find(|run| run.id == id) {
            if run.state.terminal() {
                return;
            }
            finish_clock(run);
            run.state = State::Cancelled;
            run.paused = false;
            run.pause_reason = None;
            run.output.push("cancelled   by user".to_string());
            if let Some(path) = run.temp_source.take() {
                let _ = std::fs::remove_file(path);
            }
        }
        self.start_ready_in(cwd);
    }

    pub(crate) fn cancel_all(&mut self, cwd: &Path) {
        let active = self
            .runs
            .iter()
            .filter(|run| !run.state.terminal())
            .map(|run| run.id)
            .collect::<Vec<_>>();
        for id in active {
            self.selected = Some(id);
            self.cancel_selected(cwd);
        }
    }

    pub(crate) fn remove_selected(&mut self) {
        let Some(id) = self.selected else {
            return;
        };
        let Some(index) = self.runs.iter().position(|run| run.id == id) else {
            return;
        };
        if self.runs[index].state == State::Running {
            self.notice = Some("cancel a running run before removing it".to_string());
            return;
        }
        if let Some(path) = self.runs[index].temp_source.take() {
            let _ = std::fs::remove_file(path);
        }
        self.runs.remove(index);
        self.selected = self
            .runs
            .get(index.min(self.runs.len().saturating_sub(1)))
            .map(|run| run.id);
    }

    pub(crate) fn selected_run(&self) -> Option<&Run> {
        let id = self.selected?;
        self.runs.iter().find(|run| run.id == id)
    }

    pub(crate) fn selected_run_mut(&mut self) -> Option<&mut Run> {
        let id = self.selected?;
        self.runs.iter_mut().find(|run| run.id == id)
    }

    pub(crate) fn has_active(&self) -> bool {
        self.runs.iter().any(|run| {
            matches!(
                run.state,
                State::AwaitingPermission | State::Queued | State::Running
            )
        })
    }

    pub(crate) fn active_count(&self) -> usize {
        self.runs
            .iter()
            .filter(|run| {
                matches!(
                    run.state,
                    State::AwaitingPermission | State::Queued | State::Running
                )
            })
            .count()
    }

    pub(crate) fn selected_output(&self) -> String {
        self.selected_run()
            .map(|run| run.output.join("\n"))
            .unwrap_or_default()
    }

    pub(crate) fn write_selected_output(&mut self, cwd: &Path) {
        let path = self.output_path.trim();
        if path.is_empty() {
            self.notice = Some("choose an output path first".to_string());
            return;
        }
        let target = if Path::new(path).is_absolute() {
            PathBuf::from(path)
        } else {
            cwd.join(path)
        };
        let output = self.selected_output();
        self.notice = Some(match std::fs::write(&target, output) {
            Ok(()) => format!("wrote {}", target.display()),
            Err(error) => format!("could not write {}: {error}", target.display()),
        });
    }
}

fn finish_clock(run: &mut Run) {
    run.elapsed = Some(run.elapsed());
    run.paused_at = None;
}

fn pause_clock(run: &mut Run, reason: &str) {
    if run.paused {
        return;
    }
    run.paused = true;
    run.pause_reason = Some(reason.to_string());
    run.paused_at = Some(Instant::now());
}

fn resume_clock(run: &mut Run) {
    if let Some(paused_at) = run.paused_at.take() {
        run.paused_total += paused_at.elapsed();
    }
    run.paused = false;
    run.pause_reason = None;
    run.elapsed = None;
}

/// Find the canonical Kaos command whether visual mode is linked into `kaos`
/// or launched from the standalone `kaos-visual` binary.
pub(crate) fn kaos_executable() -> PathBuf {
    if let Some(path) = std::env::var_os("KAOS_BIN").map(PathBuf::from) {
        return path;
    }
    if let Ok(current) = std::env::current_exe() {
        if current
            .file_stem()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name == "kaos")
        {
            return current;
        }
        if let Some(sibling) = current.parent().map(|parent| parent.join("kaos")) {
            if sibling.is_file() {
                return sibling;
            }
        }
    }
    PathBuf::from("kaos")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serial_runs_queue_while_parallel_runs_are_independent() {
        let mut desk = Desk::default();
        desk.mode = Mode::Direct;
        desk.authority_remembered = false;
        let first = desk.submit("\"one\"".to_string(), Some(Lane::Serial), Path::new("."));
        let second = desk.submit("\"two\"".to_string(), Some(Lane::Serial), Path::new("."));
        let parallel = desk.submit(
            "\"three\"".to_string(),
            Some(Lane::Parallel),
            Path::new("."),
        );
        assert_eq!(desk.runs[0].id, first);
        assert_eq!(desk.runs[0].state, State::AwaitingPermission);
        assert_eq!(desk.runs[1].id, second);
        assert_eq!(desk.runs[2].id, parallel);
    }

    #[test]
    fn denial_is_terminal_and_removable() {
        let mut desk = Desk::default();
        desk.mode = Mode::Direct;
        let id = desk.submit("\"work\"".to_string(), None, Path::new("."));
        desk.deny_selected();
        assert_eq!(desk.selected, Some(id));
        assert_eq!(desk.selected_run().unwrap().state, State::Cancelled);
        desk.remove_selected();
        assert!(desk.runs.is_empty());
    }

    #[test]
    fn elapsed_timer_has_minutes_seconds_and_tenths() {
        let mut desk = Desk::default();
        desk.mode = Mode::Direct;
        let id = desk.submit("\"work\"".to_string(), None, Path::new("."));
        let run = desk.runs.iter().find(|run| run.id == id).unwrap();
        assert_eq!(run.timer().chars().filter(|c| *c == ':').count(), 1);
        assert_eq!(run.timer().chars().filter(|c| *c == '.').count(), 1);
    }
}
