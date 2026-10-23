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
* `__all__`, and dynamic access inside the script: any of the three
  introspection routes of the "Threat model" section below (frame,
  traceback, code, generator and function attributes named in source or in
  a string literal; reflective accessors with a computed name; the
  interpreter and reflection APIs), which reach a caller's locals, a
  module's namespace, a function's identity or a source line number;
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

## Threat model: frame and introspection access

Several fixes are safe only when a script cannot observe something an
edit changes: a caller's locals (a renamed component, a property), a
module's namespace (a removed import), the identity of a function, or a
source line number (a deleted or inserted line). Patching one dynamic
route at a time has failed five review rounds in a row, each a new route.
This section is a *closed argument*: in Jython 2.7 (Ignition 8.3) code can
observe those things in exactly three ways, detection
(`iglint_python_semantic::dynamic`) covers every one, and a module that
takes any route is treated as reading its locals and writing its globals
in every scope (it **fails closed**). The three routes:

1. **Naming an introspection attribute in source.** Frames, tracebacks,
   code, generator and function objects expose their namespaces, their
   identity and their line numbers only through named attributes
   (`f_locals`, `f_globals`, `f_back`, `f_lineno`, `f_code`, `tb_frame`,
   `tb_next`, `tb_lineno`, `gi_frame`, `func_globals`, `func_code`,
   `__globals__`, `__code__`, every `co_*`, `__dict__`,
   `__getattribute__`, ...). The attribute can appear as an attribute
   access (`tb.tb_frame`) or as any string literal, including a dotted or
   sub-segment of one (`getattr(tb, 'tb_frame')`,
   `attrgetter('tb_frame.f_back.f_locals')`,
   `tb.__getattribute__('tb_frame')`). Every string literal of the module
   is scanned, matching whole identifiers / dotted segments only, so
   ordinary prose does not trigger.
2. **A reflective accessor with a computed name.** `getattr`, `setattr`,
   `delattr`, `hasattr`, `attrgetter`, `methodcaller`, `__getattribute__`
   (and `__dict__` subscripts, covered by route 1) whose attribute name is
   not a string literal: the attribute read is unknown, so it may be an
   introspection attribute on a frame, module or function.
3. **An interpreter or reflection API.** `eval`, `exec`, `execfile`,
   `compile`, `locals`, `globals`, `vars`, `dir`, `input`, `__import__`;
   the builtins module and the module-handing modules (`__builtin__`,
   `importlib`, `imp`); the frame-, module- and trace-reaching functions
   of `sys`, `inspect` and `gc`, plus `sys.exc_info` and the `traceback`
   module (they hand out tracebacks and render line numbers); and Java /
   Jython reflection (any import from `org.python.*`, `java.lang.reflect`,
   `java.lang.Class`, `java.lang.invoke`, `javax.script`, and
   `.getClass()` / `Class.forName`). A frame builtin used under another
   name reads the frame of whoever calls it, across modules (`ev = eval`
   in one module, `m.ev('x')` in another), so a frame builtin used as a
   value or handed out of a module is an escape for every scope.

The callee check applies the same argument across resources: a library
function or module that takes any route is not "plain" (not a bare `def`,
class or literal constant that a rule may trust), so a script that calls
it, or imports from it, reads and writes everything it could reach. Each
route is verified against IA's Jython with the oracle, and the precision
cost of the string-literal scan and the reflective-accessor rule is
measured on the private corpus.

### Line numbers

Line numbers are observable **behaviour** when code reads them into a
value: `tb_lineno`, `f_lineno`, `co_firstlineno` (route 1), and the
`traceback` module, `sys.exc_info` and `inspect` (route 3), including
functions that render a traceback into a string (`traceback.format_exc`).
Detection of those routes makes a module line-number sensitive, so every
fix that deletes or inserts source lines (`SCR123` whole-line removal,
`SCR102` import-line removal, `SCR103`, `SCR124` only touches whitespace,
`SCR301` inserts a header) is unsafe in such a module. The gateway's log
of an **uncaught** exception is diagnostic output, not a value the program
can read; a fix may move the line a traceback would print there. That is a
deliberate decision: diagnostic log text is not behaviour.

