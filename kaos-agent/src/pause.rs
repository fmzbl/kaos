//! Cooperative suspension for hosted runs.
//!
//! A model timeout is not necessarily a failed working. When the TUI opts a
//! child into this protocol, retryable model boundaries emit a private control
//! line and stop the child process. The parent retains the live process and can
//! continue it with SIGCONT, at which point the interrupted call is retried from
//! the still-live Rebis interpreter stack.

use std::io::Write;

/// Set only on child processes whose parent understands [`PAUSED_MARKER`].
pub const ENABLE_ENV: &str = "KAOS_PAUSE_ON_TRANSIENT";
/// The TUI launches each hosted job as a process-group leader, allowing a manual
/// pause or cancellation to include any model/command descendants.
pub const PROCESS_GROUP_ENV: &str = "KAOS_RUN_PROCESS_GROUP";
/// Private line protocol from a hosted child to the TUI.
pub const PAUSED_MARKER: &str = "\u{1e}KAOS_PAUSED\u{1f}";

#[must_use]
pub fn enabled() -> bool {
    kaos_core::config::enabled(ENABLE_ENV)
}

/// Errors that describe an exhausted allowance or a transient provider state,
/// rather than an invalid program/request. Authentication, malformed input,
/// refusals, and other deterministic failures deliberately remain failures.
#[must_use]
pub fn retryable_model_error(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    [
        "timed out",
        "timeout",
        "deadline",
        "time limit",
        "token budget",
        "cut before any answer",
        "rate limit",
        "too many requests",
        "http 408",
        "http 425",
        "http 429",
        "http 500",
        "http 502",
        "http 503",
        "http 504",
        "temporarily unavailable",
        "service unavailable",
        "overloaded",
        "connection reset",
        "connection refused",
        "broken pipe",
        "unexpected eof",
        "transport error",
        "network error",
    ]
    .iter()
    .any(|needle| error.contains(needle))
        || error.contains("exited with 124")
}

/// Decode a pause control line without exposing the marker in a transcript.
#[must_use]
pub fn marker_reason(line: &str) -> Option<&str> {
    line.strip_prefix(PAUSED_MARKER)
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
}

/// Stop the current hosted run at a continuation-safe boundary. Returns after
/// the parent sends SIGCONT. Outside a cooperative TUI child this is a no-op.
#[must_use]
pub fn current_run(reason: &str) -> bool {
    if !enabled() {
        return false;
    }
    let reason = reason.replace(['\n', '\r', '\t'], " ").trim().to_string();
    println!("{PAUSED_MARKER}{reason}");
    let _ = std::io::stdout().flush();

    #[cfg(unix)]
    {
        let pid = std::process::id();
        let target = if kaos_core::config::enabled(PROCESS_GROUP_ENV) {
            format!("-{pid}")
        } else {
            pid.to_string()
        };
        std::process::Command::new("kill")
            .arg("-STOP")
            .arg("--")
            .arg(target)
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    #[cfg(not(unix))]
    false
}

/// Pause only when an error is safe to retry. A successful return means the
/// run was resumed and the caller should retry the same model turn.
#[must_use]
pub fn retry_model_error(error: &str) -> bool {
    retryable_model_error(error) && current_run(error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_limits_and_transient_provider_errors_without_hiding_failures() {
        for error in [
            "charge timed out after 10s",
            "request deadline exceeded",
            "openrouter: HTTP 429",
            "model spent its whole token budget thinking",
            "service temporarily unavailable",
        ] {
            assert!(retryable_model_error(error), "{error}");
        }
        for error in [
            "OPENAI_API_KEY is not set",
            "openai: HTTP 400: invalid model",
            "provider refusal: safety",
            "bad json in response",
        ] {
            assert!(!retryable_model_error(error), "{error}");
        }
    }

    #[test]
    fn pause_marker_round_trips_a_reason() {
        let line = format!("{PAUSED_MARKER}model turn timed out after 30s");
        assert_eq!(marker_reason(&line), Some("model turn timed out after 30s"));
        assert_eq!(marker_reason("ordinary output"), None);
    }
}
