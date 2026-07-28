//! Bounded, cheap-to-clone line retention for streamed frontend output.

use std::ops::Deref;
use std::sync::Arc;
use std::{io, io::BufRead};

/// Default retained output budget for one run or task.
pub const DEFAULT_MAX_BYTES: usize = 2 * 1024 * 1024;
/// A second guard for workloads made of many tiny lines.
pub const DEFAULT_MAX_LINES: usize = 10_000;
/// One newline-free process write may not monopolize the whole log.
pub const DEFAULT_MAX_LINE_BYTES: usize = 64 * 1024;

const TRUNCATED_SUFFIX: &str = " … [line truncated]";

/// A bounded line log whose clones share their immutable backing allocation.
///
/// Frontends clone view models while painting. `Arc<Vec<_>>` makes that a
/// constant-size operation; the next incoming line uses copy-on-write. When a
/// hard limit is reached, one batch of old lines is released and a visible
/// omission marker is retained at the front.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RetainedLog {
    lines: Arc<Vec<String>>,
    bytes: usize,
    omitted_lines: usize,
    omitted_bytes: usize,
    notice_present: bool,
    max_bytes: usize,
    max_lines: usize,
    max_line_bytes: usize,
}

impl Default for RetainedLog {
    fn default() -> Self {
        Self::with_limits(DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES, DEFAULT_MAX_LINE_BYTES)
    }
}

impl RetainedLog {
    /// Construct a log with explicit budgets. Small values are raised enough
    /// to hold an omission marker and at least one useful line.
    #[must_use]
    pub fn with_limits(max_bytes: usize, max_lines: usize, max_line_bytes: usize) -> Self {
        let max_bytes = max_bytes.max(128);
        Self {
            lines: Arc::new(Vec::new()),
            bytes: 0,
            omitted_lines: 0,
            omitted_bytes: 0,
            notice_present: false,
            max_bytes,
            max_lines: max_lines.max(2),
            max_line_bytes: max_line_bytes.max(32).min(max_bytes / 2),
        }
    }

    pub fn push(&mut self, line: String) {
        let original_bytes = line.len();
        let line = truncate_utf8(line, self.max_line_bytes);
        let truncated_bytes = if original_bytes > self.max_line_bytes {
            original_bytes.saturating_sub(line.len().saturating_sub(TRUNCATED_SUFFIX.len()))
        } else {
            0
        };
        self.bytes += line.len();
        Arc::make_mut(&mut self.lines).push(line);

        let needs_notice = truncated_bytes > 0;
        self.omitted_bytes += truncated_bytes;
        if needs_notice || self.lines.len() > self.max_lines || self.bytes > self.max_bytes {
            self.rebalance();
        }
    }

    /// Release all retained text, including a shared old backing allocation.
    pub fn clear(&mut self) {
        self.lines = Arc::new(Vec::new());
        self.bytes = 0;
        self.omitted_lines = 0;
        self.omitted_bytes = 0;
        self.notice_present = false;
    }

    #[must_use]
    pub fn as_slice(&self) -> &[String] {
        self.lines.as_slice()
    }

    #[must_use]
    pub fn to_vec(&self) -> Vec<String> {
        self.lines.as_ref().clone()
    }

    #[must_use]
    pub fn retained_bytes(&self) -> usize {
        self.bytes
    }

    fn rebalance(&mut self) {
        if self.notice_present {
            let lines = Arc::make_mut(&mut self.lines);
            if !lines.is_empty() {
                self.bytes = self.bytes.saturating_sub(lines[0].len());
                lines.remove(0);
            }
        }

        // Drop to a low-water mark in one pass. This keeps ordinary `push`
        // constant-time instead of shifting the vector for every new line once
        // the log reaches its budget.
        let target_lines = (self.max_lines * 3 / 4).max(1);
        let target_bytes = self.max_bytes * 3 / 4;
        let reserve = omission_notice(self.omitted_lines, self.omitted_bytes).len();
        let mut remove = 0;
        let mut removed_bytes = 0;
        for line in self.lines.iter() {
            let visible_lines = self.lines.len().saturating_sub(remove) + 1;
            let visible_bytes = self
                .bytes
                .saturating_sub(removed_bytes)
                .saturating_add(reserve);
            if visible_lines <= target_lines && visible_bytes <= target_bytes {
                break;
            }
            remove += 1;
            removed_bytes += line.len();
        }
        if remove > 0 {
            Arc::make_mut(&mut self.lines).drain(0..remove);
            self.bytes = self.bytes.saturating_sub(removed_bytes);
            self.omitted_lines += remove;
            self.omitted_bytes += removed_bytes;
        }

        // The count changes the marker length slightly. Enforce hard bounds
        // after formatting it, dropping another oldest line only if necessary.
        loop {
            let notice = omission_notice(self.omitted_lines, self.omitted_bytes);
            let over_lines = self.lines.len() + 1 > self.max_lines;
            let over_bytes = self.bytes + notice.len() > self.max_bytes;
            if !(over_lines || over_bytes) || self.lines.is_empty() {
                self.bytes += notice.len();
                Arc::make_mut(&mut self.lines).insert(0, notice);
                self.notice_present = true;
                break;
            }
            let removed = Arc::make_mut(&mut self.lines).remove(0);
            self.bytes = self.bytes.saturating_sub(removed.len());
            self.omitted_lines += 1;
            self.omitted_bytes += removed.len();
        }
    }
}