| Rule | Fix | Safe when | Otherwise |
|---|---|---|---|
| `NAME001` | Rename a component and every reference to it in the view | No reference to the component at all; no script reads `meta.name`; no script looks children up with a computed name or has a string literal equal to the name; every script parses; and no script of the view hands a component to code outside the view (library call, message payload, `getGlobals()`, an opaque call such as `locals()`), reaches a frame builtin in any way, calls an unnamed value or calls a library name that is not a plain function, class or literal constant: a library function that receives a component, or `eval` under another name, can call `getChild('Name')` itself | Unsafe when references exist or a component may escape; no fix when a lookup cannot be rewritten or the new name is taken |
| `VIEW012` | Set, add or remove `access` in `propConfig` | Never: the access mode decides whether the browser receives the property and whether it may write it | Unsafe |
| `VIEW013` | Set `persistent: false` (and remove the stored value) | No stored value: the property starts out absent either way | Unsafe when a stored value is removed: scripts and expressions that run before the binding delivers (query bindings are asynchronous) see a different value |
| `VIEW015` | Remove an unused custom property and its `propConfig` entries | Nothing reads or writes the property, and the view's own scripts prove it: resolved reads and writes cover expressions, bindings (bidirectional ones write) and scripts, analysed default-deny (every use of a component, the view, a page or session, a property container or an alias of one must be one of the forms listed under "`VIEW015`: understood forms" below; anything else reads and writes everything it reaches); a script that reads frames or reaches a frame builtin in any way reads everything, and every library name a script calls or uses as a value is checked to be a plain function, class or literal constant (or missing); properties with a binding or change script are never reported | Unsafe when a script, a library function it calls or a bidirectional binding writes the property (the write recreates it at run time without the `access` and `persistent` settings of the removed `propConfig` entry); unsafe when the finding relies on following a value into library functions (inter-procedural: the function's body, a function that does not exist, the depth limit); unsafe for a view linted outside its project whose scripts call names that may be library functions |
| `SCR102` | Remove unused import names or statements | Not in a class body (never reported); not a module-level name the module may read dynamically (never reported); a top-level library import is provably unused by other resources; and every removed import is effect-free: `java.*` / `javax.*`, or a plain `import x` / non-package `from x import y` of a standard module on the verified list, with no project library package of that name; and the module calls no library name that may be a frame builtin | Unsafe (project library modules, third-party and non-JDK Java packages, `socket`, `logging`, dotted imports, relative imports, unknown project, a library name that is not a plain function, class or literal constant) |
| `SCR103` | Remove `x = value`, or only `x = ` when the value calls or binds something | Never | Unsafe: the value may be wanted under another name, and a removed attribute or subscript read, operator, comparison, `in` or `%` formatting can raise or run code (getters, special methods). The statement is removed only when the value calls nothing (no call, no backticks, which call `repr`, no `yield`) and binds nothing (no list comprehension, whose variables Python 2 leaks), so no pointless statement (`SCR114`) is left |
| `SCR110` | `is` to `==`, `is not` to `!=` | Never: the result changes whenever identity and equality differ (`int('1000') is 1000` is `False` in Jython), and `==` calls `__eq__` / Java `equals` | Unsafe |
| `SCR115` | `except A, B:` to `except (A, B):` | Never: the handler then catches `B` and no longer rebinds the name | Unsafe. The finding itself requires `B` to resolve (ignoring the handler) to a class statement, a capitalized `from` import or a builtin exception, and the handler body not to read it |
| `SCR123` | Remove unused suppression codes or the whole comment | Only codes are removed (the rest of the comment stays), or the comment holds nothing but the suppression. Comments do not run | Unsafe when the whole comment would go and it also holds a reason, a `# pylint:` directive or a `coding:` declaration, or when a whole line goes in a script that reads frames (line numbers move) |
| `SCR124` | Remove trailing whitespace | The script parses, the whitespace is outside string literals and outside the generated header (a backslash continuation followed by spaces is a syntax error, so it is never touched) | No finding when the script does not parse |
| `SCR206` | Remove a `shared.` / `project.` prefix | Never: in Ignition 7 the two prefixes named different libraries | Unsafe |
| `SCR301` | Rewrite or insert the event script header | Never: code may rely on the written parameters or defaults | Unsafe |
| `SCR900` | ign-lint's trailing-whitespace fix (every line `rstrip()`ed) | The script parses and no removed whitespace is inside a string literal (multi-line strings) | Unsafe |

Rules not listed offer no fix. The language server uses the same
applicability: unsafe code actions are titled `(unsafe)`, and the
fix-all action applies unsafe fixes only when configured to.

## Dynamic code: audit of every safe fix

