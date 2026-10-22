# Changelog

All notable changes to iglint are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and versions follow
[Semantic Versioning](https://semver.org/). Rule codes and names are stable
from 0.1.0 on.

## [Unreleased]

### Changed

- Faster runs: about 3x on a 300-view project and 2x on 5,000 views
  (whitelist lookups, JSON formatting detected once per file, mimalloc,
  no teardown at exit, `iglint ign-lint` builds ign-lint's node model only
  for the options that show it). Peak memory on 5,000 views is down from
  1.9 GB to 1.1 GB: parse trees live for one phase only, and JSON arrays
  and objects are allocated at their exact size.
- The cache keeps a unit's results when another file changes without
  changing the facts the project index is built from (most edits inside
  one view or script), instead of discarding every entry.
- SCR900 runs pylint while the native rules run and, on POSIX, forks one
  worker per CPU from a single Python process; `jobs` is the number of
  workers.

### Added

- SCR900 option `disable`. A configuration read from an ign-lint
  `rule_config.json` sets it to the messages ign-lint's combined file
  disables (`unused-argument`, `missing-docstring`,
  `redefined-outer-name`, `function-redefined`), also with a `pylintrc`
  of its own.
- Benchmark and parity harness in `bench/` (see bench/README.md).
- `SCR124` trailing-whitespace (off by default, safe fix).
- `SCR900` fixes pylint's `trailing-whitespace` like ign-lint's
  `PylintScriptRule` (every script line `rstrip()`ed).

### Fixed

- Resources without `resource.json` are no longer silently skipped: a
  directory that holds the main file of a resource type (`view.json`
  below `views/`, a library `code.py`, an event script, a style class,
  tags, ...) is linted as that resource, and `view.json` and Python files
  outside every resource directory are linted standalone (with the
  project's library names). `REF009` reports that the gateway does not
  load them; references do not resolve to them and they are not
  inherited. `iglint ign-lint` lints such views too. Python files of
  resource types iglint does not model (and WebDev file content) are
  named in the run summary (`note: ...: not linted: <reason>`) instead of
  being counted as checked.
- Safe fixes no longer change behaviour (audit in
  `docs/internal/fix-safety.md`): `SCR102` leaves imports in class bodies
  alone and its fix is safe only for imports without side effects (JDK
  classes, verified standard modules); `SCR115` resolves the handler name
  like Python and its fix is unsafe; `NAME001` renames are unsafe when a
  view script hands a component to code outside the view; `SCR110`,
  `VIEW012`, and `VIEW013` with a stored value, have unsafe fixes; `SCR123`
  is unsafe when the comment holds more than the suppression.
- `SCR103`'s fix removes the whole assignment when the value calls nothing,
  instead of leaving a pointless statement.
- `SCR112` follows Jython's format grammar: one length modifier at most
  (`%lls` fails), `*` values of `%%` are consumed, huge `str.format`
  indexes are keywords (no overflow), signed indexes, `Missing ']'` and
  recursion errors.
- `SCR108` folds `True`, `False` and `None` only where they are builtins.
- `SCR900` passes every argument of the `pylint` command to pylint, also
  when forking (quoted words are supported), and `category-mapping` wins
  over the rule severity for WebDev scripts too.
- `SCR204` names documented replacements Ignition does not name itself
  (`system.tag.readBlocking` for `system.tag.read`, ...).
- Deeply nested or very long scripts no longer abort the run with a stack
  overflow (`elif` chains, `a.b.c...`, `x[0][0]...`, `1 + 1 + ...`, prefix
  operators, in modules and in view scripts, also through the language
  server). Long chains are parsed with loops and freed without recursion,
  and linting runs on threads with 256 MiB stacks (reserved, not
  committed). A statement deeper than 16,384 tree levels is reported as
  SCR001 "expression nested too deeply" (Ignition's Jython fails much
  earlier on such code). Expressions may now nest 2,000 levels instead of
  100, which is more than Jython accepts on a gateway thread.
- The text output no longer slows down quadratically with many findings
  on one very long line (a script in a view is one JSON line): 100,000
  findings took 74 s, now 0.3 s.
- `iglint ign-lint` prints ign-lint node paths for every finding,
  including BIND005 findings in query binding parameters and other binding
  expressions.
- `\N{...}` escapes in unicode literals resolve every name IA's Jython
  2.7.3 accepts (its Unicode 9.0 table, with its lookup quirks); unknown
  names are reported as SCR001, as Jython rejects them.
- `iglint ign-lint` groups pylint messages by category
  (`Pylint - Convention (C):`) like ign-lint.
- pylint messages that a native rule also reports (same place and name)
  are shown once.
- SARIF artifact URIs are percent-encoded (RFC 3986, per path segment) in
  results, related locations and fixes; files outside the working
  directory get absolute `file:` URIs instead of a path relative to
  `%SRCROOT%`.
- Azure Pipelines messages no longer show `%3B`/`%5D`: only `%`, `\r` and
  `\n` are escaped in the message, `;` and `]` only in properties.
- Fixes that delete statements (`SCR102`, `SCR103`) no longer empty a block
  when several of them remove every statement of it: the last one becomes
  `pass`.
- Fixing never makes a file worse: when the fixes of a pass make a script
  or expression stop parsing or compiling, empty a Perspective script or
  break the JSON, they are applied one at a time and the offending ones are
  dropped and reported (`warning: ...: fix not applied`).
- `SCR102` no longer reports top-level imports of library modules that other
  resources use as `pkg.mod.name`, and its fix is unsafe when such uses
  cannot be ruled out.
- `SCR101` no longer reports names created by `exec code in globals()` (and
  the other qualified `exec` forms that write the module or the function's
  locals), by `setattr(sys.modules[__name__], ...)`,
  `sys.modules[__name__].x = ...` or by writes to the builtins.
- `SCR108` compares keys with Python 2.7 equality: `True` and `1`, `0j` and
  `0`, equal tuples, `1e16` and `10000000000000000` are duplicates.
- `except ValueError, TypeError:` is reported once, by `SCR115`; `SCR103`
  and `SCR117` no longer report the same name.
- `iglint ign-lint`: the timestamps of the `--timing-output` report and of
  the aggregated `--results-output` summary are in local time, like
  ign-lint's.
- `All checks passed!` (and `iglint ign-lint`'s "No style inconsistencies
  found!") is no longer printed when the run had errors.
- Files that are not valid UTF-8 are linted instead of failing the run: the
  new rule `VIEW007 invalid-utf8` reports the invalid bytes (Ignition
  replaces them with U+FFFD), and such files get no fixes.

## [0.1.0]

First public release.

### Added

- Linter for Ignition 8.3 projects on disk: Perspective views, Jython 2.7
  scripts (project library, gateway and session events, WebDev, tag events,
  scripts embedded in views), expressions, style classes, page configuration,
  session properties, named queries, resource manifests and gateway tag
  resources.
- More than 100 rules in ten categories (`VIEW`, `NAME`, `BIND`, `SCR`,
  `REF`, `PERF`, `SEC`, `A11Y`, `STYLE`, `TAG`), with cross-resource checks
  over the whole project and its parent projects. Reference in
  [docs/rules](docs/rules/README.md), generated from the rule metadata.
- Jython 2.7 parser, semantic model and expression parser tested against the
  parsers shipped in the Ignition 8.3.3 image; `system.*`, expression
  function and Perspective component facts generated from a real gateway.
- Safe and unsafe auto-fixes (`--fix`, `--unsafe-fixes`, `--diff`) that only
  change the bytes they fix.
- Configuration in `iglint.toml`, `.iglint.toml` or `[tool.iglint]` in
  `pyproject.toml`: rule selection, per-rule severity, options and excludes,
  `[[overrides]]`, `strict`.
- Script suppression comments (`# iglint: ignore[...]`,
  `# iglint: ignore-file[...]`) and a baseline file
  (`iglint baseline`, `--prune-baseline`) that is portable across platforms.
- Output formats: text, concise, grouped, JSON, JSON lines, SARIF, GitHub
  Actions, Azure DevOps, GitLab Code Quality, JUnit and pylint.
- ign-lint 0.6.4 compatibility: `iglint ign-lint` (or the binary called as
  `ign-lint`) with ign-lint's arguments and output, `rule_config.json` and
  whitelist support, an equivalent for every ign-lint rule, and
  `iglint migrate` to convert the configuration.
- Result cache and parallel checking.
- Language server (`iglint server`) and a VS Code extension.
- Distribution as prebuilt binaries, a PyPI wheel (`pip install iglint`),
  pre-commit hooks and VS Code packages.

[Unreleased]: https://github.com/Mustry-Solutions/iglint/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/Mustry-Solutions/iglint/releases/tag/v0.1.0
