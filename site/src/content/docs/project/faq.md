---
title: FAQ and troubleshooting
description: Answers to common questions, and what to do when iglint does not behave as expected.
---

## General

### Does iglint need a running gateway, Docker or Python?

No. iglint is a single binary that reads the project files. Docker and a gateway are only used by iglint's own developers to regenerate its knowledge of Ignition and to record oracle outputs. Python is needed only to install it with pip, and for the opt-in `SCR900` rule, which runs pylint.

### Which Ignition versions does it support?

Ignition 8.3. Its scripting API, expression functions, component schemas and resource layout are 8.3's. See [What iglint does not catch](/what-iglint-does-not-catch/#ignition-versions).

### Does it lint Vision?

Vision client event scripts (`.py` files) are linted, with the `system.*` functions of the Vision client. Vision windows and templates are not read.

### Does it change my files?

Only when you ask: `--fix`, `--fix-only`, `iglint baseline` (or `--update-baseline`, `--prune-baseline`) and `iglint migrate` write files. Everything else only reads, apart from the result cache in `.iglint_cache`.

### Is a clean run a guarantee that the project works?

No. iglint finds a specific set of mistakes with certainty; it cannot see logic errors, values, or anything that is not in the repository. Read [What iglint does not catch](/what-iglint-does-not-catch/).

### Can I use it on Windows?

Yes. The release binaries and VS Code extension cover Windows x86_64 (as will the planned PyPI wheel), and the test suite runs on Windows in CI. Paths in the baseline and in globs always use `/`, so the same files work on every platform.

## Troubleshooting

### "no Ignition project or lintable files found"

```text
All checks passed!
warning: no Ignition project or lintable files found under . (Python files outside an Ignition project are linted only when named on the command line)
```

iglint looks for Ignition projects (directories with a `project.json`) and gateway resource collections. Run it in the directory that holds `projects/`, or pass the project directory. A loose `.py` file is linted only when you name it on the command line, as a standalone script.

### A setting in `iglint.toml` has no effect

Run `iglint check --show-settings PATH`. It shows which configuration file was found, the workspace root, and every setting and enabled rule that applies to `PATH`. Common causes: another configuration file closer to the linted path (iglint uses the first one it finds going up), a glob that is not relative to the configuration file's directory, or `--select` on the command line, which replaces the selection everywhere, `[[overrides]]` included.

### Exit status 2 and an error about the configuration

```text
error: iglint.toml:1:1: unknown key `selec`; did you mean `select`?
```

Configuration errors stop the run with status `2` and point at the position in the file. Keys are kebab-case (`extend-select`, not `extend_select`).

### `SCR900`: "`pylint` was not found"

`select = ["ALL"]` or `select = ["SCR"]` enables `SCR900`, which runs pylint. Install pylint (`pip install pylint`), set its command with `[rules.SCR900] pylint = "python3 -m pylint"`, or add `ignore = ["SCR900"]`.

### A module function is reported as unknown

Third-party and in-house modules add functions that iglint does not know. Declare them:

- `system.*` functions: `[rules.SCR201] allow = ["system.mymodule"]`;
- expression functions: `[rules.BIND002] extra-functions = ["myFunction"]`;
- component types: `[rules.VIEW003] allowed-types = ["acme.*"]`.

These only silence the findings. To also let fixes rely on those functions, list them under [`[fix]`](/guides/configuration/#the-fix-section).

### A finding is wrong

Please report it: a false positive is a bug. Include the smallest invented example that reproduces it (never customer code), the iglint version (`iglint --version`) and the output. Meanwhile, suppress it with `# iglint: ignore[CODE]` in a script, or with `exclude` or the baseline elsewhere (see [Suppressing findings](/guides/suppressions/)).

### Why is this fix unsafe? It looks harmless.

The note under the finding says why. Most often, some code in the workspace is unknown code (it reads frames, evaluates computed strings, calls a function iglint does not know, or reads project files), or the project is inheritable. See [safe and unsafe fixes](/what-iglint-does-not-catch/#safe-and-unsafe-fixes). Review the fix and apply it with `--unsafe-fixes` if it is right.

### Linting one file gives different cross-resource findings than linting the repository

It should not: iglint analyses the same workspace in both cases, from the workspace root (the configuration file's directory, the directory that holds `projects/`, or the Git work tree). Check the root with `--show-settings`. A linked Git work tree (`git worktree add`) is its own workspace, separate from the main checkout.

### Results look stale

The cache is keyed on file contents and the project's structure, and is tested against uncached runs. If you still suspect it, run with `--no-cache`, or delete `.iglint_cache`.

### Undefined names that Ignition provides

`SCR101` knows what Ignition injects into each kind of script. If it reports a name that does exist on your gateway, the name probably comes from somewhere iglint cannot see, such as a module or a script that writes into another module's namespace. (When a parent project is outside the workspace, `SCR101` already stays silent.) Please report the case if the source is in the workspace.
