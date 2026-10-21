//! The `iglint` command-line interface.
//!
//! [`run`] is the whole program; `main.rs` only connects it to the process.
//! Tests call [`run`] in-process with their own rule registry and analyzer.
//!
//! Exit codes: 0 no findings at or above the threshold, 1 findings (or, with
//! `--exit-non-zero-on-fix`, fixes applied), 2 usage, configuration or
//! internal error.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::PathBuf;

use iglint_linter::Registry;
use iglint_linter::engine::Analyzer;

pub mod args;
mod check;
pub mod commands;
pub mod ign_lint;
pub mod output;

/// Process exit status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ExitStatus {
    /// No findings at or above the threshold.
    Success = 0,
    /// Findings at or above the threshold.
    Failure = 1,
    /// Usage, configuration or internal error.
    Error = 2,
}

/// Where the program reads and writes.
pub struct Io<'a> {
    pub stdout: &'a mut dyn Write,
    pub stderr: &'a mut dyn Write,
    pub stdin: &'a mut dyn Read,
    /// Decides `--color auto`.
    pub stdout_is_terminal: bool,
}

/// What the program runs against.
#[derive(Clone)]
pub struct Environment {
    pub registry: &'static Registry,
    /// Overrides the per-file analysis (tests).
    pub analyzer: Option<Analyzer>,
    pub cwd: PathBuf,
    /// Environment variables that influence output (`NO_COLOR`,
    /// `CLICOLOR_FORCE`, `GITHUB_ACTIONS`...).
    pub vars: Vec<(String, String)>,
}

impl Environment {
    pub fn from_process() -> Environment {
        Environment {
            registry: &iglint_linter::rules::REGISTRY,
            analyzer: None,
            cwd: std::env::current_dir().unwrap_or_default(),
            vars: std::env::vars().collect(),
        }
    }

    pub fn var(&self, name: &str) -> Option<&str> {
        self.vars
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

/// Runs iglint with `args` (including the program name).
pub fn run(args: Vec<OsString>, environment: &Environment, io: &mut Io<'_>) -> ExitStatus {
    let program = args
        .first()
        .map(|a| {
            std::path::Path::new(a)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        })
        .unwrap_or_default();
    if program == "ign-lint" || program == "ignition-lint" {
        return ign_lint::run(args, environment, io);
    }
    let args = with_default_command(args);
    let cli = match <args::Cli as clap::Parser>::try_parse_from(&args) {
        Ok(cli) => cli,
        Err(error) => {
            let text = error.render().to_string();
            return if error.use_stderr() {
                let _ = io.stderr.write_all(text.as_bytes());
                ExitStatus::Error
            } else {
                let _ = io.stdout.write_all(text.as_bytes());
                ExitStatus::Success
            };
        }
    };
    match cli.command {
        args::Command::Check(check_args) => check::run(&check_args, environment, io, false),
        args::Command::Baseline(check_args) => check::run(&check_args, environment, io, true),
        args::Command::Rule(rule_args) => commands::rule(&rule_args, environment, io),
        args::Command::Rules(rules_args) => commands::rules(&rules_args, environment, io),
        args::Command::Migrate(migrate_args) => commands::migrate(&migrate_args, environment, io),
        args::Command::Server => match iglint_server::run() {
            Ok(()) => ExitStatus::Success,
            Err(error) => {
                let _ = writeln!(io.stderr, "error: {error}");
                ExitStatus::Error
            }
        },
        args::Command::IgnLint { args: rest } => {
            let mut ign_lint_args = vec![OsString::from("ign-lint")];
            ign_lint_args.extend(rest);
            ign_lint::run(ign_lint_args, environment, io)
        }
    }
}

/// Inserts `check` when the first argument is not a subcommand or a
/// top-level flag, so that `iglint [PATHS]...` means `iglint check`.
fn with_default_command(mut args: Vec<OsString>) -> Vec<OsString> {
    let first = args.get(1).map(|a| a.to_string_lossy().into_owned());
    let explicit = match first.as_deref() {
        None => false,
        Some("-h" | "--help" | "-V" | "--version") => true,
        Some(word) => args::SUBCOMMANDS.contains(&word),
    };
    if !explicit {
        let at = args.len().min(1);
        args.insert(at, OsString::from("check"));
    }
    args
}
