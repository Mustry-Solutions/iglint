# iglint

A fast linter for [Ignition](https://inductiveautomation.com/ignition/) 8.3
projects, written in Rust.

iglint reads an Ignition project the way the gateway stores it on disk and
checks:

* **Perspective views**: component tree, properties, bindings, transforms,
  event actions, message handlers and custom methods;
* **Jython 2.7 scripts** everywhere they live: project library, gateway and
  session events, WebDev handlers, tag event scripts, and the scripts embedded
  in `view.json` (findings point inside the JSON string);
* **expressions** in bindings, transforms, tags and named query parameters;
* **cross-resource references**: views, named queries, library functions,
  message types, style classes, page URLs, parent projects;
* **tags and gateway resources**: UDT types and parameters, tag paths, alarm
  bindings, history providers.

It has more than 100 rules, auto-fixes, a language server for editors, and a
compatibility mode for projects that use
[ign-lint](https://github.com/bw-design-group/ignition-lint). See the
[rule reference](docs/rules/README.md).

## Why

* **Correct.** The script parser accepts exactly what Ignition's Jython 2.7
  accepts and is tested against it; `system.*` functions, expression
  functions and Perspective components come from a real 8.3 gateway. A false
  positive is a bug.
* **Precise.** Every finding has an exact location in the file on disk, also
  inside scripts and expressions embedded in JSON.
* **Project-aware.** Unknown views, named queries, library functions and
  message types are found across the whole project, including inherited
  projects.
* **Fast.** Each file is read and parsed once, in parallel, and results are
  cached.

## Performance

Benchmarks against ign-lint and how to reproduce them are in
[bench/README.md](bench/README.md).

## Installation

From PyPI (a wheel with the binary, no Python code):

```sh
pip install iglint      # or: uv tool install iglint / pipx install iglint
```

With Cargo (Rust 1.90 or newer):

```sh
cargo install --locked --git https://github.com/Mustry-Solutions/iglint iglint
# or, from a clone:
cargo install --locked --path crates/iglint
```

Prebuilt binaries for Linux (x86_64, aarch64; static musl), macOS (x86_64,
aarch64) and Windows (x86_64) are attached to every
[GitHub release](https://github.com/Mustry-Solutions/iglint/releases).

The VS Code extension is attached to the releases as well, as one VSIX per
platform (with the binary inside) and one that uses `iglint` from `PATH`:

```sh
code --install-extension iglint-vscode-v0.1.0-linux-x64.vsix
```

Check the installation with `iglint --version`.

## Quick start

Run iglint in the directory that holds your `projects/` folder (a gateway
data directory or a project repository):

```sh
iglint                      # same as `iglint check .`
iglint check projects/MyProject
iglint check --fix          # apply safe fixes
iglint check --diff         # show the fixes instead of writing them
iglint check --statistics   # count findings per rule
iglint rules                # list the rules
iglint rule SCR101          # explain a rule
```

A directory is searched for Ignition projects and gateway resource
collections; Python files are linted as Ignition scripts only inside
them. A `.py` file named on the command line is linted on its own, as a
standalone script. When a directory holds nothing to lint (a folder of
loose `.py` files without a `project.json`), the run says so on standard
error and exits with `0`.

Output looks like this:

```text
projects/fixture-hmi/com.inductiveautomation.perspective/views/Main/Overview/view.json:85:43: BIND004 [warning] `now` polls every 1000 ms, faster than the minimum of 10000 ms
   |
85 |                 "expression": "dateFormat(now(1000), \"HH:mm:ss\")"
   |                                           ^^^
   |
   = note: at root/Clock.props.text@binding
```

Exit status: `0` when no finding reaches the `--error-on` level (default
`warning`), `1` when one does, `2` for usage, configuration and internal
errors. `--exit-zero` always exits with `0` on findings.

## Configuration

iglint looks for `iglint.toml`, `.iglint.toml` or a `pyproject.toml` with a
`[tool.iglint]` table, starting at the linted path and going up. Without one,
the default rule set runs. Every key is optional:

```toml
# iglint.toml
# select = ["ALL"]                    # replace the default set
extend-select = ["NAME001", "REF011"]  # opt-in rules, on top of the defaults
ignore = ["A11Y"]                      # codes, prefixes, names or categories
exclude = ["projects/Sandbox/**"]      # never lint these paths
strict = false                         # also report dynamic references
baseline = "ci/iglint-baseline.json"   # default: .iglint-baseline.json, if present

[rules.BIND004]                        # or [rules.polling-interval]
minimum-interval = 5000

[rules.SCR103]
severity = "error"
exclude = ["projects/*/ignition/script-python/legacy/**"]

[rules.NAME001]
convention = "PascalCase"
target-node-types = ["component"]

[[overrides]]
files = ["projects/Legacy/**"]
ignore = ["NAME"]
severity = { SCR = "info" }

[fix]
# assume-scripts-do-not-read-workspace-files = true  # see below
# vetted-system-functions = ["system.mes.getThing"]  # see below
# vetted-expression-functions = ["myModuleFunction"]
```

* `select` replaces the default set, `extend-select` adds to it, `ignore`
  removes rules. Selectors are rule codes (`SCR101`), names
  (`undefined-name`), code prefixes (`SCR1`), categories (`SCR`) or `ALL`.
* `[rules.<code or name>]` sets `severity`, `enabled`, a per-rule `exclude`
  and the rule's options (see each rule's page).
* `[[overrides]]` apply other settings to the files matching `files`.
* `[fix] assume-scripts-do-not-read-workspace-files` (default `false`):
  every fix edits a file of the workspace, and a script that reads that
  file (`open(path).read()`, its size or modification time) sees the
  edit; a script that writes, appends to, renames or deletes one changes
  what the fix edits or moves it where a later read sees it. So while
  any code in the workspace may access a workspace file (any file
  operation whose path is not a literal outside the workspace, whatever
  its mode, or code that is not vetted), every fix is unsafe and says
  where that code is. Setting
  this to `true` states that no script reads the files of the workspace;
  checking that is your responsibility. It lifts only this condition,
  not the other reasons a fix may be unsafe.
* `[fix] vetted-system-functions` and `vetted-expression-functions`
  (default empty): a `system.*` function or an expression function that
  Ignition 8.3 does not have (one of a third-party module) may run code,
  read a frame or read files, so it keeps the fixes that depend on what
  code can do from being safe, and the fix says where it is. List the
  ones you have checked cannot: a `system.*` path also covers what is
  read from it (`system.mes.getThing` covers `system.mes.getThing.x`),
  expression functions match in any letter case. Checking them is your
  responsibility. `SCR201`'s `allow` and `BIND002`'s `extra-functions`
  only silence those rules' findings.
* Globs are relative to the configuration file and always use `/`.
* The command line wins over the file: `--select`, `--extend-select`,
  `--ignore`, `--strict`, `--config FILE`, `--isolated`. `--select`
  replaces the selection everywhere, `[[overrides]]` included, so an
  override's `select`/`extend-select` cannot re-add a rule the command line
  left out; `--extend-select` only adds and `--ignore` only removes. The
  same holds in ign-lint compatibility mode and for migrated configurations.

`iglint check --show-settings` prints the settings that apply to a path, and
`iglint check --show-files` the files that would be linted.

## Migrating from ign-lint

iglint has an equivalent for every ign-lint rule and reads ign-lint's
configuration.

* **Convert once:** `iglint migrate` turns `rule_config.json` into
  `iglint.toml`. If an ign-lint whitelist (`.ignition-lint-whitelist.txt` or
  `.whitelist.txt`) exists, the whitelisted files' current findings go into a
  baseline instead, so those files are linted from now on and only new
  findings fail. `iglint migrate --dry-run` shows the result first.
  ign-lint lints only `view.json` files, so the rules it runs on top of
  iglint's defaults (`SCR900`, which runs pylint, above all) are selected
  for views only, in an `[[overrides]]` entry with
  `files = ["**/view.json"]`; library modules, gateway events and WebDev
  scripts get iglint's native rules. Move a rule out of that entry (into
  `extend-select`) to run it everywhere. `iglint check` with a
  `rule_config.json` uses the same scope.
* **Keep the old command line:** `iglint ign-lint [ARGS]` accepts ign-lint
  0.6.4's arguments (`--config`, `--files`, `--whitelist`,
  `--ignore-warnings`, `--results-output`, ...) and prints ign-lint's output
  format. Called through a link named `ign-lint`, the binary behaves the same,
  so existing scripts keep working:

  ```sh
  ln -s "$(command -v iglint)" /usr/local/bin/ign-lint
  ign-lint --config rule_config.json --files "**/view.json"
  ```

