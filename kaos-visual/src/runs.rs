//! Process-backed Rebis run supervision for the visual Runs tab.
//!
//! The terminal and visual frontends launch the same `kaos rebis run` command.
//! This module owns only frontend-neutral lifecycle state and process control;
//! egui rendering remains in `lib.rs`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use kaos_core::retained::RetainedLog;

use crate::process::{Event, Job, Launch};

pub(crate) use kaos_core::run_model::{Authority, Lane, Lineage, Mode, Scope, State};

const MAX_RUN_HISTORY: usize = 128;

#[derive(Clone, Debug)]
pub(crate) struct Run {
    pub(crate) id: u64,
    pub(crate) source: String,
    pub(crate) input: String,
    pub(crate) scope: Scope,
    pub(crate) lane: Lane,
    pub(crate) mode: Mode,
    pub(crate) state: State,
    pub(crate) output: RetainedLog,
    pub(crate) expanded: bool,
    pub(crate) queued_at: Instant,
    pub(crate) started_at: Option<Instant>,
    pub(crate) elapsed: Option<Duration>,
    pub(crate) paused: bool,
    pub(crate) pause_reason: Option<String>,
    paused_at: Option<Instant>,
    paused_total: Duration,
    temp_source: Option<PathBuf>,
    /// Where this run's child looks for a value delivered to an `&` port. One
    /// file per run, named in the child's environment; see
    /// [`kaos_workspace::rebis_inlet`] for the one-delivery-per-file protocol.
    inlet_path: Option<PathBuf>,
    /// Where this run's child asks for runs beneath it. Drained as the run
    /// streams; see [`kaos_core::nest`].
    nest_path: Option<PathBuf>,
    /// What the reader is typing for the port this run is stopped on. Held per
    /// run so switching between two waiting runs cannot deliver one's answer to
    /// the other.
    pub(crate) input_draft: String,
    /// Where this run sits in the tree of runs. A run an agent opened from
    /// inside another is that run's child; the shared model in
    /// [`kaos_core::run_model`] answers how deep it is and what it belongs
    /// under, so both frontends draw the same tree.
    pub(crate) lineage: Lineage,
}

impl Run {
    pub(crate) const fn parallel(&self) -> bool {
        self.lane.parallel()
    }