Code that reads names without writing them can invalidate a fix. The
routes are the three of the "Threat model" section: introspection
attributes named in source or in a string literal (`.f_locals`,
`.tb_frame`, `'tb_frame.f_back'`, `co_*`, ...), reflective accessors with
a computed name (`getattr(o, n)`, `attrgetter(n)`), and the interpreter
and reflection APIs (`eval` / `locals` reached under any name such as
`e = eval` or a library variable holding one, `sys._getframe()`,
`inspect`, `traceback`, `sys.exc_info`, `org.python.*`, `.getClass()`),
plus functions whose code is replaced (`f.func_code = ...`, also through
an alias or a helper). Where they matter, the guards are shared:
`iglint_python_semantic::dynamic` (escape routes, which make every scope
of a module read and write names dynamically),
`script_facts::access` (default-deny accesses of view scripts, with
callee checks of library names), and
`unused_custom_property::{resolve_callee_checks, may_read_frames,
library_names_may_read_frames}` (callee checks resolved through the
project index: a library name that is not a plain function, class or
literal constant may hold `eval`; outside a project every such name
counts). Each rule has a regression test with the patterns.

| Rule | What dynamic code could do | Guard |
|---|---|---|
| `NAME001` | look the old name up (`eval("self.getChild('Old')")`, a library function receiving `self` whose code is replaced, `getattr(self, n)`) | Unsafe when a component may escape: accesses read everything for frame routes, `exec`, values passed to unknown code, calls of unnamed values, and callee checks that do not resolve to plain functions (`dynamic_code_makes_the_rename_unsafe`) |
| `VIEW013` | nothing: scripts cannot see the `persistent` flag, and without a stored value the property starts out absent either way | None needed (`dynamic_scripts_do_not_change_the_fix`) |
| `VIEW015` | read or write any property | Default-deny accesses, frame routes, callee checks, rebinding through exposure and `func_globals`; inter-procedural findings unsafe (`dynamic_patterns_read_everything` and the tests above) |
| `SCR102` | read the import by name (`lib.util.ev('os')`, `capture()['os']`, `getattr(me, n)` on the module itself) | Scopes with escape routes or frame builtins are dynamic (no finding); a computed `getattr` on a module object keeps module imports; a library name that may be a frame builtin makes the fix unsafe (`library_frame_builtins_and_computed_getattr`, `frame_builtins_under_other_names_keep_imports`) |
| `SCR123` | see line numbers move when a whole comment line goes (`f_lineno`, frames of tracebacks) | Unsafe when a line is deleted in a script that reads frames or calls a library name that may hold a frame builtin; removing only the comment keeps lines (`line_removal_is_unsafe_where_frames_are_read`) |
| `SCR124`, `SCR900` | nothing: trailing whitespace outside string literals is not in any value, line numbers do not move, and Jython has no source for scripts compiled from strings (`inspect.getsource` raises `IOError`, tracebacks show no source lines; verified on Ignition 8.3's Jython) | None needed (`dynamic_code_leaves_the_fix_safe`) |

Values that do not come from a name: calling the result of a call or an
item of a container (`factory()()`, `getattr(o, name)()`, `handlers[k]()`,
`vars()['f']()`) counts as calling a frame builtin; an item of a library
name (`lib.util.HANDLERS['x']`) is a callee check. Methods of other
values (`x.m()` where `x` is a local variable, an event or payload value
or a call result) are assumed not to be frame builtins: that is the
remaining documented assumption.

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

* **Statement deletion and docstrings.** Deleting a statement must not
  change the docstring of a module, class or function body: the first
  statement of a body, when it is a bare string literal, is its
  docstring (`__doc__`, which code can read). The shared deletion helper
  (`delete_statement`) keeps a `pass` when removing the first statement
  would promote the following string, and the post-fix safety net
  (`engine::py_docstring_changed`) drops any fix that changes the multiset
  of docstrings of a `.py` script, catching the cases a single deletion
  guard cannot (several deletions exposing a later string). Removing lines
  before a misplaced `from __future__ import` cannot make it valid without
  the result still being a syntax error, which the health net already
  rejects.
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
* **`VIEW015`: default deny.** Three review rounds each found a new way
  for a script to reach a property that the analysis did not know
  (`exec`, globals, method references, reads into unused locals,
  rebinding through aliases, qualified builtins). The access analysis
  (`script_facts/access.rs`, module documentation) therefore enumerates
  the understood uses and treats every other use as reading and writing
  everything the value reaches. Understood uses:
  * component or view: `.custom`, `.params` (view), `.view`, `.parent`,
    `.children`, `.rootContainer` and their getters, `.getSibling(...)`,
    `.getChild(...)` (literal names resolve, others mean any component),
    `.page`, `.session` and their getters (a host: only `.props`,
    `.custom`, names, `.print`, `.close`, `.isConnected` are understood
    on it), the harmless members `.props`, `.meta`, `.position`, `.name`,
    `.id`, `.print`, `.focus`, `.requestTooltip`, `.removeTooltip`,
    `.requestContextMenu`, `.removeContextMenu`, `.requestPrint`,
    `.refreshBinding`, custom methods the view defines for it,
    `.custom = ...` (writes all), `children[i]`;
  * container: `c.name` (read, write, delete or update by context),
    `c['name']`, `'name' in c`, `c.get('name', ...)`, `c.has_key('name')`,
    `c.__contains__('name')`, `c.__finditem__('name')`,
    `c.pop('name', ...)` and `c.setdefault('name', ...)` (read and write),
    `c.keys()`, `.values()`, `.items()`, the `iter*` methods,
    `.__iter__()`, `.__len__()`, `.__repr__()`, `.__str__()` (read all).
    Ignition 8.3's property wrapper resolves a property before a method
    of the same name (verified on the gateway's classes:
    `self.custom.keys` is property `keys` when it exists), so a member
    named like a wrapper method or a special name counts as that
    property and, unless called in one of the forms above, as the whole
    container;
  * any followed value: a local name assigned only once from it (class
    bodies and `global` names excluded; in library functions the
    assignment itself is recorded, so `x = p.custom.a` reads `a` even
    when `x` is never used), an argument of a library function (followed
    through its body), and in library functions `p is None`, `if p:`,
    `not p` on the parameter itself.

  The test `every_context_is_understood_or_reads_everything` (and
  `library_contexts_are_understood_or_read_everything` for library
  functions) puts a container and a component in every Python 2.7
  expression and statement context and checks that each is either one
  of these forms or reads everything.
* **`VIEW015`: frame builtins.** A builtin that reads the caller's
  variables (`locals`, `vars`, `dir`, `eval`, `execfile`, `input`) reads
  the frame of whoever calls it, under any name and from any module
  (verified on Jython). A script or library function that calls one, runs
  `exec`, or reaches one or a frame in any other way
  (`iglint_python_semantic::dynamic`) reads everything. Every dotted name
  that may be a library name and that a script calls or uses as a value
  (`lib.util.ev(s)`, `f = lib.util.ev`, a name imported from the
  library) is checked through the project index: a plain `def` function,
  a class or a literal constant of a module without dynamic code, or a
  name that does not exist, is fine; anything else (a variable, a name
  that may be rebound) reads everything. Values that do not come from a
  name (results of calls, items of containers, event and payload values)
  are assumed not to be frame builtins. The same escape routes make every
  scope of a module read its locals by name for the unused-name rules
  (`SCR102`, `SCR103`, ...).
* **`VIEW015`: library functions that may change.** A library function
  is followed only while nothing can replace it: a function or class
  used as a value anywhere in the project (other than as an argument of a
  `system.*` function, which only calls it) can have its `func_code`
  replaced or its module's names rebound through `func_globals` by
  whoever receives it, so every name of its module counts as rebound;
  so do modules imported with `importlib.import_module` or `imp`.
* **`VIEW015`: inter-procedural findings are unsafe.** A finding that
  holds only because values were followed into library functions (what
  the function reads, or that it does not exist) is still reported, but
  its fix is unsafe: it deletes data on the strength of code in other
  resources, which other projects, code outside the workspace or
  run-time changes can invalidate. A finding the view's scripts prove on
  their own (followed values treated as escaping) keeps the safe fix;
  checking that a called name is a plain function does not count as
  following a value.
* **`VIEW015`.** Removing a property that a script only writes is not
  neutral when its `propConfig` entry sets `access` (the property recreated
  by the write would be `PUBLIC`), so the fix is unsafe for written
  properties. A property that might be read dynamically is never reported
  (dynamic reads count as reading every property they could reach), so
  the fix is safe only when the property is provably unread and
  unwritten. Library calls are resolved in the project the view belongs
  to and in every project inheriting from it; a function a child project
  overrides counts as reading everything.
