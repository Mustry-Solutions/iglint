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
* `__all__`, and dynamic access inside the script: the introspection
  routes of `iglint_python_semantic::dynamic` (frame, traceback, code,
  generator, function and class attributes named in source or in a string
  literal; reflective accessors with a computed name or used as values;
  computed format strings; the interpreter and reflection APIs), and any
  code that is not vetted (see "Threat model");
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

## Threat model

Seven review rounds in a row found a dynamic Python route that defeated a
safe fix, each time one that an enumerated list did not contain (frames,
tracebacks, generators, reflection, `cPickle` opcodes that call `eval` or
`globals()` in the caller's frame, `str.format` reading attributes,
`warnings` and Java stack traces reading line numbers, Java bean
properties `x.class` / `x.classLoader` / `x.methods` reaching reflection,
vetted modules handing out `sys` as a `PySystemState`, and byte-string
literals that are not valid UTF-8). The argument is therefore no longer
"these are all the routes". It is:

**Safe fixes assume that the code uses only vetted modules, Java classes,
builtins and module attributes, plus the language constructs that the
default-deny walkers analyse. Everything else is unknown code, and unknown
code withholds every safe fix that depends on what code can read.**

Three mechanisms carry it:

1. **Vetted code** (`iglint_python_semantic::vetted`, default deny). A
   script or library module keeps the safe fixes of `SCR102`, `VIEW015`
   and `NAME001` only when every import, builtin and module attribute it
   uses is on the allowlist below, every import that is not is a project
   library module whose own code is vetted (transitively, also for the
   library names it calls without importing), and no vetted module name
   is shadowed by a project library package. The attributes that are
   vetted per module, and the Java member names that reach reflection, are
   not hand lists: they are measured inside the Ignition image (see
   "Machine-measured facts" below). Subclassing a Java class is not plain
   (its instances are Java-backed). Findings are not affected: only the
   fixes become unsafe.
2. **The default-deny walkers.** Within vetted code, the access analysis of
   `VIEW015` and `NAME001` (`script_facts/access.rs`) enumerates the
   understood uses of components and properties and treats every other
   use as reading and writing everything; the introspection routes of
   `iglint_python_semantic::dynamic` make every scope of a module read
   its locals and write its globals; callee checks resolve library names
   to plain functions, classes and literal constants.
