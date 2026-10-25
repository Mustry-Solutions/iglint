---
title: Editors
description: Findings while you type, quick fixes and rule documentation, in VS Code or any editor with a Language Server Protocol client.
---

`iglint server` is a language server: it speaks the [Language Server Protocol](https://microsoft.github.io/language-server-protocol/) over standard input and output. It gives you:

- findings while you type (or on save), in views, scripts and the other resources, including inside the scripts and expressions embedded in `view.json`;
- cross-resource findings that update as you edit (unknown views, popup parameters, named queries, unused views and style classes);
- quick fixes for single findings (unsafe fixes are marked as such), a quick fix that adds `# iglint: ignore[CODE]` to a script line, and "fix all" for a file;
- the rule's documentation when you hover a finding.

The server uses the same configuration files as the command line and reloads them when they change.

## How fixes are computed

Every fix the server applies is computed when you pick it, by a fresh `iglint check --fix` of the folder on the disk as it would be after saving the file you fix, so it is the fix the command line would make. While any other open file has unsaved changes, no fix is computed: save the other files first. Editors that show disabled code actions (VS Code does) list the fixes disabled with the reason, for example "Save the other modified files to apply iglint fixes"; in VS Code, "other open files" are the Ignition files the extension sends to the server (views, manifests, resource and tag files, scripts). A file that starts with a byte order mark (which editors hide) or is not valid UTF-8 gets no fixes either. Clients that support versioned workspace edits get each edit for the document version it was computed for.

The findings shown while you type leave out pylint (`SCR900`): the server does not run pylint for them, and `iglint check` reports them. The fresh check behind a fix runs pylint when the configuration selects `SCR900`, as `iglint check --fix` does.

## VS Code

Install the extension from a VSIX file attached to every [GitHub release](https://github.com/Mustry-Solutions/iglint/releases); see [Installation](/installation/#vs-code-extension). The VSIX contains the `iglint` binary for every supported platform; on any other platform, the extension uses `iglint` from your `PATH`. The extension starts when the workspace contains a `project.json`, `view.json`, `iglint.toml` or `.iglint.toml`.

| Setting | Default | |
|---|---|---|
| `iglint.path` | `""` | Path of the binary; empty uses the bundled binary or `PATH`. |
| `iglint.args` | `[]` | Extra arguments for `iglint server`. |
| `iglint.run` | `onType` | Lint while typing (`onType`) or on save (`onSave`). |
| `iglint.scope` | `open` | Problems for open files (`open`) or the whole workspace (`workspace`). |
| `iglint.unsafeFixes` | `false` | Include unsafe fixes in "fix all". |
| `iglint.trace.server` | `off` | Trace the LSP messages. |

Commands: **iglint: Restart Server**, **iglint: Fix All Auto-Fixable Problems** and **iglint: Show Output**.

To apply safe fixes on save:

```json
"editor.codeActionsOnSave": { "source.fixAll.iglint": "explicit" }
```

## Other editors

Any editor with an LSP client can run `iglint server` directly. The server reads its settings from `initializationOptions` or from the `iglint` section of `workspace/didChangeConfiguration`:

| Setting | Values | Default |
|---|---|---|
| `run` | `"onType"`, `"onSave"` | `"onType"` |
| `scope` | `"open"`, `"workspace"` | `"open"` |
| `debounceMs` | milliseconds of quiet before analysis | `10` |
| `unsafeFixes` | include unsafe fixes in "fix all" | `false` |
| `ruleDocsUrl` | link of a rule's documentation, `{code}` and `{name}` are replaced | the rule pages in the repository |

For example, in Neovim 0.11 or newer (a starting point, not tested by the iglint test suite):

```lua
vim.lsp.config('iglint', {
  cmd = { 'iglint', 'server' },
  filetypes = { 'json', 'python' },
  root_markers = { 'iglint.toml', '.iglint.toml', 'project.json', '.git' },
  init_options = { run = 'onType', scope = 'open' },
})
vim.lsp.enable('iglint')
```

iglint reads projects from disk, so the editor needs the project files: a project repository, or the `projects/` folder of a development gateway's data directory.
