---
title: Command line
description: Every iglint subcommand and option.
---

```text
iglint <COMMAND>
iglint [PATHS]...            # short for `iglint check [PATHS]...`
```

| Command | |
|---|---|
| [`check`](#iglint-check) | Lint files and directories (the default command) |
| [`baseline`](#iglint-baseline) | Write a baseline of the current findings |
| [`rules`](#iglint-rules) | List rules |
| [`rule`](#iglint-rule) | Show the documentation of a rule |
| [`migrate`](#iglint-migrate) | Convert an ign-lint `rule_config.json` (and whitelist) to `iglint.toml` and a baseline |
| [`server`](#iglint-server) | Run the language server |
| [`ign-lint`](#iglint-ign-lint) | Run with the command-line interface of ign-lint 0.6.4 |

`iglint --version` (`-V`) prints the version, commit and commit date; `--help` (`-h`) prints help for any command.

## Exit status

| Status | Meaning |
|---|---|
| `0` | No finding reaches the `--error-on` level |
| `1` | At least one finding reaches it, or a fix was applied with `--exit-non-zero-on-fix` |
| `2` | Usage, configuration or internal error |

When a directory holds nothing to lint (for example a folder of loose `.py` files without a `project.json`), iglint says so on standard error and exits with `0`.

## `iglint check`

```text
iglint check [OPTIONS] [PATHS]...
```

Lints files and directories (default: `.`). A directory is searched for Ignition projects and gateway resource collections; Python files are linted as Ignition scripts only inside them. A `.py` file named on the command line is linted on its own, as a standalone script. Whatever is linted, iglint analyses the whole workspace around it (see [pre-commit](/guides/pre-commit/#only-the-changed-files-but-the-whole-workspace)).

### Configuration

| Option | |
|---|---|
| `--config <FILE>` | Use this configuration file (`iglint.toml`, `pyproject.toml` or an ign-lint `rule_config.json`) instead of searching for one |
| `--isolated` | Ignore all configuration files |
| `--whitelist <FILE>` | Skip the files listed in an ign-lint whitelist file |

### Rule selection

| Option | |
|---|---|
| `--select <RULES>` | Rules to enable, replacing the configured selection (comma-separated codes, names, prefixes or `ALL`) |
| `--extend-select <RULES>` | Rules to enable in addition to the configured selection |
| `--ignore <RULES>` | Rules to disable |
| `--strict` | Report dynamic references that cannot be resolved statically |

### Fixes

| Option | |
|---|---|
| `--fix` | Apply safe fixes |
| `--unsafe-fixes` | Also apply unsafe fixes (with `--fix`, `--fix-only` or `--diff`), and show them as available otherwise |
| `--diff` | Show the fixes as a unified diff instead of writing them |
| `--fix-only` | Apply fixes and do not report the remaining findings |

### Output

| Option | |
|---|---|
| `--output-format <FORMAT>` | `text` (default, or `output-format` from the configuration), `concise`, `grouped`, `json`, `json-lines`, `sarif`, `github`, `azure`, `junit`, `pylint`, `gitlab`. See [Output formats](/reference/output-formats/) |
| `-o`, `--output-file <FILE>` | Write the report to this file instead of standard output |
| `--statistics` | Show the number of findings per rule instead of the findings |
| `--show-files` | List the files that would be linted, then exit |
| `--show-settings` | Show the settings that apply to the first path, then exit |
| `--timing` | Print the time spent in each phase to standard error |
| `-q`, `--quiet` | Print findings only, no summary or notices |
| `-s`, `--silent` | Print nothing |
| `--color <COLOR>` | `auto` (default), `always` or `never` |

### Exit status

| Option | |
|---|---|
| `--exit-zero` | Exit with `0` even when there are findings |
| `--exit-non-zero-on-fix` | Exit with `1` when fixes were applied, even if nothing remains |
| `--error-on <LEVEL>` | Lowest severity that makes the run fail: `info`, `warning` (default) or `error` |
| `--ignore-warnings` | Warnings and info findings do not fail the run (same as `--error-on error`) |

### Baseline

| Option | |
|---|---|
| `--baseline <FILE>` | Baseline file of known findings that do not fail the run (default: `baseline` from the configuration, or `.iglint-baseline.json` in the project root if it exists) |
| `--update-baseline` | Write all current findings to the baseline file |
| `--prune-baseline` | Remove baseline entries whose findings were fixed |

### Performance

| Option | |
|---|---|
| `--no-cache` | Do not read or write the result cache |
| `--cache-dir <DIR>` | Directory of the result cache (default: `.iglint_cache` in the project root) |
| `--threads <N>` | Number of worker threads (default: one per CPU) |

### Standard input

| Option | |
|---|---|
| `--stdin-filename <PATH>` | Lint standard input as if it were this file. With `--fix`, the fixed text is written to standard output |

## `iglint baseline`

```text
iglint baseline [OPTIONS] [PATHS]...
```

Writes a baseline of the current findings, so that only new findings fail. Same as `iglint check --update-baseline`, and accepts the same options as `check`. See [Baseline](/guides/baseline/).

## `iglint rules`

```text
iglint rules [--category <CATEGORY>] [--format text|json]
```

Lists the rules with code, name, severity, fix availability, default and summary. `--category` takes a prefix such as `SCR` or a name such as `scripts`. `--format json` includes each rule's options with their types and defaults, its ign-lint equivalent and its documentation.

## `iglint rule`

```text
iglint rule [OPTIONS] [RULE]
```

Shows the documentation of a rule, by code or name, as Markdown: the same text as the [rule pages](/reference/rules/). `--all` shows every rule; `--output-format json` prints the rule's metadata as JSON.

## `iglint migrate`

```text
iglint migrate [OPTIONS]
```

Converts an ign-lint `rule_config.json` (and whitelist) to `iglint.toml` and a baseline. See [Migrating from ign-lint](/guides/migrating-from-ign-lint/).

| Option | |
|---|---|
| `--config <FILE>` | The ign-lint configuration to convert (default: `rule_config.json`) |
| `--whitelist <FILE>` | The ign-lint whitelist (default: `.ignition-lint-whitelist.txt` or `.whitelist.txt` next to the configuration, if present) |
| `--output <FILE>` | Where to write the iglint configuration (default: `iglint.toml` next to the ign-lint configuration) |
| `--baseline <FILE>` | Where to write the baseline of the whitelisted files' findings (default: `.iglint-baseline.json` next to the iglint configuration) |
| `--force` | Overwrite existing files |
| `--dry-run` | Print the configuration instead of writing files |

## `iglint server`

```text
iglint server
```

Runs the language server (LSP over standard input and output), for editor integrations. See [Editors](/guides/editors/).

## `iglint ign-lint`

```text
iglint ign-lint [OPTIONS] [FILE]...
```

Lints Perspective `view.json` files with the command-line interface and output format of ign-lint 0.6.4. The binary behaves the same when it is called through a link named `ign-lint`.

| Option | |
|---|---|
| `--version` | Show the version and exit |
| `--config <PATH>` | Rule configuration file (default: `rule_config.json`) |
| `--files <VALUE>` | A glob or file, or a comma-separated list of them (default: `**/view.json`) |
| `-v`, `--verbose` | Show rule breakdown, per-file statistics and whitelist details |
| `--stats-only` | Print model statistics without linting |
| `--debug-nodes [<TYPE>...]` | Print up to 10 nodes (of the given types) per file |
| `--analyze-rules` | Show which nodes every rule processes |
| `--debug-output <DIR>` | Write the flattened map, model and statistics per file |
| `--ignore-warnings` | Warnings do not fail the run |
| `--timing-output <PATH>` | Write a timing report |
| `--results-output <PATH>` | Write the results report |
| `--whitelist <PATH>` | Skip the files listed in `PATH` |
| `--no-whitelist` | Ignore `--whitelist` |
| `--generate-whitelist <PATTERN>...` | Write a whitelist of the files matching the patterns and exit |
| `--whitelist-output <PATH>` | Target of `--generate-whitelist` (default: `.whitelist.txt`) |
| `--append` | Merge with the existing whitelist |
| `--dry-run` | Print what would be whitelisted |
| `--fix` | Apply safe fixes |
| `--fix-unsafe` | Apply safe and unsafe fixes |
| `--fix-dry-run` | Report results and print the proposed fixes |
| `--fix-rules <A,B>` | Only apply fixes of these rules (overrides `allow_fix: false`) |

## `iglint generate-docs`

```text
iglint generate-docs [--output-dir <DIR>] [--check]
```

A command for iglint's own development, not listed in `--help`: writes the rule reference (`docs/rules/*.md`) from the rule metadata. `--check` only checks that the pages are up to date and exits with `1` when they are not; CI runs it. The rule pages of this site are copies of those files.
