---
title: What iglint catches
description: An example-driven tour of the problems iglint finds in Ignition 8.3 projects, by area.
---

iglint's 110 rules fall into a few areas. The pages in this section show, with small invented examples, what each area catches and why it matters on a gateway. Every message shown was produced by the iglint binary on those examples.

| Area | What it catches | Categories |
|---|---|---|
| [Scripts](/checks/scripts/) | undefined names, syntax and compile errors that only show on the gateway, Python 3 syntax, wrong `system.*` calls and scopes, redefined event handlers, Jython pitfalls | `SCR` |
| [Expressions and bindings](/checks/expressions-and-bindings/) | expression syntax, unknown functions and argument counts, unresolved property references, indirect tag bindings, transforms that return nothing | `BIND`, `VIEW` |
| [References between resources](/checks/references/) | unknown views, library functions, named queries and parameters, message types, popup parameters, page URLs, style classes, parent projects, UDTs and tags | `REF`, `STYLE`, `TAG` |
| [Security, performance, accessibility](/checks/security-performance-accessibility/) | SQL built from strings, hardcoded credentials, `eval`, OS commands, fast polling, deep nesting, missing alternative text | `SEC`, `PERF`, `A11Y` |

The structure of views is checked too (`VIEW`): invalid JSON and UTF-8, duplicate keys and sibling names, unknown component types and action types, empty scripts, parameters without a direction, unused custom properties and message handlers that listen to no scope. Naming conventions are opt-in (`NAME001`).

## Where iglint looks

iglint reads the files of an Ignition 8.3 workspace:

- **Perspective**: views (`view.json`), style classes, the project stylesheet, page configuration, session properties and session, page and keyboard event scripts.
- **Scripts**: the project library (`ignition/script-python`), gateway events (startup, shutdown, update, timer, tag change, message, scheduled), WebDev handlers, Perspective session events, Vision client event scripts, and the scripts embedded in views (event actions, transforms, change scripts, message handlers, custom methods) and in tags.
- **Named queries**: their parameters and type.
- **Gateway configuration**: tag providers, tags and UDT definitions, history providers, alarm properties.
- **Projects**: `project.json` and inheritance from parent projects in the same workspace.

Each script is analysed with the names its location provides: the parameters of the header Ignition wraps around it (`self`, `event`, `value`, `payload`, ...), `system`, the project's top-level library packages and Jython's builtins.

What iglint does not read, or cannot know, is listed in [What iglint does not catch](/what-iglint-does-not-catch/).

## Severity in practice

- **error**: the code certainly fails or does nothing at run time (a `NameError`, a script that does not compile, a view that does not exist, a binding that evaluates to `null`).
- **warning**: very likely a bug or a risk (a deprecated function, SQL built from strings, an unused import, polling every second).
- **info**: worth knowing, rarely urgent (an insecure URL to an internal host, deep nesting, a disabled binding).

The full list, with each rule's default severity, is in the [rules overview](/reference/rules/).