3. **The post-fix safety net** (`crate::safety`, run by the engine for
   `--fix`, `--diff` and the language server's fix-all). A safe fix on
   Python code is dropped when, for any script of the file, it moves a
   line, changes whether the script compiles (syntax and compile errors,
   in either direction), changes a docstring or the `__future__` imports,
   changes the scopes (generator status, `global` declarations, `import
   *`, `exec`, frame reads), or changes what any remaining name resolves
   to (local, enclosing function, global, builtin, injected) or whether it
   may be unbound. It runs on every storage format: `.py` library modules
   and event files, and scripts in `view.json` and other resource JSON
   (event actions, transforms, change scripts, message handlers, custom
   methods, session and tag event scripts), each compiled with its
   generated header; `every_check_runs_for_every_script_kind` enumerates
   them.

### Machine-measured facts

Two parts of the vetted-code guard are measured, not hand-listed, because a
name list for either was defeated. They are generated inside the Ignition
8.3.3 image by `stubs-gen/gen_vetted.py` (run `stubs-gen/run.sh --vetted`,
which needs only the image's Jython and reflection, not a gateway) into
`crates/iglint_python_semantic/src/generated/vetted_generated.rs`. The test
`generated_tables_match_vetted_inputs` fails if the vetted module list or
Java class list changes without regenerating.

* **Java reflection, a closed argument.** In Jython every value is a
  `java.lang.Object` and exposes `getClass()`, spelled `.class` or
  `__class__` as a bean property; from a `Class` a script reaches its
  `ClassLoader`, `methods`, `fields` and constructors, and from those
  `org.python.core.Py`, `getSystemState()`, the module table,
  `__builtin__` and `eval` (verified on Jython: subclassing `ArrayList`
  and reading `.class` reads the module's globals). The argument is not a
  list of bad names; it is: **a script cannot reach a reflection object
  without, as its first hop from a plain value, naming a member whose
  declared type is reflection-bearing** (`Class`, `ClassLoader`,
  `java.lang.reflect.*`, `java.lang.invoke.*`). The generator enumerates
  exactly those first-hop members over the types a plain value can hold
  (`Object`, `Throwable`, `Thread` and every vetted Java class), in both
  the Java method and the Jython bean / field spellings, into
  `JAVA_REFLECTIVE_MEMBERS`. `iglint_python_semantic::dynamic` treats any
  of them, named on a value or as an identifier in a string literal, as
  introspective and fails closed; this covers values returned by
  `system.*` (any `.class` on them is caught) without enumerating their
  types. `StackTraceElement` and `Thread` are deliberately not
  reflection-bearing: a stack-trace element gives only line numbers and
  class-name strings (a name becomes a class only through `Class.forName`,
  which is flagged, and line numbers are protected by line preservation),
  and a `Thread` is plain until its `ClassLoader` is named
  (`getContextClassLoader`, which is flagged). So `Thread.currentThread()`
  and reading a stack trace, both common in Ignition, stay plain.
* **Vetted module attributes, measured positively.** `sys` in Jython is a
  `PySystemState`, not a `types.ModuleType`, so `re.sys`, `calendar.sys`
  and `os.path.sys` slipped past a "deny the module attributes" rule and
  reached `modules` and `__builtin__`. The generator now measures every
  attribute of every vetted module by kind and emits `MODULE_VETTED_ATTRS`,
  the attributes that are a function, a class or a constant of an immutable
  vetted type. Everything else, a module, a `PySystemState`, a Java class
  (`base64.binascii`) or any other object, is unknown. `vetted.rs` consults
  this positive list; the per-module denylist still removes members that
  look names up from data (`string.Formatter`, `operator.attrgetter`).

### Byte strings

A `str` literal that is not valid UTF-8 (`'{0.f_locals}\xff'`) must never
read as "nothing" in a safety scan: Jython maps each byte to the Java char
of the same value (Latin-1, `'\xe9' == u'é'`). `StringValue::to_latin1`
does that and never fails, and every safety-relevant scan uses it (the
introspection-attribute scan, `NAME001`'s literal collection). The
`script_facts` access and reference analyses already fail closed on a
non-decodable key (an unknown key reads and writes everything).

### Closed world

The Ignition runtime is shared across every project and gateway-scoped
script (tag event scripts, WebDev, session and gateway events). Code that
can read a frame or the interpreter, anywhere in that runtime, can observe
any module's names and, through `sys._current_frames()`, any frame's
locals, including a function-local import and a component held in a
running view script. So "unknown code" for the safe-fix guard is not only
`vetted::unknown_code`: a script with any frame or interpreter route
(`iglint_python_semantic::dynamic`'s escape routes) counts too, recorded
as `References::unknown_code` and surfaced as
`ProjectIndex::has_any_unknown_code`. The Safe applicability of `SCR102`
(module-level and function-local), `VIEW015` and `NAME001` requires the
whole workspace closure, every project, its parents, the children that
inherit and can call its library, and the gateway-scoped scripts, to be
free of unknown code. Projects outside the workspace: a safe fix is
withheld when the project is inheritable or its inheritance chain is
incomplete (`all_library_references_known`), so a child that may exist
outside the workspace cannot invalidate a fix. The workspace is the closed
world; there is no option to declare it complete while inheritable.

### Vetted code

Each entry was checked on Ignition 8.3's Jython: it cannot evaluate code,
read a frame, or look up callables or attributes by a name taken from data.
Callables passed as values (`json.loads(s, object_hook=f)`, `re.sub(p, f,
s)`, `system.util.invokeAsynchronous(f)`) are fine: a frame builtin used
as a value is itself a route.

| Allowed | Reason and exceptions |
|---|---|
| `math`, `cmath`, `time`, `datetime`, `calendar`, `random`, `itertools`, `heapq`, `bisect`, `base64`, `binascii`, `hashlib`, `struct`, `textwrap`, `decimal`, `fractions`, `uuid`, `re`, `json`, `os.path` | Pure computation on their arguments. Which attributes are vetted is measured positively per module (see "Machine-measured facts"): only functions, classes and immutable constants. Attributes that are modules (`re.sre_compile`, `os.path.os`, `json.decoder`), a `PySystemState` (`re.sys`), a Java class (`base64.binascii`) or private are not vetted. Module objects used as values are not vetted. |
| `string` | Except `Formatter`, which reads the attributes its format string names (verified). `Template` only looks names up in the mapping it is given (verified: `${x.y}` is rejected). |
| `collections` | `namedtuple` runs generated code in a namespace of its own and rejects type and field names that are not identifiers (verified). |
| `operator` | Except `attrgetter` and `methodcaller`, which look attributes up by name (verified). |
| `os` | Only path and file functions and constants (`path`, `sep`, `getcwd`, `listdir`, `stat`, `environ`, `getenv`, `makedirs`, `remove`, `rename`, `walk`, ...). Not processes or anything else. |
| `sys` (also as Ignition's implicit global) | Only `version`, `version_info`, `hexversion`, `platform`, `maxint`, `maxsize`, `maxunicode`, `byteorder`, `float_info`, `stdout`, `stderr`, `exit`, `getdefaultencoding`, `getfilesystemencoding`, `getrecursionlimit`. Not `_getframe`, `modules`, `exc_info`, `path` (it decides which module an import loads), trace hooks. |
| `system` (Ignition's scripting API) | No `system.*` function of 8.3 evaluates Python code or resolves a callable by name in the caller's frame (checked against the generated stubs): functions that take callables call the value, message handlers are looked up among the project's handlers, not in the caller. |
| `__future__` | Compile-time only. |
| `java.util` (except `ServiceLoader` and the resource bundles, which load classes by name), `java.util.concurrent` (and `atomic`, `locks`), `java.util.function`, `java.util.regex`, `java.util.stream`, `java.math`, `java.text`, `java.time` (and `chrono`, `format`, `temporal`), `java.nio.charset` | Data structures and formatting; no reflection. |
| `java.lang`: `Math`, `StrictMath`, `String`, `StringBuilder`, `StringBuffer`, the boxed number types, `Boolean`, `Character`, `Number`, `System` (except `load`, `loadLibrary`, `mapLibraryName`, security managers, `inheritedChannel`), `Exception` and common runtime exceptions | Not `Class`, `ClassLoader`, `Thread`, `Throwable`, `Runtime`, `Process*`, reflection, `invoke` or `StackWalker`. The exception classes hand out only stack traces (class, method, file and line), which safe fixes never change. |
| `java.io`: readers, writers, file and byte streams, `File`, `IOException` | Not object streams: deserialization instantiates classes named by the data. |
| `java.net`: `URL`, `URI`, `URLEncoder`, `URLDecoder`, `InetAddress`, `HttpURLConnection`, common exceptions | Not class loaders. |
| Builtins: every builtin except those below; every exception class | Not `eval`, `execfile`, `compile`, `__import__`, `reload`, `input` (it evaluates what it reads), `locals`, `globals`, `vars`, `dir`, `help` (it imports modules by name). `getattr`, `setattr`, `delattr` and `hasattr` only as direct calls with a literal name (route 2 of `dynamic`). |

Everything else is unknown: `cPickle`, `pickle`, `copy_reg`, `shelve`,
`marshal`, `warnings`, `logging`, `traceback`, `inspect`, `timeit`, `code`,
`codeop`, `runpy`, `pkgutil`, `doctest`, `pdb`, `profile`, `trace`,
`unittest`, `socket`, `xml`, `org.python.*`, `com.inductiveautomation.*`
(Ignition's script manager can run code), relative imports, `import
java...` of whole packages, and any Java class or module not listed. The
list is a precision trade-off, not a claim that those are harmful: adding
an entry needs the same check on Jython.

### Line numbers

Line numbers are invariant under safe fixes. A safe fix that deletes a
whole statement (`SCR102`) or a whole-line comment (`SCR123`) removes the
text and keeps the line break (an empty line, or `pass` where a block or a
docstring needs it); the line-removing variant is the unsafe fix. The
safety net rejects any safe fix to Python source that changes the number
of lines or the line of any text it does not edit, measured on the decoded
script for JSON-embedded scripts and with `\r\n`, `\n` and `\r` each
counting as one line break. So whatever reads line numbers (tracebacks,
`tb_lineno`, `f_lineno`, `warnings`, `logging` records, Java stack traces
through `Thread.currentThread().getStackTrace()` or a caught exception)
sees the same numbers after `--fix`, and detecting those readers is no
longer needed for safety.

| Rule | Fix | Safe when | Otherwise |
|---|---|---|---|
| `NAME001` | Rename a component and every reference to it in the view | No reference to the component at all; no script reads `meta.name`; no script looks children up with a computed name or has a string literal equal to the name; every script parses; and no script of the view hands a component to code outside the view (library call, message payload, `getGlobals()`, an opaque call such as `locals()`), reaches a frame builtin in any way, calls an unnamed value or calls a library name that is not a plain function, class or literal constant: a library function that receives a component, or `eval` under another name, can call `getChild('Name')` itself; and every script of the view uses only vetted code (see "Threat model") | Unsafe when references exist, a component may escape or a script uses code that is not vetted (`cPickle.loads` of an `eval` opcode); no fix when a lookup cannot be rewritten or the new name is taken |
| `VIEW012` | Set, add or remove `access` in `propConfig` | Never: the access mode decides whether the browser receives the property and whether it may write it | Unsafe |
| `VIEW013` | Set `persistent: false` (and remove the stored value) | No stored value: the property starts out absent either way | Unsafe when a stored value is removed: scripts and expressions that run before the binding delivers (query bindings are asynchronous) see a different value |
| `VIEW015` | Remove an unused custom property and its `propConfig` entries | Nothing reads or writes the property, and the view's own scripts prove it: resolved reads and writes cover expressions, bindings (bidirectional ones write) and scripts, analysed default-deny (every use of a component, the view, a page or session, a property container or an alias of one must be one of the forms listed under "`VIEW015`: understood forms" below; anything else reads and writes everything it reaches); a script that reads frames or reaches a frame builtin in any way reads everything, and every library name a script calls or uses as a value is checked to be a plain function, class or literal constant (or missing); properties with a binding or change script are never reported | Unsafe when a script, a library function it calls or a bidirectional binding writes the property (the write recreates it at run time without the `access` and `persistent` settings of the removed `propConfig` entry); unsafe when the finding relies on following a value into library functions (inter-procedural: the function's body, a function that does not exist, the depth limit); unsafe for a view linted outside its project whose scripts call names that may be library functions; unsafe when a script of the view, or a library module it imports or calls, uses code that is not vetted |
| `SCR102` | Remove unused import names or statements; the safe fix keeps the emptied lines, the unsafe fix removes them | Not in a class body (never reported); not a module-level name the module may read dynamically (never reported); a top-level library import is provably unused by other resources and no resource of the project uses code that is not vetted; every removed import is effect-free: `java.*` / `javax.*`, or a plain `import x` / non-package `from x import y` of a standard module on the verified list, with no project library package of that name; the module uses only vetted code and calls no library name that may be a frame builtin; the import is not the only binding of a name still read in its scope or a nested scope; and it does not come before a `from __future__` import | Unsafe (project library modules, third-party and non-JDK Java packages, `socket`, `logging`, dotted imports, relative imports, unknown project, a library name that is not a plain function, class or literal constant, code that is not vetted, a name that would resolve differently, a misplaced future import that would become valid) |
| `SCR103` | Remove `x = value`, or only `x = ` when the value calls or binds something | Never | Unsafe: the value may be wanted under another name, and a removed attribute or subscript read, operator, comparison, `in` or `%` formatting can raise or run code (getters, special methods). The statement is removed only when the value calls nothing (no call, no backticks, which call `repr`, no `yield`) and binds nothing (no list comprehension, whose variables Python 2 leaks), so no pointless statement (`SCR114`) is left |
| `SCR110` | `is` to `==`, `is not` to `!=` | Never: the result changes whenever identity and equality differ (`int('1000') is 1000` is `False` in Jython), and `==` calls `__eq__` / Java `equals` | Unsafe |
| `SCR115` | `except A, B:` to `except (A, B):` | Never: the handler then catches `B` and no longer rebinds the name | Unsafe. The finding itself requires `B` to resolve (ignoring the handler) to a class statement, a capitalized `from` import or a builtin exception, and the handler body not to read it |
| `SCR123` | Remove unused suppression codes or the whole comment (a comment alone on its line leaves the line empty) | Only codes are removed (the rest of the comment stays), or the comment holds nothing but the suppression. Comments do not run, and no line moves | Unsafe when the whole comment would go and it also holds a reason, a `# pylint:` directive or a `coding:` declaration; that fix removes the line |
| `SCR124` | Remove trailing whitespace | The script parses, the whitespace is outside string literals and outside the generated header (a backslash continuation followed by spaces is a syntax error, so it is never touched) | No finding when the script does not parse |
| `SCR206` | Remove a `shared.` / `project.` prefix | Never: in Ignition 7 the two prefixes named different libraries | Unsafe |
| `SCR301` | Rewrite or insert the event script header | Never: code may rely on the written parameters or defaults | Unsafe |
| `SCR900` | ign-lint's trailing-whitespace fix (every line `rstrip()`ed) | The script parses and no removed whitespace is inside a string literal (multi-line strings) | Unsafe |

Rules not listed offer no fix. The language server uses the same
applicability: unsafe code actions are titled `(unsafe)`, and the
fix-all action applies unsafe fixes only when configured to. Every path
that applies or offers a fix goes through one function,
`engine::fix_file`, and so through the health check and the post-fix
safety net (see "Threat model"): `--fix`, `--fix --unsafe-fixes`,
`--diff`, `iglint ign-lint --fix`, the language server's fix-all (code
action, `source.fixAll` on save, command) and its single quick fixes
(`Live::fix_one`). A safe fix that fails the net is not applied, reported
as skipped, and not offered as a quick fix
(`every_fix_path_goes_through_the_net`,
`quick_fixes_go_through_the_safety_net`).

## Dynamic code: audit of every safe fix

Code that reads names without writing them can invalidate a fix. The
routes are those of `iglint_python_semantic::dynamic`: introspection
attributes named in source or in a string literal (`.f_locals`,
`.tb_frame`, `'tb_frame.f_back'`, `co_*`, `__subclasses__`, ...),
reflective accessors with a computed name or used as values
(`getattr(o, n)`, `get = getattr`, `attrgetter(n)`), format strings that
are not literals (`fmt.format(o)`), and the interpreter
and reflection APIs (`eval` / `locals` reached under any name such as
`e = eval` or a library variable holding one, `sys._getframe()`,
`inspect`, `traceback`, `sys.exc_info`, `org.python.*`, `.getClass()`),
plus functions whose code is replaced (`f.func_code = ...`, also through
an alias or a helper). Where they matter, the guards are shared:
`iglint_python_semantic::dynamic` (escape routes, which make every scope
of a module read and write names dynamically),
`script_facts::access` (default-deny accesses of view scripts, with
callee checks of library names), and
`unused_custom_property::{resolve_callee_checks,
library_names_may_read_frames}` (callee checks resolved through the
project index: a library name that is not a plain function, class or
literal constant may hold `eval`; outside a project every such name
counts), and `rules::vetting` (code that is not vetted withholds the safe
fix). Each rule has a regression test with the patterns.

| Rule | What dynamic code could do | Guard |
|---|---|---|
| `NAME001` | look the old name up (`eval("self.getChild('Old')")`, a library function receiving `self` whose code is replaced, `getattr(self, n)`) | Unsafe when a component may escape: accesses read everything for frame routes, `exec`, values passed to unknown code, calls of unnamed values, and callee checks that do not resolve to plain functions, and code that is not vetted (`dynamic_code_makes_the_rename_unsafe`) |
| `VIEW013` | nothing: scripts cannot see the `persistent` flag, and without a stored value the property starts out absent either way | None needed (`dynamic_scripts_do_not_change_the_fix`) |
| `VIEW015` | read or write any property | Default-deny accesses, frame routes, callee checks, rebinding through exposure and `func_globals`; inter-procedural findings unsafe; code that is not vetted makes the fix unsafe (`dynamic_patterns_read_everything`, `unvetted_code_makes_fixes_unsafe`) |
| `SCR102` | read the import by name (`lib.util.ev('os')`, `capture()['os']`, `getattr(me, n)` on the module itself) | Scopes with escape routes or frame builtins are dynamic (no finding); a computed `getattr` on a module object keeps module imports; a library name that may be a frame builtin, or code that is not vetted, makes the fix unsafe (`library_frame_builtins_and_computed_getattr`, `frame_builtins_under_other_names_keep_imports`, `review_round_six_repros`) |
| `SCR123` | nothing: comments do not run, and the safe fix keeps every line, so line numbers (`f_lineno`, tracebacks, `warnings`, Java stack traces) do not move | None needed (`safe_removal_keeps_lines`) |
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
  (`crate::safety`) drops any safe fix that changes the multiset of
  docstrings of any script, `.py` or embedded in JSON, catching the cases
  a single deletion guard cannot (several deletions exposing a later
  string). `engine::py_docstring_changed` applies the same check to
  unsafe fixes of `.py` files.
* **Statement deletion and `from __future__`.** Deleting the statements
  before a misplaced `from __future__` import can make it valid: `import
  os` followed by `from __future__ import division` is a SyntaxError, and
  without the first line it compiles and `1/2` is `0.5` (verified on
  Jython). An earlier version of this document claimed the opposite. A
  deletion before a `__future__` import is therefore unsafe in `SCR102`,
  and the safety net rejects any safe fix that changes whether a script
  compiles, in either direction.
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
