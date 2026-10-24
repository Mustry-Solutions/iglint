---
title: Configuration file
description: Every key of iglint.toml, .iglint.toml and [tool.iglint] in pyproject.toml.
---

iglint reads the first `iglint.toml`, `.iglint.toml` or `pyproject.toml` with a `[tool.iglint]` table that it finds from the linted path upward (see [Configuration](/guides/configuration/) for a walkthrough). Every key is optional. Unknown keys and values of the wrong type are errors, reported with their position and a "did you mean" suggestion.

Paths and globs are relative to the directory of the configuration file and always use `/`.

## Top-level keys

| Key | Type | Default | |
|---|---|---|---|
| `select` | list of selectors | the default rule set | Rules to run, replacing the default set |
| `extend-select` | list of selectors | `[]` | Rules to run in addition |
| `ignore` | list of selectors | `[]` | Rules not to run |
| `extend-ignore` | list of selectors | `[]` | Like `ignore`, added to it |
| `exclude` | list of globs | `[]` | Paths never linted (replaces the default) |
| `extend-exclude` | list of globs | `[]` | Paths never linted, added to `exclude` |
| `respect-gitignore` | boolean | `true` | Skip files ignored by `.gitignore`, `.ignore` and Git's excludes |
| `strict` | boolean | `false` | Also report dynamic references that cannot be resolved statically |
| `unfixable` | list of selectors | `[]` | Rules whose fixes are never applied |
| `baseline` | path | `.iglint-baseline.json` in the project root, if it exists | Baseline file |
| `cache-dir` | path | `.iglint_cache` in the project root | Result cache directory |
| `output-format` | string | `"text"` | Default output format (see [Output formats](/reference/output-formats/)) |
| `projects` | list of paths | discovered | Ignition projects to lint |
| `gateways` | list of paths | discovered | Gateway configuration resource trees to lint |
| `extend` | path | none | Another configuration file to inherit; this file's settings win |
| `rules` | table | | Per-rule settings, see [below](#rules) |
| `overrides` | array of tables | | Per-path settings, see [below](#overrides) |
| `fix` | table | | Fix safety declarations, see [below](#fix) |

**Selectors** are rule codes (`SCR101`), rule names (`undefined-name`), code prefixes (`SCR1`), categories (`SCR`) or `ALL`. When selectors overlap, the most specific one wins, and `ignore` wins a tie.

## Rules

`[rules.<rule>]` tables configure one rule each. The table name is a rule code or name: `[rules.BIND004]` and `[rules.polling-interval]` are the same table.

| Key | Type | |
|---|---|---|
| `severity` | `"error"`, `"warning"` or `"info"` | Severity of the rule's findings |
| `enabled` | boolean | Turns the rule on or off; has the last word over selection |
| `exclude` | list of globs | Paths the rule does not report on |
| *option* | depends on the option | The rule's options, listed on its [rule page](/reference/rules/) |

`iglint rule <CODE> --output-format json` prints a rule's options with their types and defaults.

## Overrides

`[[overrides]]` is an array of tables. Each entry applies settings to the files matching `files`, after the top-level settings, in order.

| Key | Type | |
|---|---|---|
| `files` | list of globs | Required. The files this entry applies to |
| `select`, `extend-select` | list of selectors | Rules to add for these files |
| `ignore`, `extend-ignore` | list of selectors | Rules to remove for these files |
| `severity` | table of selector to severity | For example `{ SCR = "info", SCR101 = "warning" }` |
| `strict` | boolean | Strict mode for these files |
| `rules` | table | Per-rule settings for these files, as in `[rules.<rule>]` |

`--select` on the command line replaces the selection everywhere, overrides included: an override cannot re-add a rule the command line left out.

## Fix

The `[fix]` table holds declarations that let more fixes be safe. Each is a statement about your project that iglint cannot check; see [safe and unsafe fixes](/what-iglint-does-not-catch/#safe-and-unsafe-fixes).

| Key | Type | Default | |
|---|---|---|---|
| `assume-scripts-do-not-read-workspace-files` | boolean | `false` | No script reads, writes, renames or deletes the files of the workspace |
| `vetted-system-functions` | list of `system.<package>.<function>` paths | `[]` | Third-party `system.*` functions that cannot run code, read a frame or read files; a path covers what is read from it |
| `vetted-expression-functions` | list of names | `[]` | Third-party expression functions with the same guarantee; any letter case |

`iglint check --show-settings` prints the values that apply.

## In `pyproject.toml`

```toml
[tool.iglint]
extend-select = ["REF011"]

[tool.iglint.rules.BIND004]
minimum-interval = 5000

[[tool.iglint.overrides]]
files = ["projects/Legacy/**"]
ignore = ["NAME"]

[tool.iglint.fix]
assume-scripts-do-not-read-workspace-files = true
```

## ign-lint `rule_config.json`

When no iglint configuration exists, `iglint check` reads an ign-lint `rule_config.json` (with a deprecation warning), and `--config rule_config.json` reads one explicitly. Its rules and options are mapped to the iglint rules; `iglint migrate` writes the equivalent `iglint.toml`. See [Migrating from ign-lint](/guides/migrating-from-ign-lint/).
