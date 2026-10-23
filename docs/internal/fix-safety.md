# Fix safety

A fix is **safe** (applied by `--fix`) only when it cannot change what the
project does in Ignition 8.3 / Jython 2.7, with the information iglint has.
When safety cannot be proven, the fix is **unsafe** (applied only with
`--unsafe-fixes`) or not offered. A fix whose purpose is to change
behaviour (to repair a bug) is unsafe by definition: iglint cannot prove the
author meant the repaired version.

Things that can make an innocent-looking edit change behaviour, checked for
every fix below:

* class scope: names bound in a class body are attributes (`self.os`,
  `C.os`);
* module attributes: top-level names of a library module are used by other
  resources as `pkg.mod.name`, statically (project index) or dynamically
  (`getattr`, module used as a value, `runScript` with a computed name);
* `__all__`, and dynamic access inside the script: `globals()`, `locals()`,
  `vars()`, `dir()`, `eval`, `execfile`, `exec`, `sys.modules`,
  `sys._getframe`, `__dict__`, `func_globals`;
* side effects of imports: a project library module runs its top level,
  Java classes run static initializers, some standard modules start
  threads or configure logging, dotted imports make a submodule an
  attribute of its package for every other script;
* component references from outside a view: library functions that receive
  a component, message payloads, `system.util.getGlobals()`;
* string-built names (`getChild('Btn' + n)`, `meta.name` comparisons);
* Perspective run-time semantics: property access modes, persistence and
  stored values, bindings and change scripts that run on their own.

Regression tests named after each condition live next to the rule. Script
behaviour was checked on IA's Jython (Ignition 8.3.3 image) by running the
code before and after the edit.

| Rule | Fix | Safe when | Otherwise |
|---|---|---|---|
| `NAME001` | Rename a component and every reference to it in the view | No reference to the component at all; no script reads `meta.name`; no script looks children up with a computed name or has a string literal equal to the name; every script parses; and no script of the view hands a component to code outside the view (library call, message payload, `getGlobals()`, an opaque call such as `locals()`): a library function that receives a component can call `getChild('Name')` itself | Unsafe when references exist or a component may escape; no fix when a lookup cannot be rewritten or the new name is taken |
| `VIEW012` | Set, add or remove `access` in `propConfig` | Never: the access mode decides whether the browser receives the property and whether it may write it | Unsafe |
| `VIEW013` | Set `persistent: false` (and remove the stored value) | No stored value: the property starts out absent either way | Unsafe when a stored value is removed: scripts and expressions that run before the binding delivers (query bindings are asynchronous) see a different value |
| `VIEW015` | Remove an unused custom property and its `propConfig` entries | Nothing reads or writes the property: resolved reads and writes cover expressions, bindings (bidirectional ones write) and scripts, including the library functions scripts hand `self` to (followed through their bodies; anything that cannot be followed counts as reading and writing everything: a value stored where it outlives the call (a `global`, a module or class attribute, a default argument, a container the function does not own), returned, yielded, raised, passed to unknown code, captured by a nested function or lambda, or reachable through a stack frame (`sys._getframe()`, `inspect`, `f_locals`), and a function that may be replaced at run time: by its module through `exec`, `eval`, `execfile`, `globals()`, `sys.modules`, `func_code`, a later definition or star import, or by any script through a store, `setattr`, `__dict__`, or the module escaping as a value); properties with a binding or change script are never reported, so nothing that runs is removed | Unsafe when a script, a library function it calls or a bidirectional binding writes the property: the write recreates it at run time without the `access` and `persistent` settings of the removed `propConfig` entry |
| `SCR102` | Remove unused import names or statements | Not in a class body (never reported); not a module-level name the module may read dynamically (never reported); a top-level library import is provably unused by other resources; and every removed import is effect-free: `java.*` / `javax.*`, or a plain `import x` / non-package `from x import y` of a standard module on the verified list, with no project library package of that name | Unsafe (project library modules, third-party and non-JDK Java packages, `socket`, `logging`, dotted imports, relative imports, unknown project) |
| `SCR103` | Remove `x = value`, or only `x = ` when the value calls or binds something | Never | Unsafe: the value may be wanted under another name, and a removed attribute or subscript read, operator, comparison, `in` or `%` formatting can raise or run code (getters, special methods). The statement is removed only when the value calls nothing (no call, no backticks, which call `repr`, no `yield`) and binds nothing (no list comprehension, whose variables Python 2 leaks), so no pointless statement (`SCR114`) is left |
| `SCR110` | `is` to `==`, `is not` to `!=` | Never: the result changes whenever identity and equality differ (`int('1000') is 1000` is `False` in Jython), and `==` calls `__eq__` / Java `equals` | Unsafe |
| `SCR115` | `except A, B:` to `except (A, B):` | Never: the handler then catches `B` and no longer rebinds the name | Unsafe. The finding itself requires `B` to resolve (ignoring the handler) to a class statement, a capitalized `from` import or a builtin exception, and the handler body not to read it |
| `SCR123` | Remove unused suppression codes or the whole comment | Only codes are removed (the rest of the comment stays), or the comment holds nothing but the suppression. Comments do not run | Unsafe when the whole comment would go and it also holds a reason, a `# pylint:` directive or a `coding:` declaration |
| `SCR124` | Remove trailing whitespace | The script parses, the whitespace is outside string literals and outside the generated header (a backslash continuation followed by spaces is a syntax error, so it is never touched) | No finding when the script does not parse |
| `SCR206` | Remove a `shared.` / `project.` prefix | Never: in Ignition 7 the two prefixes named different libraries | Unsafe |
| `SCR301` | Rewrite or insert the event script header | Never: code may rely on the written parameters or defaults | Unsafe |
| `SCR900` | ign-lint's trailing-whitespace fix (every line `rstrip()`ed) | The script parses and no removed whitespace is inside a string literal (multi-line strings) | Unsafe |

