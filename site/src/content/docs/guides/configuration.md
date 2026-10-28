---
title: Configuration
description: Configure iglint with iglint.toml or pyproject.toml. Rule selection, severities, rule options, excludes, per-path overrides and the [fix] section.
---

iglint runs its default rule set without any configuration. To change that, add a configuration file to the repository. This guide walks through the common settings; the [configuration reference](/reference/configuration/) lists every key.

## Where iglint looks

iglint searches for a configuration file starting at the first linted path and going up through its parent directories. In each directory it tries, in this order:

1. `iglint.toml`
2. `.iglint.toml`
3. `pyproject.toml`, if it has a `[tool.iglint]` table

The first file found is used. If none is found, iglint looks for an ign-lint `rule_config.json` the same way and reads it with a deprecation warning (see [Migrating from ign-lint](/guides/migrating-from-ign-lint/)). Without any file, the defaults apply.

On the command line, `--config FILE` uses that file instead of searching, and `--isolated` ignores every configuration file. A file given with `--config` that lies outside the Git work tree of the linted paths only selects settings; it does not change which projects form the workspace.

`iglint check --show-settings PATH` prints the configuration file in use, the workspace root and every setting that applies to `PATH`, including the list of enabled rules with their severities. Use it whenever a setting does not seem to work.

## A typical file

```toml
# iglint.toml, next to the projects/ folder
extend-select = ["REF011", "REF013"]   # also report unused views and named queries
ignore = ["A11Y"]                      # not relevant for this HMI
exclude = ["projects/Sandbox/**"]      # never lint these paths

[rules.BIND004]                        # or [rules.polling-interval]
minimum-interval = 5000

[rules.SCR103]
severity = "error"
exclude = ["projects/*/ignition/script-python/legacy/**"]

[[overrides]]
files = ["projects/Legacy/**"]
ignore = ["NAME"]
severity = { SCR = "info" }
```

In `pyproject.toml` the same keys go under `[tool.iglint]`, with tables nested below it (`[tool.iglint.rules.BIND004]`, `[[tool.iglint.overrides]]`).

## Selecting rules

Three keys decide which rules run:

- `select` replaces the default set.
- `extend-select` adds rules to it.
- `ignore` removes rules.

Each takes a list of **selectors**: a rule code (`SCR101`), a rule name (`undefined-name`), a code prefix (`SCR1` is every `SCR1xx` rule), a category (`SCR`) or `ALL`.

When selectors overlap, the most specific one wins, and `ignore` wins a tie. So this enables `SCR101` and no other `SCR1xx` rule:

```toml
select = ["SCR"]
ignore = ["SCR1"]
extend-select = ["SCR101"]
```

`[rules.<rule>] enabled = true` or `false` has the last word for one rule.

:::caution
`select = ["ALL"]` and `select = ["SCR"]` also enable `SCR900`, which runs the real pylint and warns when pylint is not installed. Ignore it (`ignore = ["SCR900"]`) if you do not want pylint.
:::

Opt-in rules (*off* in the [rules overview](/reference/rules/)) are off by default because they are about style (`NAME001`, `VIEW018`), need project conventions (`VIEW012`, `VIEW013`), or are useful mainly as a periodic clean-up (`REF011` to `REF014`, `STYLE002`). `TAG003` (unknown tag paths) is opt-in because scripts can create tags at run time and one repository may hold several gateways with providers of the same name.

## Severity

Every rule has a default severity (`error`, `warning` or `info`). Change it per rule:

```toml
[rules.SCR103]
severity = "error"
```

or for many rules at once in an override (below). The severity decides the exit status together with `--error-on` (default `warning`): with the default, a run with only `info` findings exits with `0`.

## Rule options

Some rules have options, listed on each rule's page. They go in the rule's table, next to `severity`, `enabled` and `exclude`:

```toml
[rules.NAME001]
convention = "PascalCase"
target-node-types = ["component"]

[rules.PERF001]
max-depth = 8
```

Option names are kebab-case. Unknown keys and values of the wrong type are errors, reported at their position in the file with a "did you mean" suggestion. `iglint rule <CODE> --output-format json` lists a rule's options with their types and defaults.

## Excluding files

