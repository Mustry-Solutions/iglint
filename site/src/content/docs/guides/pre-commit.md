---
title: pre-commit
description: Run iglint on every commit with pre-commit, as iglint's own hooks, from a prebuilt binary, or as a drop-in for ign-lint's hook.
---

[pre-commit](https://pre-commit.com) runs hooks on the files of each commit. The iglint repository provides three hooks:

| Hook id | Runs | Files |
|---|---|---|
| `iglint` | `iglint check` | JSON and Python files |
| `iglint-fix` | `iglint check --fix` (safe fixes, then reports what is left) | JSON and Python files |
| `ign-lint` | `iglint ign-lint`, a drop-in for ign-lint's own hook | `view.json` files |

## iglint's hooks

```yaml
# .pre-commit-config.yaml
repos:
  - repo: https://github.com/Mustry-Solutions/iglint
    rev: v0.1.0
    hooks:
      - id: iglint        # or iglint-fix to apply safe fixes
```

These hooks install iglint from the repository with pip, which builds the binary from source on first use and needs a Rust toolchain. To skip the build, [install iglint](/installation/) yourself and declare a local hook that runs it from the `PATH`:

```yaml
repos:
  - repo: local
    hooks:
      - id: iglint
        name: iglint
        entry: iglint check
        language: system
        types_or: [json, python]
        require_serial: true
```

With the PyPI wheel, pre-commit can install the prebuilt binary itself:

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

## Only the changed files, but the whole workspace

pre-commit passes the changed files to the hook, and iglint reports and fixes only those files. It still analyses the whole **workspace** around them: every project and gateway configuration below the workspace root (the configuration file's directory, the directory that holds `projects/`, or the Git work tree). So cross-resource rules (an unknown view, an unused custom property read by another view) and the safety of fixes are the same as for a run over the whole repository. A warm single-file run takes a few tens of milliseconds on a workspace of 3,000 files.

A linked Git work tree (`git worktree add`, also one inside the repository such as `.worktrees/feature`) is a checkout of its own: the hook that runs in it analyses the work tree's projects as if it were a separate clone, and runs in the main checkout do not read the work tree's files. Submodules and nested repositories stay part of the repository around them. iglint reads the `.git` entries on disk, not `GIT_DIR` or `GIT_WORK_TREE`, so the hook and a run from a shell find the same workspace root.

## Replacing ign-lint's hook

To switch from ign-lint's pre-commit hook, keep its id and change only the repository. The `ign-lint` hook has the same `files` pattern (`view\.json$`), file types, default `args` (`--config=.ignition-lint-precommit.json`) and file passing as ign-lint's hook, so your configuration, `args` overrides, output and exit status stay as they were:

```yaml
repos:
  - repo: https://github.com/Mustry-Solutions/iglint   # was bw-design-group/ignition-lint
    rev: v0.1.0
    hooks:
      - id: ign-lint
        args: ["--config=rule_config.json", "--ignore-warnings"]   # optional, as before
```

As with ign-lint 0.6.4, the default `--config=.ignition-lint-precommit.json` must exist in the repository, or `args` must name another configuration. `iglint ign-lint` finds more than ign-lint (see [Migrating from ign-lint](/guides/migrating-from-ign-lint/)), so expect some new findings.
