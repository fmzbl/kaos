//! Reusable streamed child-process supervision for visual capabilities.
//!
//! Rebis runs, chat turns, code work, and one-shot rites all need the same
//! mechanics: stdin transport, merged stdout/stderr events, process-group
//! ownership, cancellation, and Unix pause/resume. Keeping that here prevents
//! each UI surface from growing its own subtly different subprocess wrapper.

use std::io::{BufReader, Write};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, SyncSender};
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
        let (sender, receiver) = mpsc::sync_channel(EVENT_QUEUE_CAPACITY);
        // One token per reader, so the waiter can tell when both have finished
        // without joining them — see the settle loop below for why a join is
        // the wrong tool here.
        let reading = Arc::new(());
        spawn_reader(stdout, sender.clone(), Arc::clone(&reading));
        spawn_reader(stderr, sender.clone(), Arc::clone(&reading));
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
            // Let the pipes settle before announcing the exit. Lines and the
            // exit share one channel across three threads, and `drain` stops at
            // `Done` — so a `Done` that overtakes a still-flushing reader
            // silently truncates the transcript.
            //
            // The wait is bounded and deliberately not a `join`. A killed child
            // can leave a descendant holding the write end of these pipes open
            // (that is exactly what `process_group` exists to prevent), and a
            // join would then block until that stranger exited — a run row that
            // never retires. Settling covers every realistic burst; the deadline
            // covers the pathological case, at the cost of the tail of a
            // transcript nobody is still reading.
            let settle = std::time::Instant::now() + SETTLE;
            while Arc::strong_count(&reading) > 1 && std::time::Instant::now() < settle {
                thread::sleep(Duration::from_millis(5));
            }
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

/// How long the waiter lets the pipes settle after the child exits before it
/// announces `Done`. Long enough for any burst a reader is mid-flush on, short
/// enough that a stranded descendant cannot wedge a run row open.
const SETTLE: Duration = Duration::from_millis(750);
/// Backpressure budget per child. Together with the bounded-line reader this
/// puts a hard ceiling on output waiting between a subprocess and the UI.
const EVENT_QUEUE_CAPACITY: usize = 64;

