# Rule catalogue

The authoritative list of rules iglint implements. Codes and names are stable
once released. "Default" says whether the rule runs without explicit
selection. Every rule must be precise: if a rule cannot decide with certainty
(dynamic paths, unknown resources outside the linted tree), it stays silent
unless `strict` is set and the rule documents a strict mode.

Sources: `docs/compat/ign-lint.md` (parity), `docs/internal/ignition-format.md`
section 12 (lint opportunities), pyflakes/pylint error classes adapted to
Jython 2.7.

Column legend: **Sev** default severity (E error, W warning, I info), **Def**
in the default set, **Fix** S safe, U unsafe, - none, **ign-lint** the
equivalent ign-lint rule class.

## ign-lint parity

All nine ign-lint rules have an equivalent. ign-lint option names are mapped
(`ign_lint_option_map`) so a legacy `rule_config.json` configures them.

| ign-lint rule | iglint |
|---|---|
| NamePatternRule | NAME001 |
| PollingIntervalRule | BIND004 |
| PylintScriptRule | SCR900 (real pylint) + every default `SCR` rule no other ign-lint rule names (category-based, so new SCR rules join automatically) |
| BadComponentReferenceRule | VIEW018 |
| ComponentReferenceValidationRule | BIND005 (expressions/property bindings) + SCR305 (scripts) |
| UnusedCustomPropertiesRule | VIEW015 |
| ExcessiveContextDataRule | VIEW006 |
| PropertyAccessRule | VIEW012 |
| PropertyPersistenceRule | VIEW013 |

## VIEW: view structure

