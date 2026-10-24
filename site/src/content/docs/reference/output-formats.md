---
title: Output formats
description: The eleven output formats of iglint check, with examples.
---

Choose a format with `--output-format <FORMAT>` or `output-format = "..."` in the configuration, and write it to a file with `--output-file FILE` (`-o`). The examples below were produced on the [Getting started](/getting-started/) project (JSON examples are shown with some objects on one line).

| Format | For |
|---|---|
| [`text`](#text) | people, in a terminal (default) |
| [`concise`](#concise) | one line per finding |
| [`grouped`](#grouped) | findings grouped by file |
| [`json`](#json) | scripts and tools: one JSON array |
| [`json-lines`](#json-lines) | streaming tools: one JSON object per line |
| [`sarif`](#sarif) | code scanning (GitHub, Azure DevOps and others) |
| [`github`](#github) | GitHub Actions annotations |
| [`gitlab`](#gitlab) | GitLab Code Quality |
| [`azure`](#azure) | Azure Pipelines logging commands |
| [`junit`](#junit) | test reports in most CI systems |
| [`pylint`](#pylint) | tools that parse pylint's `parseable` output |

## `text`

The default: each finding with a code frame, the place in the view or module, and the available fix.

```text
projects/demo-hmi/ignition/script-python/plant/pumps/code.py:18:21: SCR115 [error] `TypeError` is bound to the caught exception, not caught; use parentheses to catch both
   |
18 |     except ValueError, TypeError:
   |                        ^^^^^^^^^
   |
   = note: at plant.pumps
   = help: Add parentheses (unsafe fix available with `--unsafe-fixes`)
   = note: the fix is unsafe because the fix changes what the handler catches and no longer binds the second name

Found 1 error.
```

Colours are used when the output is a terminal; `--color always` or `never` overrides that.

## `concise`

```text
projects/demo-hmi/ignition/script-python/plant/pumps/code.py:11:4: SCR101 [error] Undefined name `log`
```

## `grouped`

```text
projects/demo-hmi/ignition/script-python/plant/pumps/code.py:
  1:8 SCR102 [warning] `os` imported but unused
  7:9 SCR204 [warning] `system.db.runNamedQuery` is deprecated; use `system.db.execQuery` or `system.db.execUpdate` instead
  7:33 REF004 [error] Named query `Pumps/ByAreaa` does not exist
```

## `json`

An array of findings. Rows and columns are 1-based; `node_path` is the place in the view or module; `fix` is `null` or the fix with its `applicability` (`safe` or `unsafe`), edits and, for unsafe fixes, the reason.

```json
[
  {
    "code": "SCR115",
    "name": "except-tuple-without-parentheses",
    "severity": "error",
    "message": "`TypeError` is bound to the caught exception, not caught; use parentheses to catch both",
    "filename": "projects/demo-hmi/ignition/script-python/plant/pumps/code.py",
    "location": { "row": 18, "column": 21 },
    "end_location": { "row": 18, "column": 30 },
    "node_path": "plant.pumps",
    "fingerprint": "20d18e47033982f7",
    "fix": {
      "applicability": "unsafe",
      "message": "Add parentheses",
      "edits": [
        {
          "filename": "projects/demo-hmi/ignition/script-python/plant/pumps/code.py",
          "content": "(ValueError, TypeError)",
          "location": { "row": 18, "column": 9 },
          "end_location": { "row": 18, "column": 30 }
        }
      ],
      "unsafe_reason": {
        "message": "the fix changes what the handler catches and no longer binds the second name",
        "what": null,
        "filename": null,
        "location": null
      }
    },
    "url": "https://github.com/Mustry-Solutions/iglint/blob/main/docs/rules/SCR115.md"
  }
]
```

When unknown code makes a fix unsafe, `unsafe_reason` also names the construct (`what`) and its location.

## `json-lines`

The same objects as `json`, one per line, without the surrounding array.

## `sarif`

[SARIF 2.1.0](https://docs.oasis-open.org/sarif/sarif/v2.1.0/sarif-v2.1.0.html), with the rule metadata (name, short and full description, default level, help link) in `tool.driver.rules`, and the code that makes a fix unsafe as a related location. Upload it to GitHub code scanning with `github/codeql-action/upload-sarif` (see [CI integration](/guides/ci/#github-actions)).

## `github`

[Workflow commands](https://docs.github.com/actions/reference/workflow-commands-for-github-actions) that GitHub Actions shows as annotations. Errors are `::error`, warnings `::warning`, info findings `::notice`.

```text
::error file=projects/demo-hmi/com.inductiveautomation.perspective/views/Line/Overview/view.json,line=84,col=75,endLine=84,endColumn=94,title=REF001::View `Line/PumpDetial` does not exist
```

## `gitlab`

A [Code Quality](https://docs.gitlab.com/ci/testing/code_quality/) report.

```json
[
  {
    "description": "REF001: View `Line/PumpDetial` does not exist",
    "check_name": "REF001",
    "fingerprint": "3eedbff7d8f2dda2",
    "severity": "major",
    "location": {
      "path": "projects/demo-hmi/com.inductiveautomation.perspective/views/Line/Overview/view.json",
      "lines": { "begin": 84, "end": 84 }
    }
  }
]
```

## `azure`

[Logging commands](https://learn.microsoft.com/azure/devops/pipelines/scripts/logging-commands) that Azure Pipelines shows in the build summary.

```text
##vso[task.logissue type=error;sourcepath=projects/demo-hmi/com.inductiveautomation.perspective/views/Line/Overview/view.json;linenumber=84;columnnumber=75;code=REF001]View `Line/PumpDetial` does not exist
```

## `junit`

JUnit XML: one test suite per file, one failing test case per finding.

```xml
<?xml version="1.0" encoding="UTF-8"?>
<testsuites name="iglint" tests="1" failures="1" errors="0">
  <testsuite name="projects/demo-hmi/com.inductiveautomation.perspective/views/Line/Overview/view.json" tests="1" failures="1" errors="0">
    <testcase name="org.iglint.REF001" classname="projects/demo-hmi/com.inductiveautomation.perspective/views/Line/Overview/view.json" file="projects/demo-hmi/com.inductiveautomation.perspective/views/Line/Overview/view.json" line="84">
      <failure message="View `Line/PumpDetial` does not exist" type="error">projects/demo-hmi/com.inductiveautomation.perspective/views/Line/Overview/view.json:84:75: View `Line/PumpDetial` does not exist</failure>
    </testcase>
  </testsuite>
</testsuites>
```

## `pylint`

pylint's `parseable` layout, for tools that already read it:

```text
projects/demo-hmi/com.inductiveautomation.perspective/views/Line/Overview/view.json:84: [REF001(unknown-view)] View `Line/PumpDetial` does not exist
```

## ign-lint's format

`iglint ign-lint` prints ign-lint 0.6.4's own output and writes its `--results-output` report; see [Migrating from ign-lint](/guides/migrating-from-ign-lint/#option-2-iglint-ign-lint).
