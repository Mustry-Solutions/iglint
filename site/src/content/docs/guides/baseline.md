---
title: Baseline
description: Record today's findings so that only new ones fail, and adopt iglint on a large existing project in one step.
---

A baseline is a file of known findings. Findings in the baseline are hidden and do not fail the run; every other finding does. It is the way to adopt iglint on a project that already has hundreds of findings: protect the project from new problems today, and fix the old ones over time.

## Writing a baseline

```sh
iglint baseline             # writes .iglint-baseline.json
```

`iglint baseline` accepts the same paths and options as `iglint check` (it is the same as `iglint check --update-baseline`), so the baseline records exactly the findings that run would report. Write it with the rule selection you will use in CI.

Commit the file. Paths in it are relative and always use `/`, so the same file works on Linux, macOS and Windows. They are relative to the directory of the configuration file, or to the workspace root when there is no configuration file or when `--config` names a file outside the Git checkout, so a shared configuration file does not tie the baseline to the checkout's directory name. Run `iglint check --show-settings` to see the directory (`Baseline paths relative to:`). Entries written relative to the configuration file's directory by earlier versions still match, and the next `iglint baseline` rewrites them.

## Using it

```sh
iglint check
```

```text
All checks passed!
12 finding(s) hidden by the baseline .iglint-baseline.json.
```

iglint uses `.iglint-baseline.json` in the project root when it exists. Another file can be set with `baseline = "ci/iglint-baseline.json"` in the configuration or `--baseline FILE` on the command line.

## How findings are matched

Each entry has a rule, a file, a fingerprint and a count:

```json
{
  "fingerprint": "697cbd65a83cfdc6",
  "rule": "BIND005",
  "path": "projects/demo-hmi/com.inductiveautomation.perspective/views/Line/Overview/view.json",
  "count": 1
}
```

The fingerprint is computed from the finding itself, not from its line number, so adding or removing code elsewhere in the file does not bring a known finding back. When the same finding occurs several times in a file, `count` records how many are known; one more fails the run.

## Keeping it small

When you fix findings that are in the baseline, remove their entries:

```sh
iglint check --prune-baseline
```

```text
Removed 1 fixed finding(s) from the baseline .iglint-baseline.json.
```

Pruning only removes entries; it never adds new findings to the baseline. To accept new findings deliberately, run `iglint baseline` again and review the diff of the file in the pull request.

## Baseline or suppression?

| | Baseline | Suppression comment | `exclude` / `ignore` |
|---|---|---|---|
| Works in | every file | scripts only | every file |
| Meaning | "known, to be fixed" | "intended, keep it" | "not checked here" |
| Hides new findings of the same rule | no | on that line or script | yes |
| Lives in | one JSON file | the code | the configuration |

## Adopting iglint on an existing project

1. Choose the rules (start with the defaults) and write an `iglint.toml` if needed.
2. `iglint baseline` and commit `.iglint-baseline.json`.
3. Gate CI on `iglint check` (see [CI integration](/guides/ci/)). Only new findings fail.
4. Fix old findings when you touch the code; `iglint check --prune-baseline` in the same commit.

Coming from ign-lint, `iglint migrate` does steps 1 and 2 for you; see [Migrating from ign-lint](/guides/migrating-from-ign-lint/).

`iglint ign-lint` (the compatibility mode) has no baseline: like ign-lint, it only knows whitelists, which skip whole files.
