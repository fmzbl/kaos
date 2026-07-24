//! Reusable streamed child-process supervision for visual capabilities.
//!
//! Rebis runs, chat turns, code work, and one-shot rites all need the same
//! mechanics: stdin transport, merged stdout/stderr events, process-group
//! ownership, cancellation, and Unix pause/resume. Keeping that here prevents
//! each UI surface from growing its own subtly different subprocess wrapper.

use std::io::{BufRead, BufReader, Write};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

pub(crate) enum Event {
    Line(String),
    Done(i32),
}

pub(crate) struct Launch {
    pub(crate) program: PathBuf,
    pub(crate) args: Vec<String>,
    pub(crate) cwd: PathBuf,
    pub(crate) env: Vec<(String, String)>,
    pub(crate) stdin: Option<String>,
    pub(crate) process_group: bool,
}

pub(crate) struct Job {
    pub(crate) id: u64,
    child: Arc<Mutex<Child>>,
    receiver: Receiver<Event>,
    process_group: bool,
}

impl Job {
    pub(crate) fn spawn(id: u64, launch: Launch) -> Result<Self, String> {
        let mut command = Command::new(&launch.program);
        command
            .args(&launch.args)
            .current_dir(&launch.cwd)
            .envs(launch.env)
            .stdin(if launch.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        if launch.process_group {
            command.process_group(0);
        }

        let mut child = command
            .spawn()
            .map_err(|error| format!("could not launch {}: {error}", launch.program.display()))?;
        if let (Some(input), Some(mut stdin)) = (launch.stdin, child.stdin.take()) {
            thread::spawn(move || {
                let _ = stdin.write_all(input.as_bytes());
            });
        }
        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            return Err("could not capture child stdout".to_string());
        };
        let Some(stderr) = child.stderr.take() else {
            let _ = child.kill();
            return Err("could not capture child stderr".to_string());
        };
        let (sender, receiver) = mpsc::channel();
        spawn_reader(stdout, sender.clone());
        spawn_reader(stderr, sender.clone());
        let child = Arc::new(Mutex::new(child));
        let waiter = Arc::clone(&child);
        thread::spawn(move || {
            let code = loop {
                let status = waiter
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .try_wait();
                match status {
                    Ok(Some(status)) => break status.code().unwrap_or(-1),
                    Ok(None) => thread::sleep(Duration::from_millis(30)),
                    Err(_) => break -1,
                }
            };
            let _ = sender.send(Event::Done(code));
        });
        Ok(Self {
            id,
            child,
            receiver,
            process_group: launch.process_group && cfg!(unix),
        })
    }

    pub(crate) fn drain(&self) -> Vec<Event> {
        let mut events = Vec::new();
        while let Ok(event) = self.receiver.try_recv() {
            let done = matches!(event, Event::Done(_));
            events.push(event);
            if done {
                break;
            }
        }
        events
    }

    pub(crate) fn signal(&self, signal: &str) -> bool {
        let child = self
            .child
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        signal_pid(child.id(), self.process_group, signal)
    }

    pub(crate) fn kill(&self) {
        let mut child = self
            .child
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.process_group {
            let _ = signal_pid(child.id(), true, "-KILL");
        } else {
            let _ = child.kill();
        }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        // A completed child ignores this; an abandoned visual tab cannot leave
        // model/tool descendants running invisibly.
        self.kill();
    }
}

fn spawn_reader(reader: impl std::io::Read + Send + 'static, sender: mpsc::Sender<Event>) {
    thread::spawn(move || {
        for line in BufReader::new(reader).lines() {
            match line {
                Ok(line) => {
                    let _ = sender.send(Event::Line(line));
                }
                Err(error) => {
                    let _ = sender.send(Event::Line(format!("stream error: {error}")));
                    break;
                }
            }
        }
    });
}

#[cfg(unix)]
fn signal_pid(pid: u32, group: bool, signal: &str) -> bool {
    let target = if group {
        format!("-{pid}")
    } else {
        pid.to_string()
    };
    Command::new("kill")
        .arg(signal)
        .arg("--")
        .arg(target)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(not(unix))]
fn signal_pid(_pid: u32, _group: bool, _signal: &str) -> bool {
    false
}
