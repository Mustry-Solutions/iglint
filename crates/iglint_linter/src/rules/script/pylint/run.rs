//! Running pylint once over many scripts and mapping its messages back.
//!
//! Every script's `source` is written to `iglint_<n>.py` in a fresh
//! temporary directory (removed afterwards, also on failure), pylint runs
//! there over `.` with `--recursive=y` (one argument however many scripts
//! there are), and its JSON report is parsed. Scripts stored in JSON strings
//! get a final newline when they lack one: they are not files, so
//! `missing-final-newline` does not apply to them.
//!
//! pylint reports 1-based lines and 0-based columns. Columns of messages
//! about AST nodes (the ones with an end position) count UTF-8 bytes, like
//! CPython's `col_offset`; columns of token and line messages (no end
//! position) count characters. Lines end at `\n`, `\r\n` or a lone `\r`,
//! as in Python.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use iglint_text::{TextRange, TextSize};
use rayon::prelude::*;
use serde::Deserialize;

use super::Message;

/// ign-lint's `.ignition-pylintrc` with the messages its combined-file
/// header disabled (MIT License, see the file's header).
pub(crate) const BUNDLED_PYLINTRC: &str = include_str!("ignition-pylintrc");

/// How pylint is run.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct Config {
    /// Program and leading arguments.
    pub command: Vec<String>,
    /// `None`: the bundled configuration.
    pub rcfile: Option<PathBuf>,
    pub jobs: u32,
}

impl Config {
    /// The configuration of `options` for a project rooted at `root`, and a
    /// notice when the configured `pylintrc` does not exist.
    pub(crate) fn new(options: &super::PylintOptions, root: &Path) -> (Config, Option<String>) {
        let mut command = options.command.clone();
        if let Some(program) = command.first_mut() {
            let path = Path::new(program.as_str());
            if path.is_relative() && path.components().count() > 1 {
                *program = root.join(path).to_string_lossy().into_owned();
            }
        }
        let mut notice = None;
        let rcfile = options.pylintrc.as_ref().and_then(|rc| {
            let path = root.join(rc);
            if path.is_file() {
                Some(path)
            } else {
                notice = Some(format!(
                    "SCR900: pylintrc `{}` not found; using the bundled pylint configuration",
                    path.display()
                ));
                None
            }
        });
        (
            Config {
                command,
                rcfile,
                jobs: options.jobs,
            },
            notice,
        )
    }

    /// The configuration text, for the cache key.
    pub(crate) fn rcfile_text(&self) -> String {
        match &self.rcfile {
            None => BUNDLED_PYLINTRC.to_owned(),
            Some(path) => std::fs::read_to_string(path).unwrap_or_default(),
        }
    }
}

/// One script to lint.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Script<'s> {
    /// The script's `source`.
    pub source: &'s str,
    /// Stored in a JSON string (not a file of its own).
    pub embedded: bool,
}

/// What one pylint run produced.
#[derive(Debug, Default)]
pub(crate) struct Outcome {
    /// Messages per script, in the order of the scripts given; `None` when
    /// pylint could not run or failed.
    pub messages: Option<Vec<Vec<Message>>>,
    /// Problems to show once for the run.
    pub notices: Vec<String>,
}

/// A temporary directory removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> std::io::Result<TempDir> {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let base = std::env::temp_dir();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        for attempt in 0..100u32 {
            let dir = base.join(format!(
                "iglint-pylint-{}-{}-{nanos}-{attempt}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            match std::fs::create_dir(&dir) {
                Ok(()) => return Ok(TempDir(dir)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e),
            }
        }
        Err(std::io::Error::other("cannot create a temporary directory"))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn file_name(index: usize) -> String {
    format!("iglint_{index}.py")
}

/// Lints `scripts` with one pylint process.
pub(crate) fn run(config: &Config, scripts: &[Script<'_>]) -> Outcome {
    let mut outcome = Outcome::default();
    if scripts.is_empty() {
        outcome.messages = Some(Vec::new());
        return outcome;
    }
    let shown = config.command.join(" ");
    let fail = |outcome: &mut Outcome, why: String| {
        outcome.notices.push(format!(
            "SCR900: {why}; pylint findings are missing from this run"
        ));
    };
    let dir = match TempDir::new() {
        Ok(dir) => dir,
        Err(error) => {
            fail(
                &mut outcome,
                format!("cannot create a temporary directory for pylint ({error})"),
            );
            return outcome;
        }
    };
    let rcfile = match &config.rcfile {
        Some(path) => path.clone(),
        None => {
            let path = dir.0.join("ignition-pylintrc");
            if let Err(error) = std::fs::write(&path, BUNDLED_PYLINTRC) {
                fail(
                    &mut outcome,
                    format!("cannot write the pylint configuration ({error})"),
                );
                return outcome;
            }
            path
        }
    };
    let written: Result<(), std::io::Error> =
        scripts.par_iter().enumerate().try_for_each(|(i, s)| {
            let path = dir.0.join(file_name(i));
            if s.embedded && !s.source.ends_with(['\n', '\r']) {
                let mut text = String::with_capacity(s.source.len() + 1);
                text.push_str(s.source);
                text.push('\n');
                std::fs::write(path, text)
            } else {
                std::fs::write(path, s.source)
            }
        });
    if let Err(error) = written {
        fail(
            &mut outcome,
            format!("cannot write the scripts for pylint ({error})"),
        );
        return outcome;
    }
    let output = Command::new(&config.command[0])
        .args(&config.command[1..])
        .arg("--output-format=json")
        .arg("--score=no")
        .arg("--persistent=no")
        .arg("--recursive=y")
        .arg(format!("--rcfile={}", rcfile.display()))
        .arg(format!("--jobs={}", config.jobs))
        .arg(".")
        .current_dir(&dir.0)
        .stdin(std::process::Stdio::null())
        .output();
    let output = match output {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fail(
                &mut outcome,
                format!(
                    "`{shown}` was not found. Install pylint (`pip install pylint`) or set the command with `[rules.SCR900] pylint = \"python3 -m pylint\"`"
                ),
            );
            return outcome;
        }
        Err(error) => {
            fail(
                &mut outcome,
                format!("`{shown}` could not be started ({error})"),
            );
            return outcome;
        }
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let code = output.status.code();
    // Exit status bit 32 is a usage error (bad option or configuration).
    let usage_error = code.is_some_and(|c| c & 32 != 0);
    let parsed = parse_report(&stdout).ok();
    let raw = match parsed {
        Some(raw) if !usage_error => raw,
        _ => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let detail: Vec<&str> = stderr
                .lines()
                .chain(stdout.lines())
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .take(5)
                .collect();
            let status = code.map_or_else(|| "a signal".to_owned(), |c| format!("status {c}"));
            fail(
                &mut outcome,
                format!(
                    "`{shown}` failed (exit {status}){}{}",
                    if detail.is_empty() { "" } else { ": " },
                    detail.join(" | ")
                ),
            );
            return outcome;
        }
    };
    let (messages, notices) = map_messages(&raw, scripts);
    outcome.notices.extend(notices);
    outcome.messages = Some(messages);
    outcome
}