Rules not listed offer no fix. The language server uses the same
applicability: unsafe code actions are titled `(unsafe)`, and the
fix-all action applies unsafe fixes only when configured to.

## Decisions

* **What "calls nothing" means.** `SCR103` and `SCR114` share one
  syntactic predicate (`has_effect` in `pointless_statement.rs`): an
  expression has an effect when it contains a call, backticks (they call
  `repr`), `yield` or a list comprehension (it binds its variables in the
  enclosing scope). Operators, comparisons, `in`, `%` formatting and
  attribute and subscript reads count as effect-free even though, on
  arbitrary objects, they run special methods, properties or Java getters
  and can raise: treating them as calls would make every expression
  effectful. That is why the `SCR103` fix that removes them is unsafe.

* **Import side effects.** Removing an import is safe only for the JDK and
  for standard library modules verified on Jython not to start threads,
  register handlers or print (the list is `PURE_STDLIB_MODULES` in
  `unused_import.rs`). Project library modules are never safe to remove:
  importing them runs their top level, which in Ignition often registers
  loggers, caches or message handlers. The one remaining change of a safe
  removal: an import that would have failed (a class missing from the JDK)
  no longer fails, which only matters to code that relied on the
  `ImportError` without catching it where it was raised.
* **Unknown context is unsafe.** A file linted outside its project, an
  inheritable project, an incomplete parent chain, or resources whose
  scripts iglint does not model make cross-resource facts unknown; fixes
  that depend on them are unsafe.
* **`VIEW015`.** Removing a property that a script only writes is not
  neutral when its `propConfig` entry sets `access` (the property recreated
  by the write would be `PUBLIC`), so the fix is unsafe for written
  properties. A property that might be read dynamically is never reported
  (dynamic reads count as reading every property they could reach), so
  the fix is safe only when the property is provably unread and
  unwritten. Library calls are resolved in the project the view belongs
  to and in every project inheriting from it; a function a child project
  overrides counts as reading everything.