- `exclude` (top level): paths that are never linted. Setting it replaces the excludes of an `extend`ed file; `extend-exclude` adds to them.
- `[rules.<rule>] exclude`: paths one rule does not report on.
- `respect-gitignore` (default `true`): files ignored by `.gitignore`, `.ignore` and Git's excludes are skipped.

Globs are relative to the directory of the configuration file and always use `/`, also on Windows. An excluded file is not reported on, but it can still be part of the workspace that cross-resource rules see: excluding a view does not make references to it unknown.

## Per-path overrides

An `[[overrides]]` entry applies other settings to the files that match its `files` globs. It accepts `select`, `extend-select`, `ignore`, `extend-ignore`, `severity`, `strict` and `rules`:

```toml
[[overrides]]
files = ["projects/Legacy/**"]
ignore = ["NAME"]
severity = { SCR = "info", SCR101 = "warning" }

[[overrides]]
files = ["**/view.json"]
extend-select = ["VIEW018"]

[overrides.rules.PERF001]   # rule options for the files of the last [[overrides]] entry
max-depth = 16
```

Overrides apply in order, after the top-level settings. `files` is required.

## Strict mode

```toml
strict = true
```

Some rules cannot decide with certainty when a value is computed at run time, for example a poll rate that is a property reference, a style class without a style class resource when no gateway theme is in the workspace, or an unknown member of a known `system.*` package that is not a near miss of a real one (third-party modules add members). They stay silent by default. `strict = true` (or `--strict`) makes them report those cases too; each rule's page says what its strict mode adds. Strict findings are not necessarily bugs.

## The `[fix]` section

Three settings let you declare facts about your project that iglint cannot check, so that more fixes can be safe. Read [safe and unsafe fixes](/what-iglint-does-not-catch/#safe-and-unsafe-fixes) first: each is a statement you make, and checking it is your responsibility.

```toml
[fix]
assume-scripts-do-not-read-workspace-files = true
vetted-system-functions = ["system.mes.getThing"]
vetted-expression-functions = ["myModuleFunction"]
```

- `assume-scripts-do-not-read-workspace-files` (default `false`). Every fix edits a file of the workspace, and a script that reads that file (its bytes, size or modification time) would see the edit. So while any code in the workspace may access a workspace file (any file operation whose path is not a literal outside the workspace, or code that is not vetted), every fix is unsafe and says where that code is. Setting this to `true` states that no script reads the files of the workspace. It lifts only this condition, not the other reasons a fix may be unsafe.
- `vetted-system-functions` (default empty). A `system.*` function that Ignition 8.3 does not have (one added by a third-party module) may run code, read a frame or read files, so it keeps the fixes that depend on what code can do from being safe. List the ones you have checked cannot. A path also covers what is read from it: `system.mes.getThing` covers `system.mes.getThing.x`. Paths are `system.<package>.<function>`, without wildcards.
- `vetted-expression-functions` (default empty). The same for expression functions of third-party modules. Names match in any letter case.

These settings only affect fixes. To silence findings about third-party functions, use `SCR201`'s `allow` option and `BIND002`'s `extra-functions` option instead; those in turn do not make fixes safe.

## Other settings

- `baseline`: the baseline file (default `.iglint-baseline.json` in the project root, if it exists). See [Baseline](/guides/baseline/).
- `output-format`: the default output format, for example `"concise"` or `"github"`. See [Output formats](/reference/output-formats/).
- `cache-dir`: where the result cache lives (default `.iglint_cache` in the project root). See [Performance and caching](/guides/performance/).
- `unfixable`: rules whose fixes are never applied.
- `projects` and `gateways`: more project directories and gateway configuration trees for the workspace (relative to the configuration file). They are analysed with the rest (parent projects, references, the safety of fixes) but only linted when you name them on the command line.
- `extend`: another configuration file to inherit from, relative to this one. Settings in this file win.

## The command line wins

Command-line options apply after every file: `--select`, `--extend-select`, `--ignore`, `--strict`, `--config FILE` and `--isolated`. `--select` replaces the selection everywhere, `[[overrides]]` included, so an override's `select` or `extend-select` cannot re-add a rule the command line left out; `--extend-select` only adds and `--ignore` only removes. The same holds in ign-lint compatibility mode and for migrated configurations.
