# iglint for VS Code

[iglint](https://github.com/Mustry-Solutions/iglint) is a fast linter for
Inductive Automation Ignition 8.3 projects. This extension runs the iglint
language server and shows its findings while you edit:

* Perspective `view.json` files, including the Jython scripts and expressions
  embedded in them (problems point inside the JSON string);
* project library scripts and event scripts (`*.py`);
* style classes, page configuration, session properties, resource manifests
  and gateway tag files;
* cross-resource checks (unknown views, popup parameters, named queries,
  unused views and style classes), updated as you type.

Quick fixes apply iglint's fixes (unsafe ones are marked), suppress script
findings with `# iglint: ignore[CODE]`, and **iglint: Fix All Auto-Fixable
Problems** (or `"editor.codeActionsOnSave": {"source.fixAll.iglint": "explicit"}`)
fixes a whole file. A fix is computed when you pick it, by a fresh
`iglint check --fix` on the disk as it would be after saving the file you
fix. While any other open Ignition file (a file this extension sends to
the server: views, manifests, resource and tag files, scripts) has unsaved
changes, iglint's fixes are shown disabled ("Save the other modified files
to apply iglint fixes") and fix-all on save does nothing; save the other
files first. Files that start with a byte order mark (which the editor
hides) or are not valid UTF-8 get no fixes; the disabled fix says why.
Hover a problem for the rule's documentation. pylint findings (`SCR900`)
are not shown in the editor; `iglint check` reports them.

## Requirements

An `iglint` binary: bundled with platform builds of the extension, found on
`PATH`, or set with `iglint.path`. Configuration comes from `iglint.toml`
(or `[tool.iglint]` in `pyproject.toml`) and is reloaded when it changes.

## Settings

| Setting | Default | |
|---|---|---|
| `iglint.path` | `""` | Binary path; empty uses the bundled binary or `PATH`. |
| `iglint.args` | `[]` | Extra arguments for `iglint server`. |
| `iglint.run` | `onType` | Lint while typing (`onType`) or on save (`onSave`). |
| `iglint.scope` | `open` | Problems for open files or the whole workspace. |
| `iglint.unsafeFixes` | `false` | Include unsafe fixes in "fix all". |
| `iglint.trace.server` | `off` | Trace the LSP messages. |

## Commands

* **iglint: Restart Server**
* **iglint: Fix All Auto-Fixable Problems**
* **iglint: Show Output**