* `iglint check` also reads a `rule_config.json` when no `iglint.toml`
  exists, with a deprecation warning.

> [!WARNING]
> `iglint ign-lint` finds more than ign-lint, so a CI gate that passes with
> ign-lint can fail after the switch. For example, `BIND005` reports
> references to view `custom` properties and `params` that do not exist,
> which ign-lint's `ComponentReferenceValidationRule` does not check, and
> `PylintScriptRule` also runs iglint's own script rules. `iglint ign-lint`
> has no baseline (its `--whitelist` skips whole files). To adopt iglint
> without breaking the build, run `iglint migrate` and then
> `iglint baseline` first, and gate on `iglint check`: today's findings are
> recorded in `.iglint-baseline.json` and only new ones fail. The extra
> `BIND005` errors are real (a reference that resolves nowhere evaluates to
> `null` with a bad quality); the baseline lets you fix them over time.

The differences in findings are listed in
[docs/compat/ign-lint-differences.md](docs/compat/ign-lint-differences.md).

## Suppressing findings

In scripts, with a comment:

```python
value = legacy_global  # iglint: ignore[SCR101]
x = 1  # iglint: ignore[unused-variable, SCR1]
# iglint: ignore-file[SCR102]
```

`ignore` applies to findings that start on the comment's line, `ignore-file`
to the whole script. Unused suppressions are reported (`SCR123`).

