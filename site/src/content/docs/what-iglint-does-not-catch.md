---
title: What iglint does not catch
description: The limits of static analysis on Ignition projects, what iglint deliberately leaves alone, the closed-world assumption, and what a safe fix guarantees.
---

iglint reads files. It never runs a script, never connects to a gateway, and only knows what is in the workspace it analyses. That makes it fast and repeatable, and it sets hard limits. This page lists them as precisely as we can, so that a clean run is not mistaken for more than it is.

The rule that governs everything else: **when iglint cannot decide with certainty, it stays silent** (or reports only in `strict` mode). So most limits below are not false positives waiting to happen; they are problems iglint will simply not tell you about.

## Behaviour at run time

iglint does not execute anything. It cannot see:

- **Logic errors.** A wrong setpoint, an inverted condition, an off-by-one in a loop, a query that returns the wrong rows: if the code is valid Jython that calls existing functions with plausible arguments, iglint has nothing to say.
- **Values.** Tag values and qualities, query results, session and page properties at run time, user input, the contents of databases, files on the gateway, or what an HTTP API returns.
- **Timing and concurrency.** Race conditions between tag change scripts, timers that overlap, blocking calls outside the cases `SCR302` and `SCR303` recognise, deadlocks.
- **Errors that depend on data.** A `KeyError` on a missing dataset column, a `None` that reaches arithmetic, a division by zero, a `TypeError` from a value of the wrong type.

## Types beyond what can be proven

iglint does not infer types. It checks the arguments of calls to `system.*` functions, expression functions and project library functions (their number and keywords), and `BIND011` (opt-in) reports obvious literal type errors in expressions. It does not check:

- attributes and methods of the objects these functions return (`system.tag.readBlocking(paths)[0].vale` is not reported);
- methods of Java objects, datasets, Perspective component objects beyond the names it models, or Python objects in general;
- the types of arguments (a string where a list of tag paths is expected);
- whether a variable may be unbound on some path. `SCR118` reports a read only where **no** assignment can have happened; a variable assigned in an `if` branch and read after it is not reported.

## Names and paths computed at run time

Every cross-resource rule checks **literal** references. A reference built at run time cannot be checked and is not reported:

```python
system.perspective.openPopup("p", "Popups/" + name)   # REF001: not checked
system.db.runNamedQuery("Reports/" + name, {})          # REF004: not checked
self.getSibling("Button" + str(n))                      # SCR305: not checked
getattr(plant.pumps, action)()                          # REF002: not checked
```

The same holds for indirect tag paths with placeholders, tag paths built in scripts, message types computed from variables, property references built as strings, `runScript` with a computed function name, and anything passed through `system.util.getGlobals()`. Rules that report unused resources (`REF011` to `REF014`) count a computed path by its literal prefix, so `"Popups/" + name` keeps every view below `Popups/` in use.

Some rules have a `strict` mode that reports a few of these cases (for example a poll rate that is not a literal); each rule's page says what its strict mode adds.

## What is not in the repository

iglint knows the workspace, not the gateway. It does not know about:

- **Projects and resources that only exist on the gateway.** A parent project that is not in the workspace makes iglint silent about every name that could come from it: `SCR101` does not report undefined names in a project whose parent is not in the workspace, and `REF001` does not report its view paths.
- **Gateway configuration that is not exported** into the repository: tag providers, tags and UDTs that are not under `config/resources/`, database connections, identity providers, security levels and zones, gateway themes (so `STYLE001` reports a missing style class only in strict mode when no theme is in the workspace), Perspective page configuration of other projects.
- **The installed modules.** iglint's knowledge of `system.*`, expression functions and Perspective components is Ignition 8.3's own. Third-party and in-house modules add more:
  - an unknown `system.*` member of a known package is reported only when it is a near miss of a real one (all of them with `strict`); declare module functions with `SCR201`'s `allow` option;
  - an unknown **expression** function is reported by `BIND002`; declare module functions with its `extra-functions` option;
  - component types that do not start with `ia.` are not checked unless you set `VIEW003`'s `allowed-types`.
- **The SQL of named queries.** iglint reads a named query's parameters and type, not whether its SQL is valid for your database or whether its tables and columns exist.

## Resources iglint does not read

iglint identifies these resources, but does not analyse their contents:

- **Vision windows and templates** (`window.bin`/`window.xml`, `template.bin`/`template.xml`) and the other Vision project resources. Vision **client event scripts** (startup, shutdown, timer, tag change, message, keystroke and shutdown-intercept scripts in `.py` files) are linted like gateway events, and `SCR205` knows which `system.*` functions exist in the Vision client.
- Reports, alarm notification pipelines, SFC charts, transaction groups, event streams, the deprecated app script library, pre-8.3 aggregate event scripts, and resources of modules iglint does not know.
- JavaScript that runs in the browser (for example in Markdown or inline frame components).

Code in these resources is invisible to the rules. For fixes, it counts as **unknown code** (see below).

## Performance at run time

`BIND004`, `PERF001` to `PERF004`, `VIEW006`, `SCR302` and `SCR303` flag patterns that commonly cause load. iglint does not measure anything: it does not know how many sessions open a view, how slow a query is, how large a dataset gets, or how busy the gateway is.