impl Deref for RetainedLog {
    type Target = [String];

    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

impl<'a> IntoIterator for &'a RetainedLog {
    type Item = &'a String;
    type IntoIter = std::slice::Iter<'a, String>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl From<Vec<String>> for RetainedLog {
    fn from(lines: Vec<String>) -> Self {
        let mut log = Self::default();
        for line in lines {
            log.push(line);
        }
        log
    }
}

impl FromIterator<String> for RetainedLog {
    fn from_iter<T: IntoIterator<Item = String>>(iter: T) -> Self {
        let mut log = Self::default();
        for line in iter {
            log.push(line);
        }
        log
    }
}

impl<T: AsRef<str>> PartialEq<Vec<T>> for RetainedLog {
    fn eq(&self, other: &Vec<T>) -> bool {
        self.iter()
            .map(String::as_str)
            .eq(other.iter().map(AsRef::as_ref))
    }
}

impl PartialEq<RetainedLog> for Vec<String> {
    fn eq(&self, other: &RetainedLog) -> bool {
        self.as_slice() == other.as_slice()
    }
}

/// Read through one newline while retaining at most `max_bytes` from it.
///
/// Unlike [`BufRead::read_line`] and [`BufRead::read_until`], this continues
/// consuming an attacker-sized/no-newline record without growing the returned
/// allocation. EOF after partial data still returns that final line.
pub fn read_bounded_line(
    reader: &mut impl BufRead,
    max_bytes: usize,
) -> io::Result<Option<String>> {
    let max_bytes = max_bytes.max(TRUNCATED_SUFFIX.len());
    let content_limit = max_bytes.saturating_sub(TRUNCATED_SUFFIX.len());
    let mut retained = Vec::with_capacity(content_limit.min(8 * 1024));
    let mut saw_data = false;
    let mut truncated = false;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            break;
        }
        saw_data = true;
        let newline = available.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map_or(available.len(), |index| index + 1);
        let content = newline.map_or(available, |index| &available[..index]);
        let remaining = content_limit.saturating_sub(retained.len());
        let keep = content.len().min(remaining);
        retained.extend_from_slice(&content[..keep]);
        truncated |= keep < content.len();
        reader.consume(consumed);
        if newline.is_some() {
            break;
        }
    }
    if !saw_data {
        return Ok(None);
    }
    if retained.last() == Some(&b'\r') {
        retained.pop();
    }
    let mut line = String::from_utf8_lossy(&retained).into_owned();
    if line.len() > content_limit {
        let mut boundary = content_limit;
        while !line.is_char_boundary(boundary) {
            boundary -= 1;
        }
        line.truncate(boundary);
        truncated = true;
    }
    if truncated {
        line.push_str(TRUNCATED_SUFFIX);
    }
    Ok(Some(line))
}

fn truncate_utf8(mut line: String, max_bytes: usize) -> String {
    if line.len() <= max_bytes {
        return line;
    }
    let content_bytes = max_bytes.saturating_sub(TRUNCATED_SUFFIX.len());
    let mut boundary = content_bytes.min(line.len());
    while !line.is_char_boundary(boundary) {
        boundary -= 1;
    }
    line.truncate(boundary);
    line.push_str(TRUNCATED_SUFFIX);
    line
}

fn omission_notice(lines: usize, bytes: usize) -> String {
    format!("[... {lines} earlier lines / {bytes} bytes omitted ...]")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn releases_old_lines_in_bounded_batches() {
        let mut log = RetainedLog::with_limits(256, 8, 64);
        for index in 0..40 {
            log.push(format!("{index:02} {}", "x".repeat(24)));
        }

        assert!(log.len() <= 8);
        assert!(log.retained_bytes() <= 256);
        assert!(log[0].contains("earlier lines"));
        assert!(log.last().unwrap().starts_with("39 "));
    }

    #[test]
    fn caps_one_giant_utf8_line_without_splitting_a_character() {
        let mut log = RetainedLog::with_limits(512, 8, 64);
        log.push("🜁".repeat(100));

        assert!(log.retained_bytes() <= 512);
        assert!(log.iter().any(|line| line.ends_with(TRUNCATED_SUFFIX)));
        assert!(log[0].contains("bytes omitted"));
    }

    #[test]
    fn clones_share_until_the_next_write() {
        let mut log = RetainedLog::default();
        log.push("first".to_string());
        let snapshot = log.clone();
        assert!(Arc::ptr_eq(&log.lines, &snapshot.lines));

        log.push("second".to_string());
        assert!(!Arc::ptr_eq(&log.lines, &snapshot.lines));
        assert_eq!(snapshot.as_slice(), ["first"]);
    }

    #[test]
    fn clear_drops_retained_state_and_omission_counts() {
        let mut log = RetainedLog::with_limits(256, 4, 64);
        for _ in 0..20 {
            log.push("output".repeat(8));
        }
        log.clear();
        log.push("fresh".to_string());
        assert_eq!(log.as_slice(), ["fresh"]);
        assert_eq!(log.retained_bytes(), 5);
    }

    #[test]
    fn bounded_reader_consumes_a_huge_record_and_keeps_the_next_line() {
        let input = format!("{}\nnext\n", "x".repeat(10_000));
        let mut reader = io::BufReader::new(input.as_bytes());
        let first = read_bounded_line(&mut reader, 128).unwrap().unwrap();
        let second = read_bounded_line(&mut reader, 128).unwrap().unwrap();
        assert!(first.len() <= 128);
        assert!(first.ends_with(TRUNCATED_SUFFIX));
        assert_eq!(second, "next");
    }
}