`view.json` and other JSON resources have no comments: use `exclude`, a
per-rule `exclude`, `[[overrides]]` or the baseline.

## Baseline

A baseline records the current findings so that only new ones fail, which
lets you adopt iglint on a large project at once:

```sh
iglint baseline             # writes .iglint-baseline.json
iglint check                # known findings are hidden; new ones fail
iglint check --prune-baseline   # drop entries whose findings were fixed
```

Entries are matched by rule, file and a fingerprint that survives moving code
around. Paths in the baseline are relative and use `/`, so one file works on
every platform. Commit it.

## Output formats and CI

`--output-format` (or `output-format` in the configuration) takes `text`
(default), `concise`, `grouped`, `json`, `json-lines`, `sarif`, `github`,
`azure`, `gitlab`, `junit` and `pylint`. `--output-file FILE` writes the
report to a file.

GitHub Actions, with annotations on the pull request and code scanning:

```yaml
- run: pip install iglint
- run: iglint check --output-format github
- run: iglint check --output-format sarif --output-file iglint.sarif --exit-zero
- uses: github/codeql-action/upload-sarif@v3
  with:
    sarif_file: iglint.sarif
```

(Uploading SARIF needs `permissions: security-events: write`.)

Azure DevOps (`##vso[task.logissue]` lines show up in the build summary):

```yaml
- script: |
    pip install iglint
    iglint check --output-format azure
  displayName: iglint
```

GitLab Code Quality:

```yaml
iglint:
  image: python:3.12
  script:
    - pip install iglint
    - iglint check --output-format gitlab --output-file gl-code-quality-report.json
  artifacts:
    when: always
    reports:
      codequality: gl-code-quality-report.json
```

JUnit XML (`--output-format junit`) works with most other CI systems.

## pre-commit

```yaml
# .pre-commit-config.yaml
repos:
  - repo: https://github.com/Mustry-Solutions/iglint
    rev: v0.1.0
    hooks:
      - id: iglint        # or iglint-fix to apply safe fixes
```

To replace ign-lint's own pre-commit hook, keep its id and change only
the repository: the `ign-lint` hook runs `iglint ign-lint` with the same
`files` pattern (`view\.json$`), file types, default `args`
(`--config=.ignition-lint-precommit.json`) and file passing, so your
configuration, `args` overrides, output and exit status stay as they were:

```yaml
repos:
  - repo: https://github.com/Mustry-Solutions/iglint   # was bw-design-group/ignition-lint
    rev: v0.1.0
    hooks:
      - id: ign-lint
        args: ["--config=rule_config.json", "--ignore-warnings"]   # optional, as before
```

These hooks build iglint from source on first use, which needs a Rust
toolchain. To use the PyPI wheel instead:

```yaml
repos:
  - repo: local
    hooks:
      - id: iglint
        name: iglint
        entry: iglint check
        language: python
        additional_dependencies: ["iglint==0.1.0"]
        types_or: [json, python]
        require_serial: true
```

Only the changed files are reported and fixed, but iglint analyses the
whole workspace around them (every project and gateway collection below
the workspace root: the configuration file's directory, the directory that
holds `projects/`, or the Git work tree), so cross-resource rules and the
safety of fixes are the same as for a run over the whole repository. A
warm single-file run takes a few tens of milliseconds on a workspace of
3,000 files.

A linked Git work tree (`git worktree add`, also one inside the repository
such as `.worktrees/feature` or `.claude/worktrees/feature`) is a checkout
of its own: the hook that runs in it analyses the work tree's projects and
collections, as if it were a separate clone, and runs in the main checkout
do not read the work tree's files. Submodules and nested repositories
stay part of the repository around them. iglint reads the `.git` entries
on disk, not `GIT_DIR` or `GIT_WORK_TREE`, so the hook and a run from a
shell find the same workspace root.

## Editors

`iglint server` is a language server (LSP over standard input and output):
findings while you type, quick fixes, "fix all" and rule documentation on
hover. Every fix it applies is computed when you pick it by a fresh
`iglint check --fix` of the folder with the editor's texts, so it is the
fix the command line would make; while an open `project.json`,
`resource.json`, configuration or baseline file has unsaved changes, no
fix is offered (save it first). The VS Code extension in
[editors/vscode](editors/vscode) runs it; other editors can start
`iglint server` directly.

## Rules

[docs/rules/README.md](docs/rules/README.md) lists every rule by category,
with its default severity, whether it runs by default and whether it can fix
findings. `iglint rule <CODE>` shows the same page in the terminal.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Changes are listed in
[CHANGELOG.md](CHANGELOG.md).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option. Third-party material is listed in
[NOTICE](NOTICE).

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in iglint by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.

iglint is not affiliated with or endorsed by Inductive Automation.