## Ignition versions

iglint targets Ignition 8.3 and Jython 2.7 as Ignition 8.3.3 ships it (IA's fork 2.7.3). Its scripting API, expression functions and component schemas were generated from an 8.3 gateway. Projects for other versions are not tested; 7.x script prefixes (`project.`, `shared.`, `app.`) are reported by `SCR206` because they do not exist in 8.3.

## Safe and unsafe fixes

Many rules can fix their findings. iglint sorts every fix into one of two classes:

- A **safe** fix is applied by `--fix`. It is safe only when iglint can show that it **cannot change what the project does** in Ignition 8.3 and Jython 2.7, with the information it has.
- An **unsafe** fix is applied only with `--fix --unsafe-fixes`, shown with `--unsafe-fixes --diff`, and marked in the editor. Every unsafe fix says why it is unsafe.

### What a safe fix guarantees

- It is a byte-exact edit of the original file: it changes nothing it does not fix, keeps the JSON formatting the Designer wrote and the indentation of scripts.
- It does not move any line of a script (an import is replaced by an empty line, not deleted), because line numbers are visible to code through tracebacks and logged stack traces.
- After the fixes, a safety net re-checks every changed script and drops a safe fix if it changes whether the script compiles (in either direction), what any remaining name resolves to, a docstring, the `__future__` imports, or the scopes of the script; and for a view, it compares how every property reference, tag path and component lookup resolves before and after.

It does **not** guarantee that the fixed code is what you meant: a safe fix only removes something that provably has no effect (an unused import, an unused custom property, a stale suppression, trailing whitespace) or renames consistently.

### Why a fix becomes unsafe

1. **The fix changes behaviour on purpose.** Repairing a bug changes what the code does, and iglint cannot prove the author meant the repaired version. These fixes are unsafe by definition: `SCR110` (`is` to `==`), `SCR115` (adding parentheses to an `except` tuple), `SCR103` (removing an unused variable whose value may have side effects), `VIEW012` (property access modes).

2. **Unknown code exists somewhere in the workspace.** Python is dynamic. Code that reads a stack frame (`sys._getframe()`), evaluates a computed string, uses reflection, or calls a function iglint does not know can observe almost anything, including an "unused" import or custom property in another module. Seven review rounds in a row found such a route that an enumerated list missed, so iglint no longer tries to enumerate them. Instead it is **default-deny**: safe fixes assume that the code uses only vetted modules, Java classes, builtins and module attributes, plus the language constructs that iglint analyses. Everything else is unknown code, and unknown code withholds every safe fix that depends on what code can read. The fix says where the first such code is:

   ```text
   = help: Remove unused import `os` (unsafe fix available with `--unsafe-fixes`)
   = note: the fix is unsafe because closed world: code in the workspace is not vetted, reads frames, reaches other views or runs in the browser, first: `sys._getframe` at projects/demo-hmi/ignition/script-python/plant/debug/code.py:5:9
   ```

   Unknown code also includes the resources iglint does not read (Vision windows, reports, SFC charts, ...), JavaScript that runs in the browser, and functions of third-party modules, unless you list them under `[fix] vetted-system-functions` or `vetted-expression-functions`.

3. **Code may read the files a fix edits.** Every fix edits a file of the workspace. A script that reads that file, its size or its modification time sees the edit. So while any code may access a workspace file (a file operation whose path is not a literal outside the workspace), every fix is unsafe:

   ```text
   = note: the fix is unsafe because code in the workspace may read the files a fix edits (set `assume-scripts-do-not-read-workspace-files = true` under `[fix]` if no script does), first: `open(path)` at projects/demo-hmi/ignition/script-python/plant/io/code.py:2:9
   ```

   If no script reads the project files, say so with `[fix] assume-scripts-do-not-read-workspace-files = true`. That lifts only this condition.

### The closed-world assumption

The Ignition runtime is shared across every project and gateway-scoped script: tag event scripts, WebDev handlers, session and gateway events. Code that can read a frame or the interpreter anywhere in that runtime can observe the names of any module. So the safe fixes of `SCR102`, `VIEW015` and `NAME001` require the **whole workspace** to be free of unknown code: every project, its parents, the children that inherit from it, and the gateway-scoped scripts.

The workspace is treated as the whole world. That is only sound when nothing outside it can depend on what a fix changes, so:

- an **inheritable** project, or one whose inheritance chain is not complete in the workspace, gets no safe fixes of these rules, because a child project outside the workspace could use its library;
- linting one file (an editor, a pre-commit hook, `cd projects/A && iglint check --fix .`) analyses the same workspace as linting the whole repository, so the verdict does not depend on what you linted;
- there is no option to declare an inheritable project's world complete.

If the deployed gateway has projects or scripts that are not in the repository and that use the repository's projects in ways other than inheritance, the closed world is wrong, and so are the safe fixes that rely on it. Keep everything that runs on a gateway in the repository you lint, or review fixes as if they were unsafe.

The full argument, with every condition and the tests that pin it, is in [Fix safety (design notes)](/internals/fix-safety/).