| Code | Name | Sev | Def | Fix | ign-lint | Summary |
|---|---|---|---|---|---|---|
| VIEW001 | invalid-json | E | yes | - | | Resource JSON does not parse. Other files still lint. |
| VIEW002 | duplicate-component-name | E | yes | - | | Siblings share `meta.name`; paths and `getChild` become ambiguous. |
| VIEW003 | unknown-component-type | W | yes | - | | `type` is not a registered Perspective component (stubs) nor from a configured module allowlist. |
| VIEW004 | unknown-component-prop | W | no | - | | Top-level `props` key not in the component schema (only for schemas that do not allow additional properties). |
| VIEW005 | invalid-position-key | W | yes | - | | `position` key not valid for the parent container type. |
| VIEW006 | excessive-context-data | W | yes | - | ExcessiveContextDataRule | Oversized arrays, breadth, depth, size or strings in view-level `custom`. Same thresholds and options. |
| VIEW008 | empty-script | E | yes | - | | Script action, transform, change script, handler or method with an empty or whitespace-only body: Ignition fails to compile it. |
| VIEW009 | disabled-change-script | I | yes | - | | `onChange.enabled: false` with code (dead code). |
| VIEW010 | unknown-action-type | E | yes | - | | Event action `type` not one Ignition knows. |
| VIEW011 | duplicate-json-key | E | yes | - | | Object has the same key twice; Ignition keeps the last. |
| VIEW012 | property-access | W | no | - | PropertyAccessRule | Same semantics as ign-lint (configuration-driven). |
| VIEW013 | property-persistence | W | no | - | PropertyPersistenceRule | Same semantics as ign-lint (configuration-driven). |
| VIEW014 | param-without-direction | W | yes | - | | View param has no `paramDirection` in `propConfig`. |
| VIEW015 | unused-custom-property | W | yes | S | UnusedCustomPropertiesRule | Custom property or view param never read anywhere it could be read (expressions, property bindings, scripts, other views' params for in/out). Exact reference matching, not substrings. |
| VIEW017 | message-handler-never-receives | W | yes | - | | Handler with page, session and view scope all false. |
| VIEW018 | fragile-component-reference | W | no | - | BadComponentReferenceRule | `getSibling`/`getParent`/`getChild`/`.parent`/`.children`, `../` relative paths. Style rule, opt-in. Parsed, not substring matched. |
| VIEW019 | output-param-never-written | I | no | - | | `output` param never assigned. |
| VIEW020 | dom-and-action-click | W | yes | - | | Button has both `dom.onClick` and `component.onActionPerformed` scripts (double firing). |

## NAME: naming

| Code | Name | Sev | Def | Fix | ign-lint | Summary |
|---|---|---|---|---|---|---|
| NAME001 | name-pattern | W | no | U | NamePatternRule | Naming conventions per node type (component, property, message handler, custom method, view param, custom property, library module/function), presets and regexes; same option structure as ign-lint. Fix renames components and updates every reference in the view (unsafe: scripts may build names dynamically). |
| NAME002 | method-shadows-component-api | E | yes | - | | Custom method name equal to a built-in component method (`refreshBinding`, `getChild`, ...). |

## BIND: bindings, transforms and expressions

| Code | Name | Sev | Def | Fix | ign-lint | Summary |
|---|---|---|---|---|---|---|
| BIND001 | expression-syntax-error | E | yes | - | | Expression does not parse (bindings, expression transforms, struct leaves, tag expressions, query params). |
| BIND002 | unknown-expression-function | E | yes | - | | Function not registered on the gateway. |
| BIND003 | expression-function-arity | E | yes | - | | Wrong number of arguments for an expression function. |
| BIND004 | polling-interval | W | yes | - | PollingIntervalRule | `now(n)` (and other polling functions) below `minimum_interval`; `now()` without argument polls at the default rate. Same options. |
| BIND005 | unresolved-property-reference | E | yes | - | ComponentReferenceValidationRule | `{this.custom.x}`, `{view.params.y}`, `{../Sibling.props.z}`, `{/root/A/B.custom.c}` or property binding path that does not resolve in the view (component and first property segment; free-form `props` subkeys are not checked). |
| BIND006 | indirect-tag-reference-mismatch | E | yes | - | | Indirect tag binding placeholders `{name}` and `references` keys differ. |
| BIND007 | placeholder-in-direct-tag-path | W | yes | - | | Direct tag binding path contains `{...}`. |
| BIND008 | disabled-binding | I | yes | - | | Binding with `enabled: false` left in place. |
| BIND009 | transform-without-return | E | yes | - | | Script transform whose body can finish without `return` (binding becomes null). |
| BIND010 | map-transform-duplicate-input | W | yes | - | | Map transform maps the same input twice. |
| BIND011 | expression-type-mismatch | W | no | - | | Obvious literal type errors (`"a" * 2`, comparing with null via `=`). |

## SCR: script correctness (Jython 2.7)

Applies to every script: library modules, Perspective scripts, gateway/session
events, WebDev, tag event scripts. Each script is analysed with the names its
location provides (header parameters, `system`, top-level library packages,
Jython builtins).

| Code | Name | Sev | Def | Fix | ign-lint | Summary |
|---|---|---|---|---|---|---|
| SCR001 | syntax-error | E | yes | - | (pylint E0001) | Does not parse with Jython 2.7 (Python 3-only syntax gets a specific hint). |
| SCR002 | compile-error | E | yes | - | | Parses but Jython refuses to compile: `return`/`yield` outside function, `break`/`continue` outside loop, duplicate argument, `return` with value in generator, `None` as target, `import *` in function with free vars, `default except` not last. |
| SCR101 | undefined-name | E | yes | - | (E0602) | Name never bound in any enclosing scope, not a builtin, not injected. Suppressed when the scope has `import *` or unqualified `exec`. |
| SCR102 | unused-import | W | yes | S | (W0611) | Import never used (not in `__init__`-like modules; `__all__` respected). |
| SCR103 | unused-variable | W | yes | U | (W0612) | Local assigned but never read (tuple unpacking and `_`-prefixed names excluded). |
| SCR104 | redefined-unused | W | yes | - | (W0404/E0102) | Function/class/import redefined before use. |
| SCR105 | unreachable-code | W | yes | - | (W0101) | Statements after `return`/`raise`/`break`/`continue`. |
| SCR106 | bare-except | W | yes | - | (W0702) | `except:` also catches Java exceptions and `SystemExit`; prefer explicit types. |
| SCR107 | mutable-default-argument | W | yes | - | (W0102) | `def f(x=[])`. |
| SCR108 | duplicate-dict-key | W | yes | - | (W0109) | Literal dict with the same constant key twice. |
| SCR109 | self-assignment | W | yes | - | (W0127) | `x = x`. |
| SCR110 | comparison-with-literal-identity | W | yes | S | (R0123) | `x is 'a'`, `x is 1`. |
| SCR111 | assert-on-tuple | W | yes | - | (W0199) | `assert (x, "msg")` is always true. |
| SCR112 | format-string-mismatch | E | yes | - | (E1305/E1306) | `%` formatting with wrong argument count or bad format; `str.format` with missing positional/keyword. |
| SCR113 | except-order | W | yes | - | (E0701) | Broader exception class caught before a subclass (builtin hierarchy). |
| SCR114 | pointless-statement | W | yes | - | (W0104/W0105) | Expression statement with no effect (not docstrings, not calls). |
| SCR115 | except-tuple-without-parentheses | E | yes | S | | `except ValueError, TypeError:` binds `TypeError` as the variable; it does not catch `TypeError`. |
| SCR116 | raise-string | E | yes | - | | `raise "message"` fails at run time. |
| SCR117 | redefined-builtin | I | no | - | (W0622) | Local name shadows a builtin. |
| SCR118 | used-before-assignment | E | yes | - | (E0601) | Local read before any assignment on every path (flow-sensitive, conservative). |
| SCR119 | return-in-finally | W | yes | - | (W0150) | `return`/`break` in `finally` swallows exceptions. |
| SCR120 | dangerous-loop-closure | W | no | - | (W0640) | Lambda/def in loop captures the loop variable. |
| SCR121 | global-at-module-level | I | yes | - | (W0604) | `global` at module level does nothing. |
| SCR122 | print-statement | I | no | - | | `print` in scripts that run on the gateway goes to the wrapper log; use `system.util.getLogger`. |
| SCR123 | unused-suppression | I | yes | S | | `# iglint: ignore[...]` that suppresses nothing. |

### System API (`system.*`)

| Code | Name | Sev | Def | Fix | Summary |
|---|---|---|---|---|---|
| SCR201 | unknown-system-function | E | yes | - | `system.x.y` (or `fpmi.x.y`, through aliases) does not exist in Ignition 8.3 (any scope). Third-party modules add packages and members, so by default only near misses of a known member are reported (with a suggestion); `strict` reports every unknown member of a known package; option `allow` (wildcards) lists module additions. |
| SCR202 | system-function-arity | E | yes | - | Arguments match no overload in the script's scope (count, missing measured-required parameter, duplicate for Python functions). Skips `*args`/`**kwargs`, unchecked and array-ending Java overloads; library modules only when the call fails in every scope. |
| SCR203 | unknown-system-keyword | E | yes | - | Keyword argument the function does not accept: keywords to a plain Java function, unknown names for Python functions. Unknown keywords of `@KeywordArgs` Java functions are not reported (`PyArgumentMap` ignores them; the stubs do not say which parser a function uses). Same exclusions as SCR202. |
| SCR204 | deprecated-system-function | W | yes | - | Deprecated function in the script's scope (library modules: in every scope), with replacement when known. |
| SCR205 | wrong-scope-system-function | E | yes | - | Member not available in the scope where the script runs (`system.gui.*` in Perspective, gateway-only functions in Vision Client scripts), and `system.perspective.*` functions with a `sessionId` parameter called without it in gateway events, WebDev and tag scripts (no current session). Library modules are not reported. |
| SCR206 | legacy-script-prefix | E | yes | U | `project.`, `shared.` or `app.` prefixes (unbound) do not exist in 8.3. Fix removes the prefix when the next name is a top-level library package of the project. |

### Ignition script context

| Code | Name | Sev | Def | Fix | Summary |
|---|---|---|---|---|---|
| SCR301 | invalid-event-script-header | E | yes | U | Gateway/session/WebDev `.py` whose first line is not the canonical header (Ignition drops/rewraps it): other parameters, one-line body, wrapped header, top-level code without header. Whitespace, trailing comments and defaults on the canonical parameters are not reported; indented header-less (legacy body) files compile and are not reported. |
| SCR302 | sleep-in-event-script | W | yes | - | `time.sleep`/`Thread.sleep` directly in a Perspective event action, transform, change script, message handler or extension function (not in nested defs/lambdas; custom methods excluded: callers may run them in the background). |
| SCR303 | module-level-side-effect | W | yes | - | Project library module runs `system.*` functions, `open`/`file`/`execfile` or sleeps at import time (module level, class bodies, decorators, defaults; not under `if __name__ == '__main__':`). Documented allowlist of benign calls. |
| SCR304 | shadowed-library-package | E | yes | - | Library folder with its own `code.py` hides its sub-modules (with inheritance); reported on that `code.py`. |
| SCR305 | unresolved-component-reference | E | yes | - | `getSibling('X')`, `getChild('A/B')`, `self.view.getChild('root/...')` with literal names that match no component, following `self`, `.parent`, `.view`, `.rootContainer` and aliases. Lookups below non-containers (embedded views) and view paths not starting at the root are not checked. |
| SCR306 | message-handler-payload-misuse | W | no | - | Strict only: handler reads `payload['k']` that no sender passes, when every send of the type is a literal dict payload. |

### External

| Code | Name | Sev | Def | Fix | ign-lint | Summary |
|---|---|---|---|---|---|---|
| SCR900 | pylint | W | no | - | PylintScriptRule | Runs real pylint once per run (one process, `jobs` workers, default one per CPU) over every linted script, each written as its own module (`source`: generated header + body, as Ignition compiles it). Messages map back to exact file ranges through the source map (AST columns are UTF-8 bytes, token columns characters); header messages go to the script anchor, except naming/signature messages about Ignition's generated header (dropped; custom method headers are the user's). Dropped as wrong for Ignition: `undefined-variable`/`import-error` for injected names, library packages and Jython builtins (all `undefined-variable` when the library is unknown, like SCR101), and `syntax-error` when Jython parses the script (or SCR001 is on). Options `pylint` (command), `pylintrc` (default: bundled ign-lint rcfile), `jobs`, `category-mapping` (replaces F/E error, W/C/R warning). `# iglint: ignore[SCR900]` applies. pylint missing or failing: one run warning, other rules unaffected, no cache entries for the affected files. Fixes of other rules move its findings without re-running pylint. |

