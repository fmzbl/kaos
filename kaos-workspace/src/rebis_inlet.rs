//! The input channel for `(& port body)` in hosted runs.
//!
//! A Rebis program that reaches an input port stops until the host delivers a
//! value. The TUI owns the delivery: when the user gives a selected, awaiting
//! run some input, it writes that value here; the paused child reads it on
//! resume and continues. The mechanism mirrors [`crate::rebis_supervisor`] — a
//! single per-run sidecar file, no process-control authority crossing into the
//! child.

use std::path::{Path, PathBuf};

/// Environment variable carrying one run's input-delivery file.
pub const INLET_PATH_ENV: &str = "KAOS_REBIS_INLET";

/// The prefix a paused child prints (as a pause reason) while awaiting input,
/// followed by the port name. The TUI matches this to know the run is waiting
/// for the user rather than for a transient provider condition to clear.
pub const AWAIT_PREFIX: &str = "awaiting input on port ";

/// The pause reason a child emits while blocked on `port`.
#[must_use]
pub fn await_reason(port: &str) -> String {
    format!("{AWAIT_PREFIX}{port}")
}

/// The port a pause reason is awaiting input on, if it is an await-input pause.
#[must_use]
pub fn awaited_port(reason: &str) -> Option<&str> {
    reason.strip_prefix(AWAIT_PREFIX).map(str::trim)
}

/// Resolve the delivery-file path once when a hosted child is constructed.
#[must_use]
pub fn path_from_env() -> Option<PathBuf> {
    std::env::var_os(INLET_PATH_ENV).map(PathBuf::from)
}

/// Take a value the host delivered for `port`, clearing it so the next port on
/// the same run blocks again. The file's first line names the port; the rest is
/// the value. A mismatched or missing file means nothing has been delivered.
#[must_use]
pub fn take_input(path: &Path, port: &str) -> Option<String> {
    let contents = std::fs::read_to_string(path).ok()?;
    let (head, value) = contents.split_once('\n')?;
    if head.trim() != port {
        return None;
    }
    // One delivery per file: remove it so a later `&` on the same run waits.
    let _ = std::fs::remove_file(path);
    Some(value.to_string())
}

/// Deliver `value` to the run's port (called by the host/TUI). The child picks
/// it up on its next read of the inlet file.
///
/// # Errors
///
/// Returns an I/O error when the delivery file cannot be written.
pub fn deliver(path: &Path, port: &str, value: &str) -> std::io::Result<()> {
    std::fs::write(path, format!("{port}\n{value}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_delivered_value_round_trips_and_clears() {
        let dir = std::env::temp_dir().join(format!("kaos-inlet-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("run.inlet");

        assert_eq!(take_input(&path, "input"), None, "nothing delivered yet");
        deliver(&path, "input", "from another agent").unwrap();
        assert_eq!(
            take_input(&path, "input").as_deref(),
            Some("from another agent")
        );
        // A second take blocks again — the delivery was consumed.
        assert_eq!(take_input(&path, "input"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_value_for_another_port_is_not_taken() {
        let dir = std::env::temp_dir().join(format!("kaos-inlet-b-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("run.inlet");
        deliver(&path, "other", "value").unwrap();
        assert_eq!(take_input(&path, "input"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn await_reason_round_trips_to_its_port() {
        let reason = await_reason("input");
        assert_eq!(awaited_port(&reason), Some("input"));
        assert_eq!(awaited_port("model timed out"), None);
    }
}
