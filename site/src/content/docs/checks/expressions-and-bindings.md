---
title: Expressions and bindings
description: What iglint finds in Perspective bindings, transforms and Ignition expressions. Syntax errors, unknown functions, argument counts, unresolved property references and tag binding mistakes.
---

Bindings are where a Perspective view gets its data, and a broken binding fails quietly: the property shows `null` or an error overlay, and nothing is written to a log you would look at. iglint parses every expression with a parser tested against Ignition's own expression parser, and resolves every property reference against the view.

Expressions are checked wherever they occur: expression and expression-structure bindings, tag bindings in expression mode, query binding parameters, expression transforms, map transforms, session properties, expression tags, derived tags and bound tag and alarm properties.

All examples are invented; the messages are iglint's.

## Syntax errors

```text
if({view.custom.count} > 0, "some", )
```

```text
view.json:31:70: BIND001 [error] Expression syntax error: unexpected `)`, expected an expression
```

The position points into the JSON string, at the offending character.

## Unknown functions and wrong argument counts

The list of expression functions, and their parameters, was generated from a real 8.3 gateway.

```text
toUpper({view.custom.name})
dateFormat(now(0))
```

```text
view.json:53:32: BIND002 [error] Unknown expression function `toUpper` (did you mean `upper`?)
view.json:75:32: BIND003 [error] `dateFormat` does not accept 1 argument (parameters: date, pattern)
```

Functions of third-party modules can be declared with `BIND002`'s `extra-functions` option.

## Property references that do not resolve

A reference to a property that does not exist evaluates to `null` with a bad quality. `BIND005` resolves every `{...}` reference and every property binding path: `{this.custom.x}`, `{view.params.y}`, `{../Sibling.props.z}`, `{/root/A/B.custom.c}` and the paths of property bindings.

```text
"Area " + {view.custom.aera}
```

```text
view.json:34:57: BIND005 [error] `view.custom.aera`: custom property `aera` is not defined
   = note: at root/Title.props.text@binding
```

The component and the first property segment are checked. Free-form keys below `props` (whatever a component accepts) are not, and neither are references built at run time.

## Polling

`now()` without an argument polls every second, in the gateway, for every open session.

```text
dateFormat(now(1000), "HH:mm:ss")
```

```text
view.json:56:43: BIND004 [warning] `now` polls every 1000 ms, faster than the minimum of 10000 ms
```

The minimum is the rule's `minimum-interval` option (10,000 ms by default). `runScript` with a literal poll rate is checked the same way. Query, tag history and HTTP bindings that poll too often are `PERF002`'s concern (see [performance](/checks/security-performance-accessibility/#performance)).

## Tag bindings

An indirect tag binding replaces `{name}` placeholders in the tag path with the values of its `references`. When the two do not match, the path is never what the author meant:

```json
{
  "type": "tag",
  "config": {
    "mode": "indirect",
    "tagPath": "[default]{area}/Speed",
    "references": { "line": "{view.custom.name}" }
  }
}
```

```text
view.json:100:19: BIND006 [error] Reference `line` is not used by any `{line}` placeholder of the tag path
view.json:102:39: BIND006 [error] Placeholder `{area}` has no entry in `references`; it is never replaced
```

`BIND007` reports a direct tag binding whose path contains `{...}`, which a direct binding never replaces. With `TAG003` enabled, static tag paths are checked against the tags of the workspace (see [References between resources](/checks/references/#tags-and-udts)).

## Transforms

```json
{
  "type": "property",
  "config": { "path": "view.custom.count" },
  "transforms": [
    { "type": "script", "code": "\tif value > 10:\n\t\treturn \"high\"" }
  ]
}
```

```text
view.json:128:30: BIND009 [error] Script transform can finish without `return`; the property then becomes null
```

Script transforms are also linted as scripts, with the header `def transform(self, value, quality, timestamp):`, so every `SCR` rule applies to them. `BIND010` reports a map transform that maps the same input twice.

## Leftovers

| Rule | Reports |
|---|---|
| `BIND008` | a binding disabled with `enabled: false` and left in place (info) |
| `VIEW009` | a property change script that is disabled but has code (info) |
| `VIEW015` | a custom property or view parameter that is never read anywhere it could be read: expressions, property bindings, scripts, and other views' parameters for input and output parameters |
| `VIEW019` | an output parameter that is never written (opt-in) |
| `BIND011` | obvious literal type errors such as `"a" * 2` (opt-in) |

`VIEW015` is one of the rules ign-lint has too, but iglint matches references exactly (`{view.custom.foobar}` is not a use of `foo`), treats script assignments as writes, not reads, and stays silent when a script uses a property container in a way it cannot follow. Its fix removes the property and its `propConfig` entry.