    /// The `&` port this run is stopped waiting on, if it is waiting on one.
    ///
    /// A run pauses for several reasons — a transient provider error, a manual
    /// pause — and only this one can be answered, so the port name is what
    /// distinguishes "waiting for you" from "waiting for something else".
    pub(crate) fn awaiting_port(&self) -> Option<&str> {
        if !self.paused {
            return None;
        }
        kaos_workspace::rebis_inlet::awaited_port(self.pause_reason.as_deref()?)
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

    #[cfg(test)]
    pub(crate) fn test_fixture(
        id: u64,
        source: impl Into<String>,
        input: impl Into<String>,
        output: Vec<String>,
    ) -> Self {
        let now = Instant::now();
        Self {
            id,
            source: source.into(),
            input: input.into(),
            scope: Scope::Program,
            lane: Lane::Serial,
            mode: Mode::Dry,
            state: State::Complete,
            lineage: Lineage::root(),
            nest_path: None,
            output: output.into(),
            expanded: false,
            queued_at: now,
            started_at: Some(now),
            elapsed: Some(Duration::from_secs(1)),
            paused: false,
            pause_reason: None,
            paused_at: None,
            paused_total: Duration::ZERO,
            temp_source: None,
            inlet_path: None,
            input_draft: String::new(),
        }
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
            for path in [
                run.temp_source.as_ref(),
                run.inlet_path.as_ref(),
                run.nest_path.as_ref(),
            ]
            .into_iter()
            .flatten()
            {
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
        self.queue_under(source, lane_override, Lineage::root(), cwd)
    }

    /// Open every run the children have asked for, beneath whichever asked.
    ///
    /// Drained on each poll rather than at completion, so a program that opens
    /// a run early is not left invisible until it finishes. The request comes
    /// through a per-run sidecar rather than the output stream: these programs
    /// are often *about* writing Rebis, and a marker in their output would let
    /// a program open runs by describing one.
    fn open_requested_runs(&mut self, cwd: &Path) -> bool {
        let asked = self
            .runs
            .iter()
            .filter_map(|run| {
                let path = run.nest_path.as_ref()?;
                let requests = kaos_core::nest::drain(path);
                (!requests.is_empty()).then_some((run.id, run.lineage, requests))
            })
            .collect::<Vec<_>>();
        let mut opened = false;
        for (parent, lineage, requests) in asked {
            for request in requests {
                let child = self.queue_under(
                    request.source.clone(),
                    None,
                    kaos_core::run_model::Lineage::under(parent, lineage),
                    cwd,
                );
                if let Some(run) = self.runs.iter_mut().find(|run| run.id == parent) {
                    run.output.push(format!(
                        "nested      \u{2325} run #{child} \u{2014} {}",
                        kaos_core::nest::label(&request, parent)
                    ));
                }
                opened = true;
            }
        }
        opened
    }

    /// Queue a run at a given place in the tree.
    ///
    /// [`Self::submit`] is this with a root lineage. A run an agent asked for
    /// comes through here with the asking run's lineage extended by one.
    pub(crate) fn queue_under(
        &mut self,
        source: String,
        lane_override: Option<Lane>,
        lineage: Lineage,
        cwd: &Path,
    ) -> u64 {
        self.prune_history();
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
            output: RetainedLog::default(),
            expanded: true,
            queued_at: Instant::now(),
            started_at: None,
            elapsed: None,
            paused: false,
            pause_reason: None,
            paused_at: None,
            paused_total: Duration::ZERO,
            temp_source: None,
            inlet_path: None,
            nest_path: None,
            input_draft: String::new(),
            lineage,
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
        // The child stops at an `&` port and waits to be handed a value. That
        // needs three things in its environment: permission to stop at all, the
        // file to look in, and — since it owns a process group — the knowledge
        // that stopping means stopping its descendants too, so a model call in
        // flight halts with it.
        let inlet_path =
            std::env::temp_dir().join(format!("kaos-visual-inlet-{}-{id}", std::process::id()));
        let _ = std::fs::remove_file(&inlet_path);
        // The same shape for runs the child asks to open beneath itself. Only
        // given when nesting is still allowed at this depth, so the limit is
        // enforced by not handing over the channel rather than by trusting the
        // child to check.
        let nest_path = self.runs[index].lineage.may_nest().then(|| {
            let path =
                std::env::temp_dir().join(format!("kaos-visual-nest-{}-{id}", std::process::id()));
            let _ = std::fs::remove_file(&path);
            path
        });
        let launch = Launch {
            program: kaos_executable(),
            args,
            cwd: cwd.to_path_buf(),
            env: vec![
                (
                    "KAOS_MODEL".to_string(),
                    kaos_core::config::value("KAOS_MODEL").unwrap_or_else(|| "sim".to_string()),
                ),
                (kaos_agent::pause::ENABLE_ENV.to_string(), "1".to_string()),
                // The stance travels with the run, so a program that opens a
                // nested run or a chat beneath itself inherits it. Written
                // unconditionally, including "0": this editor may itself have
                // been launched with `KAOS_CHAOS=1`, and a dry or direct run
                // that merely inherited that would not be the run the user
                // chose in the modal.
                (
                    kaos_core::chaos::ENABLE_ENV.to_string(),
                    kaos_core::chaos::export(mode == Mode::Chaos).to_string(),
                ),
                (
                    kaos_agent::pause::PROCESS_GROUP_ENV.to_string(),
                    "1".to_string(),
                ),
                (
                    kaos_workspace::rebis_inlet::INLET_PATH_ENV.to_string(),
                    inlet_path.display().to_string(),
                ),
            ]
            .into_iter()
            .chain(nest_path.iter().map(|path| {
                (
                    kaos_core::nest::NEST_PATH_ENV.to_string(),
                    path.display().to_string(),
                )
            }))
            .collect(),
            stdin: Some(input),
            process_group: true,
        };

        match Job::spawn(id, launch) {
            Ok(job) => {
                self.jobs.push(job);
                let run = &mut self.runs[index];
                run.temp_source = Some(path);
                run.inlet_path = Some(inlet_path);
                run.nest_path = nest_path;
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
        let mut changed = self.open_requested_runs(cwd);
        let mut finished = Vec::new();
        for job in &self.jobs {
            for event in job.drain() {
                match event {
                    Event::Line(line) => {
                        if let Some(run) = self.runs.iter_mut().find(|run| run.id == job.id) {
                            // The child announces a stop on a private marker
                            // line, then suspends itself. Consume the marker —
                            // it is protocol, not output — and record the stop,
                            // so the pane can tell a run waiting for the reader
                            // from one that is simply working.
                            match kaos_agent::pause::marker_reason(&line) {
                                Some(reason) => {
                                    pause_clock(run, reason);
                                    let readable = match kaos_workspace::rebis_inlet::awaited_port(
                                        reason,
                                    ) {
                                        Some(port) => format!(
                                            "awaiting    ⌷ input on port {port} · type a value and send"
                                        ),
                                        None => format!("paused      ⏸ {reason}"),
                                    };
                                    run.output.push(readable);
                                }
                                None => run.output.push(line),
                            }
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
                } else if code == kaos_core::outcome::ABSTAINED_EXIT {
                    // A run that abstained FINISHED. Falling into the branch
                    // below would pause it as though the process had fallen
                    // over, and offer to retry work that was already done and
                    // already judged — reporting a sound refusal as a crash,
                    // which is the confusion the separate exit code exists to
                    // prevent.
                    run.state = State::Abstained;
                    run.output
                        .push("abstained   ⊘ the work is done and the gate refused it".to_string());
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
                if let Some(path) = run.inlet_path.take() {
                    let _ = std::fs::remove_file(path);
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

    /// Hand `value` to the `&` port run `id` is stopped on, and let it continue.
    ///
    /// Delivery is a file the child re-reads when it wakes, so the value must be
    /// in place BEFORE the resume signal — a child resumed with nothing waiting
    /// parks itself again, and the reader would see nothing happen. Returns
    /// whether the run was actually waiting and the value was delivered.
    pub(crate) fn deliver_input(&mut self, id: u64, value: &str) -> bool {
        let Some(index) = self.runs.iter().position(|run| run.id == id) else {
            return false;
        };
        let Some(port) = self.runs[index].awaiting_port().map(str::to_string) else {
            self.notice = Some(format!("run #{id} is not waiting for input"));
            return false;
        };
        let Some(path) = self.runs[index].inlet_path.clone() else {
            self.notice = Some(format!("run #{id} has no input seam"));
            return false;
        };
        if let Err(error) = kaos_workspace::rebis_inlet::deliver(&path, &port, value) {
            self.notice = Some(format!("could not deliver input to run #{id}: {error}"));
            return false;
        }
        // Only now wake it.
        if let Some(job) = self.jobs.iter().find(|job| job.id == id) {
            if !job.signal("-CONT") {
                self.notice = Some(format!("delivered, but could not resume run #{id}"));
                return false;
            }
        }
        let run = &mut self.runs[index];
        resume_clock(run);
        run.input_draft.clear();
        run.output
            .push(format!("received    ⌷ {port} ← {}", one_line(value)));
        true
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
        if let Some(path) = self.runs[index].inlet_path.take() {
            let _ = std::fs::remove_file(path);
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

    fn prune_history(&mut self) {
        while self.runs.len() >= MAX_RUN_HISTORY {
            let Some(index) = self.runs.iter().position(|run| run.state.terminal()) else {
                break;
            };
            let mut run = self.runs.remove(index);
            for path in [run.temp_source.take(), run.inlet_path.take()]
                .into_iter()
                .flatten()
            {
                let _ = std::fs::remove_file(path);
            }
        }
    }

    pub(crate) fn has_live_process(&self, id: u64) -> bool {
        self.jobs.iter().any(|job| job.id == id)
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

/// A delivered value collapsed to one readable line for the run's trace. The
/// value itself already reached the child; this is only the record of it.
fn one_line(value: &str) -> String {
    let flat = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > 60 {
        format!("{}…", flat.chars().take(60).collect::<String>())
    } else {
        flat
    }
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

    /// A run stopped on a port, as the child's own pause protocol would leave it.
    fn desk_awaiting_input(port: &str) -> (Desk, u64, PathBuf) {
        let mut desk = Desk::default();
        let id = desk.submit("\"work\"".to_string(), None, Path::new("."));
        let inlet = std::env::temp_dir().join(format!(
            "kaos-visual-inlet-test-{}-{id}-{port}",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&inlet);
        let run = desk.runs.iter_mut().find(|run| run.id == id).expect("run");
        run.state = State::Running;
        run.inlet_path = Some(inlet.clone());
        pause_clock(run, &kaos_workspace::rebis_inlet::await_reason(port));
        (desk, id, inlet)
    }

    /// The whole seam across a real process boundary, without a model.
    ///
    /// A live `kaos rebis run` refuses the simulated mind, so the child here is a
    /// stand-in that speaks the same protocol the real one does: announce the
    /// stop on the marker line, suspend itself, and on waking read the delivery
    /// file. What is under test is this side of it — the environment handed over,
    /// the marker parsed, the value written before the resume, and the signal
    /// that lands.
    #[test]
    fn a_real_child_stops_on_a_port_and_continues_with_the_value_delivered() {
        let inlet =
            std::env::temp_dir().join(format!("kaos-visual-inlet-e2e-{}", std::process::id()));
        let _ = std::fs::remove_file(&inlet);
        let reason = kaos_workspace::rebis_inlet::await_reason("topic");
        let script = format!(
            "printf '{}{reason}\\n'; kill -STOP $$; cat \"$KAOS_REBIS_INLET\"",
            kaos_agent::pause::PAUSED_MARKER
        );
        let job = Job::spawn(
            1,
            Launch {
                program: PathBuf::from("sh"),
                args: vec!["-c".to_string(), script],
                cwd: std::env::current_dir().expect("cwd"),
                env: vec![(
                    kaos_workspace::rebis_inlet::INLET_PATH_ENV.to_string(),
                    inlet.display().to_string(),
                )],
                stdin: None,
                process_group: true,
            },
        )
        .expect("spawn");

        // Wait for the stop to be announced.
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut seen = Vec::new();
        let mut port = None;
        while Instant::now() < deadline && port.is_none() {
            for event in job.drain() {
                if let Event::Line(line) = event {
                    if let Some(reason) = kaos_agent::pause::marker_reason(&line) {
                        port =
                            kaos_workspace::rebis_inlet::awaited_port(reason).map(str::to_string);
                    }
                    seen.push(line);
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            port.as_deref(),
            Some("topic"),
            "the child announced its wait: {seen:?}"
        );

        // The value goes in first; only then is the child woken.
        kaos_workspace::rebis_inlet::deliver(&inlet, "topic", "delivered value").expect("deliver");
        assert!(job.signal("-CONT"), "the child must accept the resume");

        let deadline = Instant::now() + Duration::from_secs(10);
        let mut echoed = false;
        while Instant::now() < deadline && !echoed {
            for event in job.drain() {
                if let Event::Line(line) = event {
                    echoed |= line.contains("delivered value");
                    seen.push(line);
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            echoed,
            "the woken child read the value from its inlet: {seen:?}"
        );
        let _ = std::fs::remove_file(&inlet);
    }

    #[test]
    fn the_pause_marker_becomes_a_waiting_run_and_never_reaches_the_output() {
        // The marker is the child's private protocol. Left in the stream it
        // would be shown to the reader as line noise AND the run would look busy
        // rather than waiting, so the input box would never appear.
        let mut desk = Desk::default();
        let id = desk.submit("\"work\"".to_string(), None, Path::new("."));
        let reason = kaos_workspace::rebis_inlet::await_reason("topic");
        let line = format!("{}{reason}", kaos_agent::pause::PAUSED_MARKER);

        let run = desk.runs.iter_mut().find(|run| run.id == id).expect("run");
        match kaos_agent::pause::marker_reason(&line) {
            Some(reason) => pause_clock(run, reason),
            None => panic!("the marker must be recognised"),
        }

        let run = &desk.runs[0];
        assert_eq!(run.awaiting_port(), Some("topic"));
        assert!(
            !run.output
                .iter()
                .any(|line| line.contains(kaos_agent::pause::PAUSED_MARKER)),
            "the marker is consumed, not shown: {:?}",
            run.output
        );
    }

    #[test]
    fn a_pause_that_is_not_an_await_offers_no_input_box() {
        // Runs also pause for transient provider errors and by hand. Those are
        // waiting for something other than a person and cannot be answered.
        let mut desk = Desk::default();
        let id = desk.submit("\"work\"".to_string(), None, Path::new("."));
        let run = desk.runs.iter_mut().find(|run| run.id == id).expect("run");
        pause_clock(run, "rate limited · retrying");
        assert_eq!(desk.runs[0].awaiting_port(), None);
    }

    #[test]
    fn delivering_input_writes_the_value_for_the_port_and_clears_the_wait() {
        // The value must be in the file BEFORE the child is resumed, or it wakes,
        // finds nothing, and parks again.
        let (mut desk, id, inlet) = desk_awaiting_input("topic");
        assert!(desk.deliver_input(id, "the Rebis input port"));

        // Written in the protocol's shape: the port names itself on line one.
        let written = std::fs::read_to_string(&inlet).expect("delivery file");
        assert_eq!(written, "topic\nthe Rebis input port");
        // And it is what the child would take for that port, and only that port.
        assert_eq!(
            kaos_workspace::rebis_inlet::take_input(&inlet, "other"),
            None,
            "another port must not consume this delivery"
        );
        assert_eq!(
            kaos_workspace::rebis_inlet::take_input(&inlet, "topic").as_deref(),
            Some("the Rebis input port")
        );

        let run = &desk.runs[0];
        assert!(!run.paused, "the run is no longer waiting");
        assert_eq!(run.awaiting_port(), None);
        assert!(
            run.input_draft.is_empty(),
            "the box is emptied after sending"
        );
        assert!(
            run.output.iter().any(|line| line.contains("topic ←")),
            "the delivery is recorded on the run: {:?}",
            run.output
        );
        let _ = std::fs::remove_file(&inlet);
    }

    #[test]
    fn a_run_that_is_not_waiting_refuses_a_delivery() {
        let mut desk = Desk::default();
        let id = desk.submit("\"work\"".to_string(), None, Path::new("."));
        assert!(!desk.deliver_input(id, "unasked for"));
        assert!(desk.notice.is_some(), "the refusal says why");
        assert!(!desk.deliver_input(id + 999, "no such run"));
    }

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

#[cfg(test)]
mod nesting_tests {
    use super::*;

    #[test]
    fn a_run_queued_under_another_is_drawn_beneath_it() {
        let mut runs = Desk::default();
        let cwd = std::path::Path::new(".");
        let parent = runs.submit("(\"root\")".into(), None, cwd);
        let parent_lineage = runs
            .runs
            .iter()
            .find(|run| run.id == parent)
            .map(|run| run.lineage)
            .unwrap();
        assert!(!parent_lineage.nested(), "a queued run starts at the root");

        let child = runs.queue_under(
            "(\"branch\")".into(),
            None,
            Lineage::under(parent, parent_lineage),
            cwd,
        );
        let child_lineage = runs
            .runs
            .iter()
            .find(|run| run.id == child)
            .map(|run| run.lineage)
            .unwrap();
        assert_eq!(child_lineage.parent, Some(parent));
        assert_eq!(child_lineage.depth, 1);

        // And the list draws the child under its parent, one step in — the same
        // ordering the terminal uses, from the same shared function.
        let shape = kaos_core::run_model::tree_order(
            &runs
                .runs
                .iter()
                .map(|run| (run.id, run.lineage.parent))
                .collect::<Vec<_>>(),
        );
        assert_eq!(shape, vec![(0, parent), (1, child)], "{shape:?}");
    }

    #[test]
    fn a_request_from_a_child_becomes_a_run_beneath_it() {
        let mut runs = Desk::default();
        let cwd = std::path::Path::new(".");
        let parent = runs.submit("(\"root\")".into(), None, cwd);

        // Stand in for the child process: give the run a sidecar and write to
        // it exactly as the conductor would from inside the child.
        let path =
            std::env::temp_dir().join(format!("kaos-nest-test-{}-{parent}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        if let Some(run) = runs.runs.iter_mut().find(|run| run.id == parent) {
            run.nest_path = Some(path.clone());
        }
        kaos_core::nest::request(&path, "answer the comments", "(\"reply\")").unwrap();
        kaos_core::nest::request(&path, "make the changes", "(\"edit\")").unwrap();

        assert!(runs.open_requested_runs(cwd), "the requests opened runs");
        let shape = kaos_core::run_model::tree_order(
            &runs
                .runs
                .iter()
                .map(|run| (run.id, run.lineage.parent))
                .collect::<Vec<_>>(),
        );
        assert_eq!(shape.len(), 3, "{shape:?}");
        assert_eq!(shape[0], (0, parent));
        assert!(shape[1..].iter().all(|(depth, _)| *depth == 1), "{shape:?}");
        // In the order they were asked for, carrying the programs as written.
        let sources = runs
            .runs
            .iter()
            .filter(|run| run.lineage.parent == Some(parent))
            .map(|run| run.source.clone())
            .collect::<Vec<_>>();
        assert_eq!(sources, vec!["(\"reply\")", "(\"edit\")"]);
        // Draining is once only: a second poll opens nothing further.
        assert!(!runs.open_requested_runs(cwd), "a request opened twice");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn nesting_stops_before_it_runs_away() {
        // Depth is bounded by the shared model, so neither frontend has to
        // remember to check it.
        let mut lineage = Lineage::root();
        for _ in 0..kaos_core::run_model::MAX_NESTING * 2 {
            if !lineage.may_nest() {
                break;
            }
            lineage = Lineage::under(1, lineage);
        }
        assert!(!lineage.may_nest());
        assert!(lineage.depth < kaos_core::run_model::MAX_NESTING);
    }
}
