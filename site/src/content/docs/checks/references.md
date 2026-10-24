---
title: References between resources
description: What iglint finds across a project. Unknown views, library members, named queries and their parameters, message types, popup parameters, page URLs, style classes, parent projects, UDTs and tags.
---

Most resources of an Ignition project refer to others by name: a button opens a popup view by its path, a script calls a library function, a binding runs a named query with parameters, a tag is an instance of a UDT. Rename or move one, and the references break silently. iglint builds an index of the whole workspace (every project, its parent projects and the gateway configuration) and checks every literal reference against it.

All examples are invented; the messages are iglint's.

## Views

```python
# a button's onActionPerformed script
system.perspective.openPopup("pumpDetail", "Line/PumpDetial", params={"pumpId": 7})
```

```text
view.json:84:75: REF001 [error] View `Line/PumpDetial` does not exist
```

`REF001` checks the literal view paths of embedded views, flex repeaters, view canvases, carousels, accordions, dashboards and table subviews; popup, navigate and dock actions; the page configuration; and the literal arguments of `system.perspective.openPopup`, `togglePopup` and `navigate(view=...)` in scripts, also through aliases and local variables that only ever hold a literal.

`REF007` checks the parameters passed to a view against the parameters it declares:

```python
system.perspective.openPopup("p", "Line/PumpDetail", params={"pumpID": 7})
```

```text
view.json:200:95: REF007 [warning] `pumpID` is not a parameter of view `Line/PumpDetail`
```

`REF008` reports navigation to a page URL that the page configuration does not define.

## Library modules and functions

```python
demo.scripts.undefined_nme([])
```

```text
view.json:223:43: REF002 [error] Module `demo.scripts` has no attribute `undefined_nme`
```

`REF002` follows imports and aliases, and knows inherited libraries and child modules hidden by a parent's `code.py` (`SCR304`). It is silent for modules that are used dynamically, for uses guarded by `try`/`except AttributeError`, and for names that a descendant project defines. `REF003` checks the argument count and keywords of calls to plain library functions (see [Scripts](/checks/scripts/#wrong-argument-counts-to-library-functions)).

## Named queries

```python
rows = system.db.runNamedQuery("Pumps/ByAreaa", {"area": area})
```

```text
code.py:7:33: REF004 [error] Named query `Pumps/ByAreaa` does not exist
```

`REF004` checks query bindings and script calls (`runNamedQuery`, `execQuery`, `execUpdate` and the project-scoped and keyword forms). `REF005` checks the parameters passed against the named query's own parameters, in bindings and in dict literals of script calls:

```text
view.json:344:19: REF005 [error] Named query `Pumps/ByArea` has no parameter `zone`
view.json:350:30: REF005 [error] Query binding does not pass parameter `area` of named query `Pumps/ByArea`
```

`SEC005` warns about `QueryString` parameters, which are pasted into the SQL text.

## Messages

```python
system.perspective.sendMessage("refresh-pumpz", payload={}, scope="page")
```

```text
view.json:177:61: REF006 [warning] No component message handler listens for `refresh-pumpz`
```

`REF006` checks literal message types of `system.perspective.sendMessage` against the component message handlers of the project (and their scopes), and `system.util.sendMessage` and `sendRequest` against the gateway message handlers of the target project. `VIEW017` reports a handler that listens to no scope at all. Opt-in: `REF014` (handlers nobody sends to) and, in strict mode, `SCR306` (a handler that reads a payload key no sender sends).

## Style classes

`STYLE001` reports a style class name in a literal `classes` list (or a `style-list` map transform) that matches no style class resource, no rule of the project stylesheet and no gateway theme in the workspace. Because gateway themes usually live outside the repository, a name without a style class resource is reported only with `strict = true` when no theme is in the workspace:

```text
view.json:273:25: STYLE001 [warning] Style class `Demo/Missing` does not exist (no style class resource; gateway themes are not in the workspace)
```

Opt-in: `STYLE002` (unused style classes). `STYLE003` reports empty style classes.

## Projects and manifests

- `REF010`: `parent` in `project.json` names a project that does not exist in the workspace or is not inheritable.
- `REF009`: a `resource.json` that lists a missing file or misses a data file, or a resource directory without one.

## Tags and UDTs

When the gateway configuration is in the workspace (`config/resources/...` or `services/<gateway>/config/resources/...`), iglint reads tag providers, tags and UDT definitions:

| Rule | Reports |
|---|---|
| `TAG001` | a UDT instance whose `typeId` names no UDT definition, or cyclic UDT inheritance |
| `TAG002` | a `{Param}` in a UDT member that names no parameter |
| `TAG003` | a static tag path to a tag that does not exist (opt-in) |
| `TAG004` | a `historyProvider` that is not configured on the gateway |
| `TAG005` | an alarm property bound with a bind type alarms do not accept |
| `TAG006` | a gateway tag change script without tag paths, or with an invalid one |
| `TAG007` | a scheduled script with an invalid cron expression |

```json
{ "name": "Motor1", "tagType": "UdtInstance", "typeId": "Motors/Motr" }
```

is reported by `TAG001` because the definition is `Motors/Motor`. With `TAG003` enabled:

```python
system.tag.readBlocking(["[default]Plant/Line1/Stat"])
```

```text
code.py:2:34: TAG003 [error] Tag `[default]Plant/Line1/Stat` does not exist
```

`TAG003` checks direct tag bindings, `{[provider]...}` references and literal `tag("...")` calls in expressions, tag change script paths, and literal paths passed to `system.tag.readBlocking`, `writeBlocking`, the async variants and `queryTagHistory`. It only checks providers whose tags are in the workspace, compares case-insensitively, resolves UDT members through their definitions, and ignores relative paths, paths with placeholders, and remote or managed providers. It is opt-in because scripts can create tags at run time, and one repository may hold several gateways with providers of the same name.
