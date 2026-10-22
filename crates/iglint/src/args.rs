//! Command-line arguments.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::output::OutputFormat;

#[derive(Debug, Parser)]
#[command(
    name = "iglint",
    version,
    about = "A fast linter for Ignition 8.3 projects.",
    long_about = "A fast linter for Ignition 8.3 projects.\n\n`iglint [PATHS]...` is short for `iglint check [PATHS]...`.\n\nExit status: 0 when no finding reaches the --error-on threshold, 1 when one does (or a fix was applied with --exit-non-zero-on-fix), 2 for usage, configuration and internal errors.",
    disable_help_subcommand = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Lint files and directories (the default command).
    Check(Box<CheckArgs>),
    /// Show the documentation of a rule, as Markdown.
    Rule(RuleArgs),
    /// List rules.
    Rules(RulesArgs),
    /// Convert an ign-lint `rule_config.json` (and whitelist) to iglint.toml
    /// and a baseline.
    Migrate(MigrateArgs),
    /// Write a baseline of the current findings, so that only new findings
    /// fail (same as `check --update-baseline`).
    Baseline(Box<CheckArgs>),
    /// Run the language server (LSP over standard input and output), for
    /// editor integrations.
    Server,
    /// Run with the command-line interface of ign-lint 0.6.4.
    #[command(name = "ign-lint", disable_help_flag = true)]
    IgnLint {
        /// ign-lint arguments, see `iglint ign-lint --help`.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 0..)]
        args: Vec<std::ffi::OsString>,
    },
}

