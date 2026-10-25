---
title: Security, performance, accessibility
description: What iglint finds about SQL injection, credentials, code execution, OS commands, polling, view size and accessibility.
---

All examples are invented; the messages are iglint's.

## Security

These rules point at patterns that are risky on a gateway, where scripts run with the gateway's permissions and database connections. They are warnings: iglint cannot know whether the input can be controlled by a user, so treat each finding as a question to answer.

```python
def find_orders(customer):
	query = "SELECT * FROM orders WHERE customer = '%s'" % customer
	return system.db.runQuery(query)


def connect():
	password = "hunter2"
	return system.net.httpClient().get("https://example.invalid/api", headers={"Authorization": "Bearer abc123def456"})


def cleanup(path):
	import os
	os.system("rm -rf " + path)


def run(code):
	return eval(code)
```

```text
code.py:2:10: SEC001 [warning] SQL text for `system.db.runQuery` is built from non-literal values; use a prepared query with `?` arguments
code.py:7:13: SEC002 [warning] Hardcoded credential in `password`; load it from a secret provider or a protected setting
code.py:8:94: SEC002 [warning] Hardcoded credential in `Authorization`; load it from a secret provider or a protected setting
code.py:13:2: SEC006 [warning] `os.system` starts an operating system process
code.py:17:9: SEC003 [warning] `eval` runs code that is not a literal
```

(The same code also gets `SCR204` for the deprecated `runQuery` and `SCR103` for the unused `password`; they are left out here.)

| Rule | Reports |
|---|---|
| `SEC001` | SQL text passed to `system.db.runQuery`, `runUpdateQuery`, `runScalarQuery` (and the query text of the prepared variants) built with `%`, `+` or `format` from values that are not literals, through aliases and plain assignments |
| `SEC002` | a literal password, token, secret or API key in a script (variables, attributes, dict keys, keyword arguments, `password` parameters of `system.*` calls) or in an HTTP binding (authentication, `Authorization` headers); placeholders and field names are skipped |
| `SEC003` | `eval`, `exec`, `execfile` or `compile` on input that is not a literal |
| `SEC004` | a plain `http://` URL in an HTTP binding or a `system.net` call, except localhost (info) |
| `SEC005` | a named query parameter of type `QueryString`, which is pasted into the SQL text |
| `SEC006` | a script that starts an operating system process: `os.system`, `os.popen`, the `exec` and `spawn` families, `subprocess`, `commands`, `Runtime.exec`, `ProcessBuilder` |

iglint is not a security scanner: it does not track data from user input to these sinks, and it does not know your gateway's security settings, roles or zones.

## Performance

| Rule | Reports | Default |
|---|---|---|
| `BIND004` | an expression that polls faster than `minimum-interval` (10 s) | on |
| `PERF002` | a query, tag history or HTTP binding that polls faster than `minimum-interval` | on |
| `PERF001` | components nested deeper than `max-depth` (12) | on (info) |
| `VIEW006` | oversized data pasted into view-level custom properties (large arrays, deep nesting, long strings) | on |
| `PERF003` | a component with `meta.visible: false` that still has bindings | off |
| `PERF004` | a view with more than `max-components` components | off |
| `SCR302` | `time.sleep` in a Perspective event script, which blocks the session's thread | on |
| `SCR303` | a library module that calls `system.*` when it is imported | on |

```text
view.json:320:27: PERF002 [warning] Query binding polls every 1 s (minimum 10 s)
view.json:246:45: SCR302 [warning] `time.sleep` blocks the Perspective session's thread while it sleeps
code.py:2:10: SCR303 [warning] `system.tag.readBlocking` runs whenever the module is imported, in the importing script's scope
```

These are static hints. iglint does not measure how a project performs on a gateway; see [What iglint does not catch](/what-iglint-does-not-catch/#performance-at-run-time).

## Accessibility

| Rule | Reports | Default |
|---|---|---|
| `A11Y001` | an Image component without alternative text (`props.altText`), missing or blank and not bound | on |
| `A11Y002` | a clickable button or icon without text or tooltip | off |

```text
view.json:69:19: A11Y001 [warning] Image `Logo` has no alternative text (`props.altText`)
view.json:281:19: A11Y002 [warning] Clickable icon `IconOnly` has no text or tooltip; give it `meta.tooltip.text`
```

Screen readers announce an image without alternative text by its file name. These two rules cover only what can be decided from the view file; contrast, focus order and keyboard navigation are not checked.
