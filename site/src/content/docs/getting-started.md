---
title: Getting started
description: A 5-minute tutorial on a small example project. First run, reading the output, safe and unsafe fixes, selecting rules and adopting a baseline.
---

This tutorial runs iglint on a small, made-up Perspective project with a dozen deliberate mistakes. Every output below was captured from the real binary (iglint 0.1.0); `site/scripts/run-example.sh` in the repository replays the whole tutorial.

## The example project

The project lives in the iglint repository under [`site/examples/quickstart`](https://github.com/Mustry-Solutions/iglint/tree/main/site/examples/quickstart). It is laid out the way an Ignition 8.3 gateway stores projects on disk:

```text
projects/demo-hmi/
├── project.json
├── com.inductiveautomation.perspective/views/Line/
│   ├── Overview/view.json          a view with a title, clock, logo and button
│   └── PumpDetail/view.json        a popup with a `pumpId` parameter
└── ignition/
    ├── script-python/plant/pumps/code.py    a library module
    ├── named-query/Pumps/ByArea/            a named query with an `area` parameter
    └── timer/refresh-pumps/handleTimerEvent.py   a gateway timer script
```

Copy it to a directory of its own, outside the iglint checkout, and make it a Git repository, as your project would be:

```sh
cp -R iglint/site/examples/quickstart ~/demo-ignition
cd ~/demo-ignition
git init -q && git add -A && git commit -qm "example project"
```

:::note[Why outside the checkout?]
iglint analyses the whole **workspace** around the files you lint: every project and gateway configuration below the workspace root (the directory of the configuration file, the directory that holds `projects/`, or the top of the Git work tree). Inside the iglint repository, the root would be the repository itself, whose test fixtures contain other projects.
:::

## First run

Run iglint in the directory that holds `projects/`. Without arguments, `iglint` is the same as `iglint check .`:

```sh
iglint
```

```text
projects/demo-hmi/com.inductiveautomation.perspective/views/Line/Overview/view.json:4:5: VIEW015 [warning] Custom property `legacyFilter` is never read
  |
4 |     "legacyFilter": ""
  |     ^^^^^^^^^^^^^^
  |
  = note: at view.custom.legacyFilter
  = help: [*] Remove unused custom property 'legacyFilter'

projects/demo-hmi/com.inductiveautomation.perspective/views/Line/Overview/view.json:34:57: BIND005 [error] `view.custom.aera`: custom property `aera` is not defined
   |
34 |                 "expression": "\"Area \" + {view.custom.aera}"
   |                                                         ^^^^
   |
   = note: at root/Title.props.text@binding

projects/demo-hmi/com.inductiveautomation.perspective/views/Line/Overview/view.json:56:43: BIND004 [warning] `now` polls every 1000 ms, faster than the minimum of 10000 ms
   |
56 |                 "expression": "dateFormat(now(1000), \"HH:mm:ss\")"
   |                                           ^^^
   |
   = note: at root/Clock.props.text@binding

projects/demo-hmi/com.inductiveautomation.perspective/views/Line/Overview/view.json:69:19: A11Y001 [warning] Image `Logo` has no alternative text (`props.altText`)
   |
69 |           "name": "Logo"
   |                   ^^^^^^
   |
   = note: at root/Logo

projects/demo-hmi/com.inductiveautomation.perspective/views/Line/Overview/view.json:84:75: REF001 [error] View `Line/PumpDetial` does not exist
   |
84 | ...m.perspective.openPopup(\"pumpDetail\", \"Line/PumpDetial\", params={\"pumpId\": 7})\n\tplant.pumps.refresh(self.view.cu...
   |                                            ^^^^^^^^^^^^^^^^^^^
   |
   = note: at root/Details@events.component.onActionPerformed

projects/demo-hmi/ignition/script-python/plant/pumps/code.py:1:8: SCR102 [warning] `os` imported but unused
  |
1 | import os
  |        ^^
  |
  = note: at plant.pumps
  = help: [*] Remove unused import `os`

projects/demo-hmi/ignition/script-python/plant/pumps/code.py:7:9: SCR204 [warning] `system.db.runNamedQuery` is deprecated; use `system.db.execQuery` or `system.db.execUpdate` instead
  |
7 |     rows = system.db.runNamedQuery("Pumps/ByAreaa", {"area": area})
  |            ^^^^^^^^^^^^^^^^^^^^^^^
  |
  = note: at plant.pumps

projects/demo-hmi/ignition/script-python/plant/pumps/code.py:7:33: REF004 [error] Named query `Pumps/ByAreaa` does not exist
  |
7 |     rows = system.db.runNamedQuery("Pumps/ByAreaa", {"area": area})
  |                                    ^^^^^^^^^^^^^^^
  |
  = note: at plant.pumps

projects/demo-hmi/ignition/script-python/plant/pumps/code.py:10:6: SCR110 [warning] `is` compares identity with a literal; use `==`
   |
10 |         if status is "fault":
   |            ^^^^^^^^^^^^^^^^^
   |
   = note: at plant.pumps
   = help: Replace `is` with `==` (unsafe fix available with `--unsafe-fixes`)
   = note: the fix is unsafe because `==` calls `__eq__` and can give a different result than the identity test

projects/demo-hmi/ignition/script-python/plant/pumps/code.py:11:4: SCR101 [error] Undefined name `log`
   |
11 |             log.warn("Pump %s is faulted" % row["name"])
   |             ^^^
   |
   = note: at plant.pumps

projects/demo-hmi/ignition/script-python/plant/pumps/code.py:18:21: SCR115 [error] `TypeError` is bound to the caught exception, not caught; use parentheses to catch both
   |
18 |     except ValueError, TypeError:
   |                        ^^^^^^^^^
   |
   = note: at plant.pumps
   = help: Add parentheses (unsafe fix available with `--unsafe-fixes`)
   = note: the fix is unsafe because the fix changes what the handler catches and no longer binds the second name

projects/demo-hmi/ignition/timer/refresh-pumps/handleTimerEvent.py:3:2: REF003 [error] `plant.pumps.refresh` accepts at most 1 argument positionally, the call passes 2
  |
3 |     plant.pumps.refresh("South", True)
  |     ^^^^^^^^^^^^^^^^^^^
  |
  = note: at refresh-pumps

Found 6 errors and 6 warnings.
[*] 2 fixable with the `--fix` option (2 hidden fixes can be enabled with the `--unsafe-fixes` option).
```

Twelve findings in three files, without any configuration. Most of them would only show up at run time on a gateway, and some not even there: a view that does not exist makes the popup silently fail, the mistyped property reference makes the title evaluate to `null` with a bad quality, and the `except ValueError, TypeError:` line looks right but does not catch `TypeError` at all.

## Reading a finding

Take the `BIND005` finding:

```text
projects/.../Line/Overview/view.json:34:57: BIND005 [error] `view.custom.aera`: custom property `aera` is not defined
   |
34 |                 "expression": "\"Area \" + {view.custom.aera}"
   |                                                         ^^^^
   |
   = note: at root/Title.props.text@binding
```

- `view.json:34:57` is the line and column in the file on disk. The expression is a JSON string inside the view, and iglint maps the position back through the JSON escapes, so the caret is under `aera` itself.
- `BIND005` is the rule code; `iglint rule BIND005` (or the [rule page](/reference/rules/bind005/)) explains it. Every rule also has a name (`unresolved-property-reference`) that you can use instead of the code anywhere.
- `[error]` is the severity: `error`, `warning` or `info`.
- `note: at root/Title.props.text@binding` is the place in the view, as a component path: the `Title` component under `root`, the binding on its `props.text`.
- `help: [*] ...` marks a finding with a fix that `--fix` applies. A help line with "(unsafe fix available with `--unsafe-fixes`)" has a fix that iglint could not prove safe, followed by the reason.

The exit status is `1` because there are findings of severity `warning` or higher. It is `0` when nothing reaches the `--error-on` level (default `warning`), and `2` for usage, configuration or internal errors.

To get an overview instead of the details, count the findings per rule:

```sh
iglint check --statistics
```

```text
1	A11Y001	    image-without-alt
1	BIND004	    polling-interval
1	BIND005	    unresolved-property-reference
1	REF001 	    unknown-view
1	REF003 	    library-function-arity
1	REF004 	    unknown-named-query
1	SCR101 	    undefined-name
1	SCR102 	[*] unused-import
1	SCR110 	    comparison-with-literal-identity
1	SCR115 	    except-tuple-without-parentheses
1	SCR204 	    deprecated-system-function
1	VIEW015	[*] unused-custom-property
```

## Preview and apply fixes

`--diff` shows what `--fix` would change, without writing anything:

```sh
iglint check --diff
```

```diff
--- a/projects/demo-hmi/com.inductiveautomation.perspective/views/Line/Overview/view.json
+++ b/projects/demo-hmi/com.inductiveautomation.perspective/views/Line/Overview/view.json
@@ -1,14 +1,10 @@
 {
   "custom": {
-    "area": "North",
-    "legacyFilter": ""
+    "area": "North"
   },
   "params": {},
   "propConfig": {
     "custom.area": {
-      "persistent": true
-    },
-    "custom.legacyFilter": {
       "persistent": true
     }
   },
--- a/projects/demo-hmi/ignition/script-python/plant/pumps/code.py
+++ b/projects/demo-hmi/ignition/script-python/plant/pumps/code.py
@@ -1,5 +1,5 @@
-import os
 
+
 logger = system.util.getLogger("plant.pumps")
 
 

Would fix 2 issue(s) in 2 file(s).
```

Two things to notice:

- The view fix removes the property and its `propConfig` entry and touches nothing else: fixes are byte-exact edits, so the rest of the file keeps the Designer's formatting.
- The import is replaced by an empty line instead of being deleted. Removing the line would shift every following line up by one, and line numbers are something a script can observe (tracebacks and logged stack traces show them). A safe fix keeps them.

Apply them:

```sh
iglint check --fix --output-format concise
```

```text
projects/demo-hmi/com.inductiveautomation.perspective/views/Line/Overview/view.json:30:57: BIND005 [error] `view.custom.aera`: custom property `aera` is not defined
projects/demo-hmi/com.inductiveautomation.perspective/views/Line/Overview/view.json:52:43: BIND004 [warning] `now` polls every 1000 ms, faster than the minimum of 10000 ms
projects/demo-hmi/com.inductiveautomation.perspective/views/Line/Overview/view.json:65:19: A11Y001 [warning] Image `Logo` has no alternative text (`props.altText`)
projects/demo-hmi/com.inductiveautomation.perspective/views/Line/Overview/view.json:80:75: REF001 [error] View `Line/PumpDetial` does not exist
projects/demo-hmi/ignition/script-python/plant/pumps/code.py:7:9: SCR204 [warning] `system.db.runNamedQuery` is deprecated; use `system.db.execQuery` or `system.db.execUpdate` instead
projects/demo-hmi/ignition/script-python/plant/pumps/code.py:7:33: REF004 [error] Named query `Pumps/ByAreaa` does not exist
projects/demo-hmi/ignition/script-python/plant/pumps/code.py:10:6: SCR110 [warning] `is` compares identity with a literal; use `==`
projects/demo-hmi/ignition/script-python/plant/pumps/code.py:11:4: SCR101 [error] Undefined name `log`
projects/demo-hmi/ignition/script-python/plant/pumps/code.py:18:21: SCR115 [error] `TypeError` is bound to the caught exception, not caught; use parentheses to catch both
projects/demo-hmi/ignition/timer/refresh-pumps/handleTimerEvent.py:3:2: REF003 [error] `plant.pumps.refresh` accepts at most 1 argument positionally, the call passes 2
Fixed 2 issues.
Found 6 errors and 4 warnings.
```

`--output-format concise` prints one line per finding. `--fix-only` applies the fixes without reporting what is left.

## Safe and unsafe fixes

The two remaining fixable findings have **unsafe** fixes. A fix is safe only when iglint can show that it cannot change what the project does in Ignition. These two change behaviour on purpose:

- `status is "fault"` to `status == "fault"`: `==` calls `__eq__`, which can give a different result than the identity test (in this case the intended one, but iglint cannot know that).
- `except ValueError, TypeError:` to `except (ValueError, TypeError):`: the handler then catches `TypeError` and no longer assigns the exception to a variable named `TypeError`.

Both are almost certainly what the author meant, which is exactly why they are offered, but a reviewer should look at them. Preview them with `--unsafe-fixes`:

```sh
iglint check --unsafe-fixes --diff
```

```diff
--- a/projects/demo-hmi/ignition/script-python/plant/pumps/code.py
+++ b/projects/demo-hmi/ignition/script-python/plant/pumps/code.py
@@ -7,7 +7,7 @@
 	rows = system.db.runNamedQuery("Pumps/ByAreaa", {"area": area})
 	for row in rows:
 		status = row["status"]
-		if status is "fault":
+		if status == "fault":
 			log.warn("Pump %s is faulted" % row["name"])
 	return len(rows)
 
@@ -15,5 +15,5 @@
 def parse_speed(text):
 	try:
 		return float(text)
-	except ValueError, TypeError:
+	except (ValueError, TypeError):
 		return None

Would fix 2 issue(s) in 1 file(s).
```

and apply them with `iglint check --fix --unsafe-fixes`. A fix that is safe in one project can be unsafe in another: if any code in the workspace reads frames, evaluates computed strings or reads the files a fix would edit, the fixes that depend on it become unsafe and say where that code is. [What iglint does not catch](/what-iglint-does-not-catch/#safe-and-unsafe-fixes) explains why.

The rest (the mistyped view, property and query names, the extra argument, the undefined `log`) need a human: iglint reports what is wrong but does not guess the intended name.

## Selecting rules

The default set (88 of the 110 rules) runs without configuration. Rules are selected by code, name, code prefix or category:

```sh
iglint check --select BIND,REF          # only these two categories
iglint check --ignore SCR204            # everything else
iglint check --extend-select REF011     # the defaults plus an opt-in rule
```

Selecting a whole category also turns on its opt-in rules. With `--select BIND,REF`, the `REF011`, `REF012` and `REF013` rules report unused resources:

```text
projects/demo-hmi/com.inductiveautomation.perspective/views/Line/Overview/view.json:1:1: REF011 [info] View `Line/Overview` is not referenced anywhere
projects/demo-hmi/com.inductiveautomation.perspective/views/Line/Overview/view.json:30:57: BIND005 [error] `view.custom.aera`: custom property `aera` is not defined
projects/demo-hmi/com.inductiveautomation.perspective/views/Line/Overview/view.json:52:43: BIND004 [warning] `now` polls every 1000 ms, faster than the minimum of 10000 ms
projects/demo-hmi/com.inductiveautomation.perspective/views/Line/Overview/view.json:80:75: REF001 [error] View `Line/PumpDetial` does not exist
projects/demo-hmi/com.inductiveautomation.perspective/views/Line/PumpDetail/view.json:1:1: REF011 [info] View `Line/PumpDetail` is not referenced anywhere
projects/demo-hmi/ignition/named-query/Pumps/ByArea/resource.json:1:1: REF013 [info] Named query `Pumps/ByArea` is not referenced anywhere
projects/demo-hmi/ignition/script-python/plant/pumps/code.py:7:33: REF004 [error] Named query `Pumps/ByAreaa` does not exist
projects/demo-hmi/ignition/script-python/plant/pumps/code.py:15:5: REF012 [info] Library function `plant.pumps.parse_speed` is never referenced
projects/demo-hmi/ignition/timer/refresh-pumps/handleTimerEvent.py:3:2: REF003 [error] `plant.pumps.refresh` accepts at most 1 argument positionally, the call passes 2
Found 4 errors, 1 warning and 4 info.
```

`Line/PumpDetail` is unused because the only reference to it is the misspelled one. (`Line/Overview` is unused because this small project has no page configuration that opens it.)

To browse the rules from the terminal:

```sh
iglint rules                 # every rule, with severity, fix and default
iglint rules --category REF  # one category
iglint rule SCR115           # the documentation of one rule
```

Settings you want every time go into an [`iglint.toml`](/guides/configuration/) file.

## Adopting iglint on an existing project

A real project that has never been linted can have hundreds of findings. You do not have to fix them before iglint can protect you: a **baseline** records today's findings, and from then on only new ones fail. Start again from the original example (`git checkout .`) and write one:

```sh
iglint baseline
```

```text
Wrote 12 finding(s) to the baseline .iglint-baseline.json.
```

```sh
iglint check
```

```text
All checks passed!
12 finding(s) hidden by the baseline .iglint-baseline.json.
```

Commit `.iglint-baseline.json`. Now add a new function with a typo to `plant/pumps/code.py`:

```python
def area_label(area):
	return "Area " + aera
```

```text
projects/demo-hmi/ignition/script-python/plant/pumps/code.py:23:19: SCR101 [error] Undefined name `aera`
   |
23 |     return "Area " + aera
   |                      ^^^^
   |
   = note: at plant.pumps

Found 1 error.
12 finding(s) hidden by the baseline .iglint-baseline.json.
```

The new finding fails the run, the old ones stay hidden. Entries are matched by rule, file and a fingerprint that survives moving code around, so editing other parts of the file does not bring old findings back. When you fix old findings (here, `log.warn` becomes `logger.warn`, and the new typo is fixed too), drop their entries:

```sh
iglint check --prune-baseline
```

```text
Removed 1 fixed finding(s) from the baseline .iglint-baseline.json.
All checks passed!
11 finding(s) hidden by the baseline .iglint-baseline.json.
```

More in the [baseline guide](/guides/baseline/).

## Next steps

- [Configuration](/guides/configuration/): rule selection, severities, rule options and per-path overrides in `iglint.toml`.
- [CI integration](/guides/ci/): annotations on pull requests, SARIF, GitLab and Azure DevOps.
- [Editors](/guides/editors/): findings while you type in VS Code or any LSP editor.
- [What iglint catches](/checks/) and, just as important, [what it does not catch](/what-iglint-does-not-catch/).