## REF: references between resources

| Code | Name | Sev | Def | Fix | Summary |
|---|---|---|---|---|---|
| REF001 | unknown-view | E | yes | - | Embedded view, repeater, popup/dock/navigate action, page config or literal script argument names a view that does not exist in the project (with inheritance). |
| REF002 | unknown-library-member | E | yes | - | `pkg.mod.func` (through aliases and imports) where the module or attribute does not exist in the effective library, or a child module hidden by its parent's `code.py`. Silent for dynamic modules, guarded uses (`try`/`except AttributeError`), and names a descendant project defines. |
| REF003 | library-function-arity | E | yes | - | Call to a library function with an impossible argument count, unknown keyword or duplicate argument. Only plain functions defined once and undecorated. |
| REF004 | unknown-named-query | E | yes | - | Query binding or script call (`runNamedQuery`, project-scoped and keyword forms, `execQuery`, `execUpdate`, ...) with a literal path that does not exist. |
| REF005 | named-query-parameter-mismatch | E | yes | - | Missing or unknown parameters for a named query (bindings, and dict literals in script calls). |
| REF006 | unknown-message-type | W | yes | - | `system.perspective.sendMessage` with a literal type no component handler listens for (or none on the call's scope); `system.util.sendMessage`/`sendRequest` to a handler the target project does not have. |
| REF007 | popup-param-mismatch | W | yes | - | Popup/embedded view params (also dict literals of script `openPopup`/`navigate`) that are not params of the target view. |
| REF008 | unknown-page-url | W | yes | - | Navigate (action or script) to a URL not in page config. |
| REF009 | resource-manifest-mismatch | W | yes | - | `resource.json` lists a missing file or misses a data file. |
| REF010 | unknown-parent-project | E | yes | - | `parent` not found or not inheritable. |
| REF011 | unused-view | I | no | - | View not referenced anywhere (bindings, actions, page config, scripts). Paths built at run time count by literal prefix. |
| REF012 | unused-library-function | I | no | - | Library function never referenced by scripts or `runScript`. Silent for modules used as values or star-imported, and for projects with dynamic code. |
| REF013 | unused-named-query | I | no | - | Named query never referenced by bindings or scripts. Paths built at run time count by literal prefix. |
| REF014 | unhandled-message-handler | I | no | - | Component, gateway, session or client handler whose type no script sends (computed types count by prefix). |

## PERF

| Code | Name | Sev | Def | Fix | Summary |
|---|---|---|---|---|---|
| PERF001 | deep-component-nesting | I | yes | - | Nesting deeper than `max_depth` (default 12). |
| PERF002 | fast-query-polling | W | yes | - | Query or tag-history binding polling faster than `minimum_interval`. |
| PERF003 | hidden-component-bindings | I | no | - | `meta.visible: false` with bindings. |
| PERF004 | large-view | I | no | - | More than `max_components` components. |

## SEC

| Code | Name | Sev | Def | Fix | Summary |
|---|---|---|---|---|---|
| SEC001 | sql-string-building | W | yes | - | SQL text passed to `system.db.runQuery`/`runUpdateQuery`/`runScalarQuery` (and the query text of `runPrepQuery`/`runPrepUpdate`/`runScalarPrepQuery`) built with `%`, `+`, `format` from non-literals; follows aliases and plain assignments. |
| SEC002 | hardcoded-credential | W | yes | - | Literal password/token/secret/API key in scripts (credential-named names, attributes, dict keys, keyword args, `password` parameters of `system.*` calls), HTTP bindings (auth, `Authorization` headers) and connection calls; placeholders and field names skipped. |
| SEC003 | eval-exec | W | yes | - | Builtin `eval`/`exec`/`execfile`/`compile` (not shadowed) on non-literal input. |
| SEC004 | insecure-url | I | yes | - | Literal `http://` URLs in HTTP bindings, `system.net` calls and `system.net.httpClient()` methods (except localhost, 127.0.0.0/8, ::1). |
| SEC005 | querystring-parameter | W | yes | - | Named query `QueryString` parameter (injection risk). |
| SEC006 | os-command | W | yes | - | `os.system`/`popen`/`exec*`/`spawn*`, `commands`, `subprocess`, `Runtime.exec`, `ProcessBuilder`, resolved through imports and aliases. |

## A11Y

| Code | Name | Sev | Def | Fix | Summary |
|---|---|---|---|---|---|
| A11Y001 | image-without-alt | W | yes | - | Image component without `alt` text. |
| A11Y002 | icon-button-without-text | W | no | - | Button or icon with an action and no text or title. |

## STYLE

| Code | Name | Sev | Def | Fix | Summary |
|---|---|---|---|---|---|
| STYLE001 | unknown-style-class | W | yes | - | `props.style.classes` (static) or map output names a missing style class. |
| STYLE002 | unused-style-class | I | no | - | Style class never referenced. |
| STYLE003 | empty-style-class | I | yes | - | Style class with no styles. |

## TAG: tags and gateway resources

| Code | Name | Sev | Def | Fix | Summary |
|---|---|---|---|---|---|
| TAG001 | unknown-udt-type | E | yes | - | `typeId` does not resolve to a UDT definition; inheritance cycles. |
| TAG002 | undefined-udt-parameter | E | yes | - | `{Param}` in a UDT member that is not a parameter. |
| TAG003 | unknown-tag-path | E | no | - | Static tag path (bindings, scripts with literal paths, tag change scripts, expressions) to a provider whose tags are in the linted tree, where the tag does not exist. Opt-in: scripts create tags at run time and one repository may hold several gateways with same-named providers. |
| TAG004 | unknown-history-provider | W | yes | - | `historyProvider` not configured on the gateway. |
| TAG005 | invalid-alarm-bind-type | E | yes | - | Alarm property bound with `parameter` or another type alarms reject. |
| TAG006 | tag-change-script-paths | E | yes | - | Gateway tag change script with no paths or invalid tag path syntax. |
| TAG007 | invalid-cron-expression | E | yes | - | Scheduled script with an invalid cron expression. |
