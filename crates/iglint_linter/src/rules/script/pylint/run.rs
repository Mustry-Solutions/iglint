//! Running pylint over many scripts and mapping its messages back.
//!
//! Every script's `source` is written to `iglint_<n>.py` in a fresh
//! temporary directory (removed afterwards, also on failure). The scripts
//! are split into shards of similar size, one subdirectory each, and one
//! pylint process per shard runs over its directory with `--recursive=y`
//! and `--jobs=1`, all at the same time. Their JSON reports are parsed and
//! merged; file names carry the script index. Scripts stored in JSON
//! strings get a final newline when they lack one: they are not files, so
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
    /// Messages disabled on the command line (`--disable`).
    pub disable: Vec<String>,
}

impl Config {
    /// Number of pylint processes: `jobs`, or one per CPU when it is 0.
    pub(crate) fn processes(&self) -> usize {
        match self.jobs {
            0 => std::thread::available_parallelism().map_or(1, usize::from),
            n => n as usize,
        }
    }

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
                disable: options.disable.clone(),
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

/// Scripts per worker below which another worker does not pay for its
/// set-up (each builds its own linter).
const MIN_SCRIPTS_PER_PROCESS: usize = 8;

/// Splits the scripts into at most `processes` shards of similar size
/// (longest first onto the lightest shard). Deterministic.
fn shards(scripts: &[Script<'_>], processes: usize) -> Vec<Vec<usize>> {
    let count = processes
        .min(scripts.len().div_ceil(MIN_SCRIPTS_PER_PROCESS))
        .max(1);
    let mut order: Vec<usize> = (0..scripts.len()).collect();
    order.sort_by_key(|&i| (std::cmp::Reverse(scripts[i].source.len()), i));
    let mut shards: Vec<(usize, Vec<usize>)> = vec![(0, Vec::new()); count];
    for i in order {
        // Per-file overhead counts too, not only the text.
        let weight = scripts[i].source.len() + 2_000;
        let lightest = (0..count).min_by_key(|&k| (shards[k].0, k)).unwrap_or(0);
        shards[lightest].0 += weight;
        shards[lightest].1.push(i);
    }
    shards
        .into_iter()
        .map(|(_, mut members)| {
            members.sort_unstable();
            members
        })
        .filter(|m| !m.is_empty())
        .collect()
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
    // Shards: one worker process lints each share of the scripts. Every
    // script is a module of its own, so the split does not change findings.
    // The forking driver needs a Python interpreter and `fork`; otherwise
    // the pylint command runs over all scripts in one directory.
    let python = if cfg!(unix) {
        forking(&config.command)
    } else {
        None
    };
    let shards = if python.is_some() {
        shards(scripts, config.processes())
    } else {
        vec![(0..scripts.len()).collect()]
    };
    let shard_dir = |k: usize| -> PathBuf {
        if python.is_some() {
            dir.0.join(format!("s{k}"))
        } else {
            dir.0.clone()
        }
    };
    let written: Result<(), std::io::Error> =
        shards.par_iter().enumerate().try_for_each(|(k, members)| {
            let shard_dir = shard_dir(k);
            if python.is_some() {
                std::fs::create_dir(&shard_dir)?;
            }
            for &i in members {
                let s = &scripts[i];
                let path = shard_dir.join(file_name(i));
                if s.embedded && !s.source.ends_with(['\n', '\r']) {
                    let mut text = String::with_capacity(s.source.len() + 1);
                    text.push_str(s.source);
                    text.push('\n');
                    std::fs::write(path, text)?;
                } else {
                    std::fs::write(path, s.source)?;
                }
            }
            Ok(())
        });
    if let Err(error) = written {
        fail(
            &mut outcome,
            format!("cannot write the scripts for pylint ({error})"),
        );
        return outcome;
    }
    let shard_dirs: Vec<String> = (0..shards.len()).map(|k| format!("s{k}")).collect();
    let raw = match run_forking(
        python.as_ref(),
        &config.disable,
        &dir.0,
        &rcfile,
        &shard_dirs,
    ) {
        Some(Ok(raw)) => raw,
        Some(Err(why)) => {
            fail(&mut outcome, why);
            return outcome;
        }
        None => match run_cli(config, &dir.0, &rcfile, config.processes()) {
            Ok(raw) => raw,
            Err(why) => {
                fail(&mut outcome, why);
                return outcome;
            }
        },
    };
    let (messages, notices) = map_messages(&raw, scripts);
    outcome.notices.extend(notices);
    outcome.messages = Some(messages);
    outcome
}

/// Runs pylint in-process in one Python interpreter that imports pylint
/// once and forks one worker per shard directory (POSIX only). Workers
/// inherit the imported modules instead of importing them again, which
/// roughly halves the time of pylint's own `--jobs`. Each worker writes its
/// JSON report next to its directory.
const FORKING_DRIVER: &str = r#"
import io, json, os, sys
try:
    from pylint.lint import Run
    from pylint.reporters import JSONReporter
except BaseException as e:
    sys.stderr.write("iglint driver: cannot import pylint: %s\n" % e)
    sys.exit(3)
if not hasattr(os, "fork"):
    sys.exit(3)
rc, disable, user, dirs = sys.argv[1], sys.argv[2], json.loads(sys.argv[3]), sys.argv[4:]
extra = ["--disable=" + disable] if disable else []
root = os.getcwd()
children = []
for d in dirs:
    pid = os.fork()
    if pid == 0:
        status = 0
        try:
            os.chdir(os.path.join(root, d))
            out = io.StringIO()
            Run(user + ["--score=no", "--persistent=no", "--recursive=y", "--rcfile=" + rc] + extra + ["--jobs=1", "."],
                reporter=JSONReporter(out), exit=False)
            with open(os.path.join(root, d + ".json"), "w") as f:
                f.write(out.getvalue() or "[]")
        except BaseException as e:
            sys.stderr.write("iglint driver: %s: %r\n" % (d, e))
            status = 1
        sys.stdout.flush()
        sys.stderr.flush()
        os._exit(status)
    children.append(pid)
failed = 0
for pid in children:
    _, code = os.waitpid(pid, 0)
    failed |= code != 0
sys.exit(1 if failed else 0)
"#;

/// How the forking driver runs the configured pylint command: the Python
/// interpreter (with its arguments) and the arguments for pylint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Forking {
    pub python: Vec<String>,
    pub pylint_args: Vec<String>,
}

/// pylint options that change what pylint prints or does instead of
/// linting: with them the command runs as configured, without the driver.
const CLI_ONLY_OPTIONS: &[&str] = &[
    "--output-format",
    "-f",
    "--output",
    "--from-stdin",
    "--help",
    "-h",
    "--long-help",
    "--version",
    "--help-msg",
    "--list-msgs",
    "--list-msgs-enabled",
    "--list-groups",
    "--list-conf-levels",
    "--list-extensions",
    "--full-documentation",
    "--generate-rcfile",
    "--generate-toml-config",
];

/// The forking driver's form of the configured pylint command:
///
/// * `python [python args] -m pylint [args]`;
/// * `pylint [args]` or `/path/to/pylint [args]`, a script whose `#!` line
///   names a Python interpreter (`#!/usr/bin/env python3` included; the
///   rest of a `#!` line is one argument, as the kernel passes it).
///
/// The pylint arguments are passed through unchanged, before iglint's
/// own, as `run_cli` does. `None` (run the command itself) for any other
/// form, and when an argument changes pylint's output or makes it do
/// something else than lint.
pub(crate) fn forking(command: &[String]) -> Option<Forking> {
    let (python, pylint_args) = if let Some(m) = command.iter().position(|a| a == "-m") {
        if command.get(m + 1).map(String::as_str) != Some("pylint") || m == 0 {
            return None;
        }
        (command[..m].to_vec(), command[m + 2..].to_vec())
    } else {
        (
            shebang_interpreter(command.first()?)?,
            command[1..].to_vec(),
        )
    };
    let cli_only = pylint_args.iter().any(|a| {
        let name = a.split_once('=').map_or(a.as_str(), |(n, _)| n);
        CLI_ONLY_OPTIONS.contains(&name)
            || (a.starts_with("-f") && !a.starts_with("--"))
            || (a.starts_with("-h") && !a.starts_with("--"))
    });
    (!cli_only).then_some(Forking {
        python,
        pylint_args,
    })
}

/// The interpreter of a `pylint` script named by `program` (found on
/// `PATH` when it has no directory part), from its `#!` line.
fn shebang_interpreter(program: &str) -> Option<Vec<String>> {
    let program = Path::new(program);
    let path = if program.components().count() > 1 {
        program.to_path_buf()
    } else {
        std::env::split_paths(&std::env::var_os("PATH")?)
            .map(|dir| dir.join(program))
            .find(|p| p.is_file())?
    };
    let head = std::fs::read(&path).ok()?;
    let first = head.split(|b| *b == b'\n').next()?;
    let line = std::str::from_utf8(first).ok()?.strip_prefix("#!")?.trim();
    let (interpreter, rest) = match line.split_once([' ', '\t']) {
        Some((i, rest)) => (i, rest.trim()),
        None => (line, ""),
    };
    let python = if interpreter.ends_with("/env") {
        // `#!/usr/bin/env python3`; `env` gets the rest as one word, so
        // anything after the program name is not what it seems.
        if rest.is_empty() || rest.contains(char::is_whitespace) {
            return None;
        }
        vec![rest.to_owned()]
    } else {
        let mut v = vec![interpreter.to_owned()];
        if !rest.is_empty() {
            v.push(rest.to_owned());
        }
        v
    };
    let name = Path::new(&python[0]).file_name()?.to_str()?;
    if !name.starts_with("python") {
        return None;
    }
    Some(python)
}

/// `None` when the forking driver is not available here (not POSIX, no
/// Python interpreter found, or pylint not importable by it); the caller
/// then runs the pylint command.
fn run_forking(
    forking: Option<&Forking>,
    disable: &[String],
    dir: &Path,
    rcfile: &Path,
    shards: &[String],
) -> Option<Result<Vec<RawMessage>, String>> {
    let forking = forking?;
    let python = &forking.python;
    let pylint_args = serde_json::to_string(&forking.pylint_args).ok()?;
    let output = Command::new(&python[0])
        .args(&python[1..])
        .arg("-c")
        .arg(FORKING_DRIVER)
        .arg(rcfile)
        .arg(disable.join(","))
        .arg(pylint_args)
        .args(shards)
        .current_dir(dir)
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    match output.status.code() {
        Some(0) => {}
        Some(3) => return None,
        code => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let detail: Vec<&str> = stderr
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .take(5)
                .collect();
            let status = code.map_or_else(|| "a signal".to_owned(), |c| format!("status {c}"));
            return Some(Err(format!(
                "pylint failed (exit {status}){}{}",
                if detail.is_empty() { "" } else { ": " },
                detail.join(" | ")
            )));
        }
    }
    let mut raw = Vec::new();
    for shard in shards {
        let text = match std::fs::read_to_string(dir.join(format!("{shard}.json"))) {
            Ok(text) => text,
            Err(error) => return Some(Err(format!("pylint wrote no report ({error})"))),
        };
        match parse_report(&text) {
            Ok(found) => raw.extend(found),
            Err(error) => return Some(Err(format!("unreadable pylint report ({error})"))),
        }
    }
    Some(Ok(raw))
}