/// Stream one pipe into the shared channel, a line at a time.
///
/// `reading` is a liveness token: this thread holds it until the pipe reaches
/// EOF, and dropping it is what tells the waiter the stream is finished.
fn spawn_reader(
    reader: impl std::io::Read + Send + 'static,
    sender: SyncSender<Event>,
    reading: Arc<()>,
) {
    thread::spawn(move || {
        let _reading = reading;
        let mut reader = BufReader::new(reader);
        loop {
            match kaos_core::retained::read_bounded_line(
                &mut reader,
                kaos_core::retained::DEFAULT_MAX_LINE_BYTES,
            ) {
                Ok(Some(line)) => {
                    // The child writes for a terminal: colours, hyperlinks,
                    // carriage returns. Nothing in this editor renders an escape
                    // sequence, so an unstripped line reaches the reader as the
                    // literal digits of its own colour codes. Cleaned once here,
                    // at the only place child output enters, rather than at each
                    // pane that shows it.
                    if sender
                        .send(Event::Line(kaos_core::chat::strip_terminal_escapes(&line)))
                        .is_err()
                    {
                        break;
                    }
                }
                Ok(None) => break,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// A launch of `sh -c` in the current directory, with nothing inherited.
    fn shell(script: &str) -> Launch {
        Launch {
            program: PathBuf::from("sh"),
            args: vec!["-c".to_string(), script.to_string()],
            cwd: std::env::current_dir().expect("cwd"),
            env: Vec::new(),
            stdin: None,
            process_group: false,
        }
    }

    /// Collect events until `Done` or the deadline. `drain` is deliberately
    /// non-blocking, so every caller — the UI included — has to poll; this is
    /// that polling loop, with a bound so a hung child fails the test rather
    /// than the suite.
    fn collect(job: &Job, within: Duration) -> (Vec<String>, Option<i32>) {
        let deadline = Instant::now() + within;
        let (mut lines, mut code) = (Vec::new(), None);
        while Instant::now() < deadline {
            for event in job.drain() {
                match event {
                    Event::Line(line) => lines.push(line),
                    Event::Done(status) => code = Some(status),
                }
            }
            if code.is_some() {
                return (lines, code);
            }
            thread::sleep(Duration::from_millis(10));
        }
        (lines, code)
    }

    #[test]
    fn a_childs_colour_codes_never_reach_the_editor() {
        // The child writes for a terminal. This editor has no ANSI renderer, so
        // an unstripped line arrives as the literal digits of its own colours —
        // which is what was appearing in run output and chat replies.
        let job = Job::spawn(
            1,
            shell("printf '  \\033[48;2;250;250;250;38;2;74;124;240m- 1964-1973\\033[0m\\n'"),
        )
        .expect("spawn");
        let (lines, code) = collect(&job, Duration::from_secs(5));

        assert_eq!(code, Some(0));
        assert_eq!(
            lines,
            vec!["  - 1964-1973".to_string()],
            "the text survives with its indentation; the escapes do not"
        );
    }

    #[test]
    fn both_streams_are_merged_and_the_exit_code_is_reported() {
        // stdout and stderr are one stream to the caller by design — a rite's
        // diagnostics belong in the transcript beside its output, in order.
        let job = Job::spawn(1, shell("echo out; echo err 1>&2; exit 3")).expect("spawn");
        let (lines, code) = collect(&job, Duration::from_secs(10));
        assert_eq!(code, Some(3), "the child's exit code reaches the caller");
        assert!(lines.contains(&"out".to_string()), "stdout line: {lines:?}");
        assert!(lines.contains(&"err".to_string()), "stderr line: {lines:?}");
    }

    /// `Done` means the job has said everything it will ever say.
    ///
    /// Lines and the exit reach the caller on one channel from three threads,
    /// and `drain` stops at `Done` — so a `Done` racing a still-flushing reader
    /// truncates the transcript. A child that writes a burst and exits at once
    /// is exactly the shape that loses output, so this asserts every line is
    /// present when the exit arrives.
    #[test]
    fn no_output_is_lost_when_a_child_writes_a_burst_and_exits_immediately() {
        const BURST: usize = 200;
        let job = Job::spawn(
            7,
            shell(&format!(
                "i=0; while [ $i -lt {BURST} ]; do echo out-$i; echo err-$i 1>&2; i=$((i+1)); done"
            )),
        )
        .expect("spawn");
        let (lines, code) = collect(&job, Duration::from_secs(20));
        assert_eq!(code, Some(0));
        for i in 0..BURST {
            assert!(
                lines.contains(&format!("out-{i}")),
                "stdout line out-{i} was dropped before Done ({} of {BURST} arrived)",
                lines.len()
            );
            assert!(
                lines.contains(&format!("err-{i}")),
                "stderr line err-{i} was dropped before Done ({} of {BURST} arrived)",
                lines.len()
            );
        }
    }

    #[test]
    fn stdin_is_delivered_to_the_child() {
        let mut launch = shell("cat");
        launch.stdin = Some("a program on stdin\n".to_string());
        let job = Job::spawn(2, launch).expect("spawn");
        let (lines, code) = collect(&job, Duration::from_secs(10));
        assert_eq!(code, Some(0));
        assert_eq!(lines, vec!["a program on stdin".to_string()]);
    }

    #[test]
    fn one_newline_free_record_is_capped_before_it_reaches_the_event_queue() {
        let mut launch = shell("cat");
        launch.stdin = Some(format!(
            "{}\nnext\n",
            "x".repeat(kaos_core::retained::DEFAULT_MAX_LINE_BYTES * 4)
        ));
        let job = Job::spawn(6, launch).expect("spawn");
        let (lines, code) = collect(&job, Duration::from_secs(10));

        assert_eq!(code, Some(0));
        assert!(lines[0].len() <= kaos_core::retained::DEFAULT_MAX_LINE_BYTES);
        assert!(lines[0].ends_with("[line truncated]"));
        assert_eq!(lines[1], "next");
    }

    #[test]
    fn a_missing_program_is_an_error_and_never_a_panic() {
        let mut launch = shell("true");
        launch.program = PathBuf::from("kaos-no-such-executable");
        let Err(error) = Job::spawn(3, launch) else {
            panic!("a missing program must not launch");
        };
        assert!(
            error.contains("could not launch"),
            "the message names the failure: {error}"
        );
    }

    #[test]
    fn drain_stops_at_done_and_leaves_nothing_behind() {
        let job = Job::spawn(4, shell("echo one; echo two")).expect("spawn");
        let (lines, code) = collect(&job, Duration::from_secs(10));
        assert_eq!(code, Some(0));
        assert_eq!(lines, vec!["one".to_string(), "two".to_string()]);
        // Everything was retired: a drain after Done yields nothing further.
        assert!(job.drain().is_empty());
    }

    #[test]
    fn killing_a_job_ends_a_child_that_would_otherwise_run_on() {
        let job = Job::spawn(5, shell("sleep 30")).expect("spawn");
        job.kill();
        let (_, code) = collect(&job, Duration::from_secs(10));
        assert!(
            code.is_some(),
            "a killed child still reports Done, so the UI can retire the row"
        );
    }

    /// The claim in `Drop`'s comment, tested: an abandoned tab must not leave
    /// model or tool descendants running invisibly.
    ///
    /// Without `process_group`, killing `sh` leaves the `sleep` it started
    /// orphaned and running. With it, the whole group goes. This is the
    /// difference between closing a tab and leaking a process tree.
    #[cfg(unix)]
    #[test]
    fn a_process_group_job_takes_its_descendants_with_it() {
        let marker = std::env::temp_dir().join(format!(
            "kaos-visual-group-{}-{}",
            std::process::id(),
            Instant::now().elapsed().as_nanos()
        ));
        let _ = std::fs::remove_file(&marker);
        let script = format!(
            "sh -c 'sleep 2; : > {}' & echo started; wait",
            marker.display()
        );
        let mut launch = shell(&script);
        launch.process_group = true;
        let job = Job::spawn(6, launch).expect("spawn");

        // Wait until the grandchild is definitely running, then drop the job.
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut started = false;
        while Instant::now() < deadline && !started {
            started = job
                .drain()
                .iter()
                .any(|event| matches!(event, Event::Line(line) if line == "started"));
            thread::sleep(Duration::from_millis(10));
        }
        assert!(started, "the child never reported that it had started");
        drop(job);

        // The grandchild would touch the marker at +2s if it survived.
        thread::sleep(Duration::from_millis(3500));
        assert!(
            !marker.exists(),
            "a descendant outlived its job and touched {}",
            marker.display()
        );
        let _ = std::fs::remove_file(&marker);
    }
}
