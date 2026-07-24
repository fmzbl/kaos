//! Typed visual projection of Kaos's non-Rebis terminal capabilities.
//!
//! Native visual surfaces (chat, source, runs, sigils, settings) are opened by
//! the editor. Everything that is intrinsically a Kaos rite—code, cast,
//! conclave, inspection, models, help, and credentials—uses this one streamed
//! task desk and the shared process supervisor.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use kaos_core::run_model::{Lane, State};

use crate::process::{Event, Job, Launch};
use crate::runs::kaos_executable;

/// Capabilities represented by native visual documents rather than a command
/// subprocess.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Surface {
    Mandala,
    Chat,
    Source,
    Runs,
    Sigils,
    Settings,
}

impl Surface {
    pub(crate) const ALL: [Self; 6] = [
        Self::Mandala,
        Self::Chat,
        Self::Source,
        Self::Runs,
        Self::Sigils,
        Self::Settings,
    ];

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Mandala => "mandala",
            Self::Chat => "chat & sessions",
            Self::Source => "Rebis source",
            Self::Runs => "runs",
            Self::Sigils => "sigils",
            Self::Settings => "settings",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Kind {
    Chat,
    Code,
    Cast,
    Conclave,
    Scry,
    Roster,
    Egregore,
    Models,
    AuthStatus,
    AuthSet,
    AuthForget,
    Help,
}

impl Kind {
    pub(crate) const ALL: [Self; 12] = [
        Self::Code,
        Self::Cast,
        Self::Conclave,
        Self::Scry,
        Self::Roster,
        Self::Egregore,
        Self::Models,
        Self::AuthStatus,
        Self::AuthSet,
        Self::AuthForget,
        Self::Help,
        Self::Chat,
    ];

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Chat => "chat intent",
            Self::Code => "code / work",
            Self::Cast => "cast",
            Self::Conclave => "conclave",
            Self::Scry => "scry",
            Self::Roster => "roster",
            Self::Egregore => "egregore",
            Self::Models => "models",
            Self::AuthStatus => "credential status",
            Self::AuthSet => "store credential",
            Self::AuthForget => "forget credential",
            Self::Help => "help",
        }
    }

    pub(crate) const fn needs_intent(self) -> bool {
        matches!(
            self,
            Self::Chat | Self::Code | Self::Cast | Self::Conclave | Self::Scry
        )
    }

    pub(crate) const fn may_use_tools(self) -> bool {
        matches!(self, Self::Chat | Self::Code)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ToolAccess {
    Ask,
    EditsOnly,
    Shell,
}

impl ToolAccess {
    const fn env(self) -> &'static str {
        match self {
            Self::Ask | Self::EditsOnly => "0",
            Self::Shell => "1",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Attachment {
    pub(crate) path: PathBuf,
    pub(crate) bytes: usize,
    text: String,
}

#[derive(Clone, Debug)]
pub(crate) struct Task {
    pub(crate) id: u64,
    pub(crate) kind: Kind,
    pub(crate) label: String,
    pub(crate) state: State,
    pub(crate) lane: Lane,
    pub(crate) output: Vec<String>,
    pub(crate) queued_at: Instant,
    pub(crate) started_at: Option<Instant>,
    pub(crate) elapsed: Option<Duration>,
    command: Vec<String>,
    stdin: Option<String>,
    env: Vec<(String, String)>,
    session: Option<String>,
    delivered: bool,
}

impl Task {
    pub(crate) fn elapsed(&self) -> Duration {
        self.elapsed
            .unwrap_or_else(|| self.started_at.unwrap_or(self.queued_at).elapsed())
    }

    pub(crate) fn timer(&self) -> String {
        let elapsed = self.elapsed();
        format!(
            "{:02}:{:02}.{:01}",
            elapsed.as_secs() / 60,
            elapsed.as_secs() % 60,
            elapsed.subsec_millis() / 100
        )
    }
}

pub(crate) struct Desk {
    pub(crate) tasks: Vec<Task>,
    jobs: Vec<Job>,
    next_id: u64,
    pub(crate) selected: Option<u64>,
    pub(crate) kind: Kind,
    pub(crate) intent: String,
    pub(crate) path: String,
    pub(crate) quorum: usize,
    pub(crate) gate: String,
    pub(crate) lane: Lane,
    pub(crate) tools: ToolAccess,
    pub(crate) notice: Option<String>,
    pub(crate) attachments: Vec<Attachment>,
    pub(crate) attachment_path: String,
    pub(crate) provider: String,
    pub(crate) secret: String,
}

impl Default for Desk {
    fn default() -> Self {
        Self {
            tasks: Vec::new(),
            jobs: Vec::new(),
            next_id: 1,
            selected: None,
            kind: Kind::Code,
            intent: String::new(),
            path: ".".to_string(),
            quorum: 1,
            gate: String::new(),
            lane: Lane::Serial,
            tools: ToolAccess::Ask,
            notice: None,
            attachments: Vec::new(),
            attachment_path: String::new(),
            provider: "openrouter".to_string(),
            secret: String::new(),
        }
    }
}

impl Drop for Desk {
    fn drop(&mut self) {
        for job in self.jobs.drain(..) {
            job.kill();
        }
    }
}

impl Desk {
    pub(crate) fn submit_current(&mut self, cwd: &Path) -> Option<u64> {
        let kind = self.kind;
        if kind.needs_intent() && self.intent.trim().is_empty() {
            self.notice = Some(format!("{} needs an intent", kind.label()));
            return None;
        }
        let context = self.with_attachments(&self.intent);
        let (command, stdin) = self.command(kind, context);
        let needs_permission = kind.may_use_tools() && self.tools == ToolAccess::Ask;
        Some(self.enqueue(
            kind,
            command,
            stdin,
            Vec::new(),
            None,
            needs_permission,
            cwd,
        ))
    }

    pub(crate) fn submit_chat(
        &mut self,
        intent: String,
        session: String,
        resume: bool,
        cwd: &Path,
    ) -> u64 {
        let input = self.with_attachments(&intent);
        let env = vec![
            ("KAOS_RAW_CHAT_TASK_STDIN".to_string(), "1".to_string()),
            ("KAOS_SESSION".to_string(), session.clone()),
            (
                "KAOS_RESUME".to_string(),
                if resume { "1" } else { "0" }.to_string(),
            ),
        ];
        // A chat runs immediately: it is a conversation, and whether the agent
        // may actually use tools is already carried to it by `KAOS_CLAUDE_YOLO`
        // (via the tool-access env, `0` under the default `Ask`). Gating the
        // whole task on a permission the Chat tab can't grant would just leave
        // every message parked in `AwaitingPermission`, so the bot never
        // answers.
        self.enqueue(
            Kind::Chat,
            vec!["code".to_string()],
            Some(input),
            env,
            Some(session),
            false,
            cwd,
        )
    }

    pub(crate) fn submit_auth(&mut self, kind: Kind) -> Option<u64> {
        let output = match kind {
            Kind::AuthStatus => kaos_agent::auth::status()
                .into_iter()
                .map(|(provider, variable, live, saved)| {
                    format!(
                        "{provider:<11} {:<5} {variable}{}",
                        if live { "set" } else { "unset" },
                        if saved { " (stored)" } else { "" }
                    )
                })
                .collect::<Vec<_>>(),
            Kind::AuthSet if !self.provider.trim().is_empty() && !self.secret.is_empty() => {
                let secret = std::mem::take(&mut self.secret);
                vec![
                    match kaos_agent::auth::store(self.provider.trim(), &secret) {
                        Ok((variable, path)) => {
                            format!("{variable} stored in {}", path.display())
                        }
                        Err(error) => format!("could not store credential: {error}"),
                    },
                ]
            }
            Kind::AuthForget if !self.provider.trim().is_empty() => {
                vec![match kaos_agent::auth::forget(self.provider.trim()) {
                    Ok(variable) => format!("forgot {variable}"),
                    Err(error) => format!("could not forget credential: {error}"),
                }]
            }
            Kind::AuthSet => {
                self.notice = Some("provider and key are required".to_string());
                return None;
            }
            Kind::AuthForget => {
                self.notice = Some("provider is required".to_string());
                return None;
            }
            _ => return None,
        };
        let id = self.next_id;
        self.next_id += 1;
        self.tasks.push(Task {
            id,
            kind,
            label: kind.label().to_string(),
            state: State::Complete,
            lane: Lane::Serial,
            output,
            queued_at: Instant::now(),
            started_at: Some(Instant::now()),
            elapsed: Some(Duration::ZERO),
            command: Vec::new(),
            stdin: None,
            env: Vec::new(),
            session: None,
            delivered: true,
        });
        self.selected = Some(id);
        Some(id)
    }

    #[allow(clippy::too_many_arguments)]
    fn enqueue(
        &mut self,
        kind: Kind,
        command: Vec<String>,
        stdin: Option<String>,
        mut env: Vec<(String, String)>,
        session: Option<String>,
        needs_permission: bool,
        cwd: &Path,
    ) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        env.push((
            "KAOS_MODEL".to_string(),
            kaos_core::config::value("KAOS_MODEL").unwrap_or_else(|| "sim".to_string()),
        ));
        env.push(("KAOS_CLAUDE_YOLO".to_string(), self.tools.env().to_string()));
        let state = if needs_permission {
            State::AwaitingPermission
        } else {
            State::Queued
        };
        let label = format!(
            "{} · {}",
            kind.label(),
            command
                .iter()
                .skip(1)
                .find(|part| !part.trim().is_empty())
                .map_or("", String::as_str)
        );
        self.tasks.push(Task {
            id,
            kind,
            label,
            state,
            lane: self.lane,
            output: Vec::new(),
            queued_at: Instant::now(),
            started_at: None,
            elapsed: None,
            command,
            stdin,
            env,
            session,
            delivered: false,
        });
        self.selected = Some(id);
        if needs_permission {
            self.notice = Some(format!("task #{id} needs edits-only or shell authority"));
        } else {
            self.start_ready(cwd);
        }
        id
    }

    fn command(&self, kind: Kind, intent: String) -> (Vec<String>, Option<String>) {
        match kind {
            Kind::Chat => (vec!["code".to_string()], Some(intent)),
            Kind::Code => {
                let mut spec = String::new();
                if !self.path.trim().is_empty() {
                    spec.push_str(self.path.trim());
                    spec.push(' ');
                }
                if self.quorum > 1 {
                    spec.push_str(&format!("x{} ", self.quorum));
                }
                spec.push_str(&intent);
                if !self.gate.trim().is_empty() {
                    spec.push_str(" -- ");
                    spec.push_str(self.gate.trim());
                }
                (vec!["code".to_string(), spec], None)
            }
            Kind::Cast => (vec!["cast".to_string(), intent], None),
            Kind::Conclave => (vec!["conclave".to_string(), intent], None),
            Kind::Scry => (vec!["scry".to_string(), intent], None),
            Kind::Roster => (vec!["roster".to_string()], None),
            Kind::Egregore => (vec!["egregore".to_string()], None),
            Kind::Models => (vec!["models".to_string()], None),
            Kind::AuthStatus => (vec!["auth".to_string()], None),
            Kind::AuthSet | Kind::AuthForget => (Vec::new(), None),
            Kind::Help => (vec!["help".to_string()], None),
        }
    }

    fn with_attachments(&self, intent: &str) -> String {
        if self.attachments.is_empty() {
            return intent.to_string();
        }
        let mut framed = String::from("Attached files for context:\n");
        for attachment in &self.attachments {
            framed.push_str(&format!(
                "\n===== {} =====\n{}\n",
                attachment.path.display(),
                attachment.text
            ));
        }
        framed.push_str("\n===== end of attachments =====\n\n");
        framed.push_str(intent);
        framed
    }

    pub(crate) fn add_attachment(&mut self, cwd: &Path) {
        let raw = self.attachment_path.trim();
        let path = if Path::new(raw).is_absolute() {
            PathBuf::from(raw)
        } else {
            cwd.join(raw)
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let bytes = text.len();
                self.attachments.retain(|item| item.path != path);
                self.attachments.push(Attachment {
                    path: path.clone(),
                    bytes,
                    text,
                });
                self.attachment_path.clear();
                self.notice = Some(format!("attached {} ({bytes} bytes)", path.display()));
            }
            Err(error) => {
                self.notice = Some(format!("could not attach {}: {error}", path.display()));
            }
        }
    }

    pub(crate) fn attach_text(&mut self, label: impl Into<PathBuf>, text: String) {
        let path = label.into();
        let bytes = text.len();
        self.attachments.retain(|item| item.path != path);
        self.attachments.push(Attachment { path, bytes, text });
    }

    pub(crate) fn grant_selected(&mut self, access: ToolAccess, cwd: &Path) {
        let Some(task) = self.selected_task_mut() else {
            return;
        };
        if task.state != State::AwaitingPermission {
            return;
        }
        task.state = State::Queued;
        if let Some(value) = task
            .env
            .iter_mut()
            .find(|(key, _)| key == "KAOS_CLAUDE_YOLO")
            .map(|(_, value)| value)
        {
            *value = access.env().to_string();
        }
        self.tools = access;
        self.start_ready(cwd);
    }

    fn start_ready(&mut self, cwd: &Path) {
        let serial_busy = self
            .tasks
            .iter()
            .any(|task| task.lane == Lane::Serial && task.state == State::Running);
        let mut serial_claimed = serial_busy;
        let ids = self
            .tasks
            .iter()
            .filter(|task| task.state == State::Queued)
            .filter_map(|task| match task.lane {
                Lane::Parallel => Some(task.id),
                Lane::Serial if !serial_claimed => {
                    serial_claimed = true;
                    Some(task.id)
                }
                Lane::Serial => None,
            })
            .collect::<Vec<_>>();
        for id in ids {
            self.start(id, cwd);
        }
    }

    fn start(&mut self, id: u64, cwd: &Path) {
        let Some(index) = self.tasks.iter().position(|task| task.id == id) else {
            return;
        };
        let task = &self.tasks[index];
        if task.command.is_empty() {
            self.tasks[index].state = State::Cancelled;
            self.tasks[index].output.push("empty command".to_string());
            return;
        }
        let launch = Launch {
            program: kaos_executable(),
            args: task.command.clone(),
            cwd: cwd.to_path_buf(),
            env: task.env.clone(),
            stdin: task.stdin.clone(),
            process_group: true,
        };
        match Job::spawn(id, launch) {
            Ok(job) => {
                self.jobs.push(job);
                let task = &mut self.tasks[index];
                task.state = State::Running;
                task.started_at = Some(Instant::now());
                task.output
                    .push(format!("started     {}", task.kind.label()));
            }
            Err(error) => {
                let task = &mut self.tasks[index];
                task.state = State::Cancelled;
                task.elapsed = Some(task.queued_at.elapsed());
                task.output.push(error.clone());
                self.notice = Some(error);
            }
        }
    }

    pub(crate) fn poll(&mut self, cwd: &Path) -> bool {
        let mut changed = false;
        let mut finished = Vec::new();
        for job in &self.jobs {
            for event in job.drain() {
                changed = true;
                match event {
                    Event::Line(line) => {
                        if let Some(task) = self.tasks.iter_mut().find(|task| task.id == job.id) {
                            task.output.push(line);
                        }
                    }
                    Event::Done(code) => {
                        finished.push((job.id, code));
                        break;
                    }
                }
            }
        }
        for (id, code) in finished {
            self.jobs.retain(|job| job.id != id);
            if let Some(task) = self.tasks.iter_mut().find(|task| task.id == id) {
                task.elapsed = Some(task.started_at.unwrap_or(task.queued_at).elapsed());
                task.state = if code == 0 {
                    State::Complete
                } else {
                    State::Cancelled
                };
                task.output.push(format!(
                    "{}     process exited {code}",
                    if code == 0 { "complete" } else { "cancelled" }
                ));
            }
        }
        if changed {
            self.start_ready(cwd);
        }
        changed
    }

    pub(crate) fn cancel_selected(&mut self, cwd: &Path) {
        let Some(id) = self.selected else {
            return;
        };
        if let Some(job) = self.jobs.iter().find(|job| job.id == id) {
            job.kill();
        }
        self.jobs.retain(|job| job.id != id);
        if let Some(task) = self.tasks.iter_mut().find(|task| task.id == id) {
            if !task.state.terminal() {
                task.state = State::Cancelled;
                task.elapsed = Some(task.queued_at.elapsed());
                task.output.push("cancelled   by user".to_string());
            }
        }
        self.start_ready(cwd);
    }

    pub(crate) fn remove_selected(&mut self) {
        let Some(id) = self.selected else {
            return;
        };
        let Some(index) = self.tasks.iter().position(|task| task.id == id) else {
            return;
        };
        if self.tasks[index].state == State::Running {
            self.notice = Some("cancel a running task before removing it".to_string());
            return;
        }
        self.tasks.remove(index);
        self.selected = self
            .tasks
            .get(index.min(self.tasks.len().saturating_sub(1)))
            .map(|task| task.id);
    }

    pub(crate) fn selected_task(&self) -> Option<&Task> {
        let id = self.selected?;
        self.tasks.iter().find(|task| task.id == id)
    }

    fn selected_task_mut(&mut self) -> Option<&mut Task> {
        let id = self.selected?;
        self.tasks.iter_mut().find(|task| task.id == id)
    }

    pub(crate) fn active_count(&self) -> usize {
        self.tasks
            .iter()
            .filter(|task| !task.state.terminal())
            .count()
    }

    pub(crate) fn session_active(&self, session: &str) -> bool {
        self.tasks
            .iter()
            .any(|task| task.session.as_deref() == Some(session) && !task.state.terminal())
    }

    /// Completed chat replies not yet transferred into their durable sessions.
    pub(crate) fn take_chat_replies(&mut self) -> Vec<(String, String)> {
        let mut replies = Vec::new();
        for task in &mut self.tasks {
            let Some(session) = task.session.clone() else {
                continue;
            };
            if task.delivered || !task.state.terminal() {
                continue;
            }
            task.delivered = true;
            let text = task
                .output
                .iter()
                .filter(|line| !line.starts_with("started") && !line.starts_with("complete"))
                .cloned()
                .collect::<Vec<_>>()
                .join("\n");
            replies.push((
                session,
                if text.trim().is_empty() {
                    "(the task ended without output)".to_string()
                } else {
                    text
                },
            ));
        }
        replies
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_metadata_covers_every_action() {
        for kind in Kind::ALL {
            assert!(!kind.label().is_empty());
        }
    }

    #[test]
    fn attachments_are_framed_once_for_every_model_rite() {
        let mut desk = Desk::default();
        desk.attachments.push(Attachment {
            path: PathBuf::from("facts.txt"),
            bytes: 4,
            text: "fact".to_string(),
        });
        let framed = desk.with_attachments("answer");
        assert_eq!(framed.matches("Attached files for context").count(), 1);
        assert!(framed.ends_with("answer"));
    }

    #[test]
    fn code_command_uses_typed_quorum_and_gate_fields() {
        let mut desk = Desk::default();
        desk.path = "repo".to_string();
        desk.quorum = 3;
        desk.gate = "cargo test".to_string();
        let (args, stdin) = desk.command(Kind::Code, "fix it".to_string());
        assert_eq!(args, ["code", "repo x3 fix it -- cargo test"]);
        assert!(stdin.is_none());
    }
}