/// Names of the subcommands, to recognise the implicit `check`.
pub const SUBCOMMANDS: &[&str] = &[
    "check", "rule", "rules", "migrate", "baseline", "server", "ign-lint", "help",
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum ColorChoice {
    #[default]
    Auto,
    Always,
    Never,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Level {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum ListFormat {
    #[default]
    Text,
    Json,
}

#[derive(Debug, Default, Args)]
pub struct CheckArgs {
    /// Files or directories to lint [default: .]
    pub paths: Vec<PathBuf>,

    /// Use this configuration file (iglint.toml, pyproject.toml or an
    /// ign-lint rule_config.json) instead of searching for one.
    #[arg(long, value_name = "FILE", help_heading = "Configuration")]
    pub config: Option<PathBuf>,
    /// Ignore all configuration files.
    #[arg(long, conflicts_with = "config", help_heading = "Configuration")]
    pub isolated: bool,
    /// Rules to enable, replacing the configured selection (comma-separated
    /// codes, names, prefixes or ALL).
    #[arg(
        long,
        value_delimiter = ',',
        value_name = "RULES",
        help_heading = "Rule selection"
    )]
    pub select: Option<Vec<String>>,
    /// Rules to enable in addition to the configured selection.
    #[arg(
        long,
        value_delimiter = ',',
        value_name = "RULES",
        help_heading = "Rule selection"
    )]
    pub extend_select: Vec<String>,
    /// Rules to disable.
    #[arg(
        long,
        value_delimiter = ',',
        value_name = "RULES",
        help_heading = "Rule selection"
    )]
    pub ignore: Vec<String>,
    /// Rules to disable, in addition to `--ignore`.
    #[arg(long, value_delimiter = ',', value_name = "RULES", hide = true)]
    pub extend_ignore: Vec<String>,
    /// Report dynamic references that cannot be resolved statically.
    #[arg(long, help_heading = "Rule selection")]
    pub strict: bool,
    /// Skip the files listed in an ign-lint whitelist file.
    #[arg(long, value_name = "FILE", help_heading = "Configuration")]
    pub whitelist: Option<PathBuf>,

    /// Apply safe fixes.
    #[arg(long, help_heading = "Fixes")]
    pub fix: bool,
    /// Also apply unsafe fixes (with --fix, --fix-only or --diff), and show
    /// them as available otherwise.
    #[arg(long, help_heading = "Fixes")]
    pub unsafe_fixes: bool,
    /// Show the fixes as a unified diff instead of writing them.
    #[arg(long, conflicts_with_all = ["fix", "fix_only"], help_heading = "Fixes")]
    pub diff: bool,
    /// Apply fixes and do not report the remaining findings.
    #[arg(long, help_heading = "Fixes")]
    pub fix_only: bool,

    /// Output format [default: text, or `output-format` from the configuration]
    #[arg(long, value_enum, value_name = "FORMAT", help_heading = "Output")]
    pub output_format: Option<OutputFormat>,
    /// Write the report to this file instead of standard output.
    #[arg(long, short = 'o', value_name = "FILE", help_heading = "Output")]
    pub output_file: Option<PathBuf>,
    /// Show the number of findings per rule instead of the findings.
    #[arg(long, help_heading = "Output")]
    pub statistics: bool,
    /// List the files that would be linted, then exit.
    #[arg(long, help_heading = "Output")]
    pub show_files: bool,
    /// Show the settings that apply to the first path, then exit.
    #[arg(long, help_heading = "Output")]
    pub show_settings: bool,
    /// Print the time spent in each phase to standard error.
    #[arg(long, help_heading = "Output")]
    pub timing: bool,
    /// Print findings only, no summary or notices.
    #[arg(short, long, help_heading = "Output")]
    pub quiet: bool,
    /// Print nothing.
    #[arg(short, long, conflicts_with = "quiet", help_heading = "Output")]
    pub silent: bool,
    /// When to use colours.
    #[arg(long, value_enum, default_value_t = ColorChoice::Auto, help_heading = "Output")]
    pub color: ColorChoice,

    /// Exit with 0 even when there are findings.
    #[arg(long, help_heading = "Exit status")]
    pub exit_zero: bool,
    /// Exit with 1 when fixes were applied, even if nothing remains.
    #[arg(long, help_heading = "Exit status")]
    pub exit_non_zero_on_fix: bool,
    /// Lowest severity that makes the run fail [default: warning]
    #[arg(long, value_enum, value_name = "LEVEL", help_heading = "Exit status")]
    pub error_on: Option<Level>,
    /// Warnings and info findings do not fail the run (same as
    /// `--error-on error`).
    #[arg(long, conflicts_with = "error_on", help_heading = "Exit status")]
    pub ignore_warnings: bool,

    /// Baseline file of known findings that do not fail the run [default:
    /// `baseline` from the configuration, or .iglint-baseline.json in the
    /// project root if it exists]
    #[arg(long, value_name = "FILE", help_heading = "Baseline")]
    pub baseline: Option<PathBuf>,
    /// Write all current findings to the baseline file.
    #[arg(long, help_heading = "Baseline")]
    pub update_baseline: bool,
    /// Remove baseline entries whose findings were fixed.
    #[arg(long, conflicts_with = "update_baseline", help_heading = "Baseline")]
    pub prune_baseline: bool,

    /// Do not read or write the result cache.
    #[arg(long, help_heading = "Performance")]
    pub no_cache: bool,
    /// Directory of the result cache [default: .iglint_cache in the project
    /// root]
    #[arg(long, value_name = "DIR", help_heading = "Performance")]
    pub cache_dir: Option<PathBuf>,
    /// Number of worker threads [default: one per CPU]
    #[arg(long, value_name = "N", help_heading = "Performance")]
    pub threads: Option<usize>,

    /// Lint standard input as if it were this file. With --fix, the fixed
    /// text is written to standard output.
    #[arg(long, value_name = "PATH")]
    pub stdin_filename: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct RuleArgs {
    /// Rule code or name.
    #[arg(required_unless_present = "all")]
    pub rule: Option<String>,
    /// Show every rule.
    #[arg(long, conflicts_with = "rule")]
    pub all: bool,
    #[arg(long, value_enum, default_value_t = ListFormat::Text)]
    pub output_format: ListFormat,
}

#[derive(Debug, Args)]
pub struct RulesArgs {
    /// Only rules of this category (prefix such as SCR, or name such as
    /// scripts).
    #[arg(long)]
    pub category: Option<String>,
    #[arg(long, value_enum, default_value_t = ListFormat::Text)]
    pub format: ListFormat,
}

#[derive(Debug, Args)]
pub struct MigrateArgs {
    /// The ign-lint configuration to convert.
    #[arg(long, value_name = "FILE", default_value = "rule_config.json")]
    pub config: PathBuf,
    /// The ign-lint whitelist [default: .ignition-lint-whitelist.txt or
    /// .whitelist.txt next to the configuration, if present]
    #[arg(long, value_name = "FILE")]
    pub whitelist: Option<PathBuf>,
    /// Where to write the iglint configuration [default: iglint.toml next to
    /// the ign-lint configuration]
    #[arg(long, value_name = "FILE")]
    pub output: Option<PathBuf>,
    /// Where to write the baseline of the whitelisted files' findings
    /// [default: .iglint-baseline.json next to the iglint configuration]
    #[arg(long, value_name = "FILE")]
    pub baseline: Option<PathBuf>,
    /// Overwrite existing files.
    #[arg(long)]
    pub force: bool,
    /// Print the configuration instead of writing files.
    #[arg(long)]
    pub dry_run: bool,
}
