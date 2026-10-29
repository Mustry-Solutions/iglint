---
title: Suppressing findings
description: Silence individual findings in scripts with comments, and everything else with configuration or the baseline.
---

Sometimes a finding is correct but you want to keep the code as it is. iglint offers several ways to say so, from one line to a whole directory.

## In scripts: suppression comments

Any script (a library module, an event script, or a script embedded in a view) can carry a comment:

```python
value = legacy_global  # iglint: ignore[SCR101]
x = 1  # iglint: ignore[unused-variable, SCR1]
# iglint: ignore-file[SCR102]
```

- `# iglint: ignore[...]` suppresses the findings that **start on the comment's line**.
- A statement that spans several lines (inside brackets, after a backslash, or with a triple-quoted string) takes the comment on **the line where it ends**, which also suppresses the findings that start on its earlier lines. A finding on a line that opens a triple-quoted string has no room for a comment there:

  ```python
  query = """SELECT * FROM orders
      WHERE id = {}""".format(order_id)  # iglint: ignore[SEC001]
  ```

  A comment on an earlier line of the statement covers only the lines up to its own.
- `# iglint: ignore-file[...]` suppresses findings in the **whole script**, wherever the comment is. For a script embedded in a view, "the whole script" is that one script (one event action, transform or handler), not the whole view.
- Inside the brackets go rule codes, code prefixes (`SCR1`) or rule names, separated by commas. Without a list (`# iglint: ignore`), the comment suppresses every rule. Prefer a list: it documents what you meant and does not hide new problems on the same line.

Comments are found with a Python lexer, so `#` inside a string literal is never a suppression.

A suppression that suppresses nothing is reported by `SCR123` (`unused-suppression`, info). Its fix removes the stale comment, so suppressions do not outlive the code they were written for.

## In views and other JSON resources

`view.json`, `resource.json`, tag files and the other JSON resources cannot hold comments, and iglint does not invent a marker format for them. Use one of these instead:

| To silence | Use |
|---|---|
| one finding that you will fix later | the [baseline](/guides/baseline/) |
| one rule in some files | `[rules.<rule>] exclude = ["glob", ...]` |
| one rule everywhere | `ignore = ["<rule>"]` or `[rules.<rule>] enabled = false` |
| several rules, or other severities, in some directories | an `[[overrides]]` entry |
| whole files or directories | top-level `exclude` |

See [Configuration](/guides/configuration/) for the syntax.

## ign-lint whitelists

An ign-lint whitelist file (`.ignition-lint-whitelist.txt` or `.whitelist.txt`) skips whole files. `iglint check --whitelist FILE` honours one for compatibility, and `iglint migrate` turns it into a baseline, which is usually better: the whitelisted files are linted again, but only their new findings fail. See [Migrating from ign-lint](/guides/migrating-from-ign-lint/).