/// Runs the pylint command over the whole directory with pylint's own
/// `--jobs`.
fn run_cli(
    config: &Config,
    dir: &Path,
    rcfile: &Path,
    processes: usize,
) -> Result<Vec<RawMessage>, String> {
    let shown = config.command.join(" ");
    let output = Command::new(&config.command[0])
        .args(&config.command[1..])
        .arg("--output-format=json")
        .arg("--score=no")
        .arg("--persistent=no")
        .arg("--recursive=y")
        .arg(format!("--rcfile={}", rcfile.display()))
        .args(
            (!config.disable.is_empty()).then(|| format!("--disable={}", config.disable.join(","))),
        )
        .arg(format!("--jobs={processes}"))
        .arg(".")
        .current_dir(dir)
        .stdin(std::process::Stdio::null())
        .output();
    let output = match output {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!(
                "`{shown}` was not found. Install pylint (`pip install pylint`) or set the command with `[rules.SCR900] pylint = \"python3 -m pylint\"`"
            ));
        }
        Err(error) => return Err(format!("`{shown}` could not be started ({error})")),
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let code = output.status.code();
    // Exit status bit 32 is a usage error (bad option or configuration).
    let usage_error = code.is_some_and(|c| c & 32 != 0);
    match parse_report(&stdout) {
        Ok(raw) if !usage_error => Ok(raw),
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
            Err(format!(
                "`{shown}` failed (exit {status}){}{}",
                if detail.is_empty() { "" } else { ": " },
                detail.join(" | ")
            ))
        }
    }
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
    let text = source
        .get(start..end)
        .unwrap_or_default()
        .trim_end_matches(['\n', '\r']);
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