/// One entry of pylint's JSON report.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct RawMessage {
    #[serde(default)]
    pub line: Option<i64>,
    #[serde(default)]
    pub column: Option<i64>,
    #[serde(default, rename = "endLine")]
    pub end_line: Option<i64>,
    #[serde(default, rename = "endColumn")]
    pub end_column: Option<i64>,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub symbol: String,
    #[serde(default)]
    pub message: String,
    #[serde(default, rename = "message-id")]
    pub message_id: String,
}

/// Parses pylint's JSON report.
pub(crate) fn parse_report(text: &str) -> Result<Vec<RawMessage>, String> {
    serde_json::from_str(text.trim()).map_err(|e| e.to_string())
}

/// Assigns messages to scripts and maps their positions. Messages that
/// belong to no script (configuration problems) become notices.
pub(crate) fn map_messages(
    raw: &[RawMessage],
    scripts: &[Script<'_>],
) -> (Vec<Vec<Message>>, Vec<String>) {
    let mut out: Vec<Vec<Message>> = vec![Vec::new(); scripts.len()];
    let mut notices: Vec<String> = Vec::new();
    for m in raw {
        let index = Path::new(&m.path)
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_prefix("iglint_"))
            .and_then(|n| n.strip_suffix(".py"))
            .and_then(|n| n.parse::<usize>().ok())
            .filter(|i| *i < scripts.len());
        let Some(index) = index else {
            let notice = format!(
                "SCR900: pylint: {} ({}, {})",
                m.message, m.symbol, m.message_id
            );
            if !notices.contains(&notice) {
                notices.push(notice);
            }
            continue;
        };
        let source = scripts[index].source;
        let end = match (m.end_line, m.end_column) {
            (Some(l), Some(c)) => Some((l, c)),
            _ => None,
        };
        let range = source_range(source, m.line.unwrap_or(1), m.column.unwrap_or(0), end);
        out[index].push(Message {
            range,
            id: m.message_id.clone(),
            symbol: m.symbol.clone(),
            text: m.message.clone(),
        });
    }
    (out, notices)
}

/// Start offsets of the lines of `source` (Python line breaks).
fn line_starts(source: &str) -> Vec<usize> {
    let b = source.as_bytes();
    let mut starts = vec![0];
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\n' => starts.push(i + 1),
            b'\r' if b.get(i + 1) != Some(&b'\n') => starts.push(i + 1),
            _ => {}
        }
        i += 1;
    }
    starts
}

/// The text of 1-based line `line` without its line break, and its start.
fn line_text<'s>(source: &'s str, starts: &[usize], line: i64) -> (usize, &'s str) {
    let Ok(line) = usize::try_from(line.max(1)) else {
        return (0, "");
    };
    let Some(&start) = starts.get(line - 1) else {
        return (source.len(), "");
    };
    let end = starts.get(line).copied().unwrap_or(source.len());
    let text = source[start..end].trim_end_matches(['\n', '\r']);
    (start, text)
}

/// The offset of a 0-based `column` in line `line`, counting bytes or
/// characters.
fn offset(source: &str, starts: &[usize], line: i64, column: i64, bytes: bool) -> usize {
    let (start, text) = line_text(source, starts, line);
    let column = usize::try_from(column.max(0)).unwrap_or(0);
    let within = if bytes {
        let mut at = column.min(text.len());
        while !text.is_char_boundary(at) {
            at -= 1;
        }
        at
    } else {
        text.char_indices()
            .nth(column)
            .map_or(text.len(), |(i, _)| i)
    };
    start + within
}

/// The range in `source` of a message at `line`/`column`, ending at `end`
/// when pylint gives an end. Without an end the range is empty.
pub(crate) fn source_range(
    source: &str,
    line: i64,
    column: i64,
    end: Option<(i64, i64)>,
) -> TextRange {
    let starts = line_starts(source);
    let bytes = end.is_some();
    let start = offset(source, &starts, line, column, bytes);
    let end = end.map_or(start, |(l, c)| {
        offset(source, &starts, l, c, true).max(start)
    });
    TextRange::new(TextSize::from(start), TextSize::from(end))
}
