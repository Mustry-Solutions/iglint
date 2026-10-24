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
  stored values, bindings and change scripts that run on their own;
* the files themselves: every fix edits a file of the workspace, and code
  that reads that file (its bytes, size or modification time) sees the
  edit, however neutral it is for the code that runs (see "File reads").

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
   (its instances are Java-backed). Names are resolved through the
   semantic model's scopes, flow-insensitively: a use may stand for every
   binding of the name in the scope it resolves to, whatever the textual
   order (a function defined before `import re` reads the module `re`
   when it runs), plus the injected name or builtin where Python looks
   further at run time (module and class bodies, `global` names). A use
   is unknown when any of those bindings is not vetted; a name the model
   cannot resolve, or that resolves dynamically (`import *`, `exec`), is
   unknown. `verdict_does_not_depend_on_statement_order` checks every
   permutation of the top-level statements of each known exploit, at
   module level and inside a function. Findings are not affected: only
   the fixes become unsafe.
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
   may be unbound. Names that statements bind without naming them are
   bindings of the semantic model too: `__module__` in class bodies and
   the `__package__` that every import binds in a library module's
   globals (see "Names imports bind"), so removing the only import of a
   module that reads `__package__` changes what the read finds. It runs
   on every storage format: `.py` library modules
   and event files, and scripts in `view.json` and other resource JSON
   (event actions, transforms, change scripts, message handlers, custom
   methods, session and tag event scripts), each compiled with its
   generated header; `every_check_runs_for_every_script_kind` enumerates
   them.

### Machine-measured facts

Parts of the vetted-code guard are measured, not hand-listed, because a
name list for each was defeated or could not be checked. They are generated inside the Ignition
8.3.3 image by `stubs-gen/gen_vetted.py` (run `stubs-gen/run.sh --vetted`,
which needs only the image's Jython, jars and reflection, not a gateway)
into `crates/iglint_python_semantic/src/generated/vetted_generated.rs`. The
test `generated_tables_match_vetted_inputs` fails if the vetted module
list, the Java class or package lists or the stubs build changes without
regenerating, and `vetted_names_exist_in_the_image` that every vetted
class, package and module was measured to exist.

* **Java reflection and the interpreter, a closed argument.** A Java
  value in Jython exposes its public Java members, `getClass()` included,
  spelled `.class` or `__class__` as a bean property; from a `Class` a
  script reaches its `ClassLoader`, `methods`, `fields` and constructors,
  and from those `org.python.core.Py`, `getSystemState()`, the module
  table, `__builtin__` and `eval` (verified on Jython: subclassing
  `ArrayList` and reading `.class` reads the module's globals). Python
  values do not: Jython types that declare `@ExposedType` show only their
  exposed members (verified: `[].getClass`, `str.getProxyType` and
  `(lambda: 1).toString` raise `AttributeError`), and their introspection
  attributes are `INTROSPECTION_ATTRS`. The argument is not a list of bad
  names; it is: **a script cannot reach a stop type without, as its first
  hop from a value it holds, naming a member whose declared type mentions
  one**. The stop types are Java reflection (`Class`, `ClassLoader`,
  `Module`, `ModuleLayer`, `Package`, `java.lang.reflect.*`,
  `java.lang.invoke.*`), Jython's reflection handles (`ReflectedArgs`,
  reflected fields, bean properties and events) and the interpreter
  (`PySystemState`, frames, tracebacks, code, modules and their
  namespaces, `PythonInterpreter`, `javax.script` engines, Ignition's
  `ScriptManager`). The generator computes those first hops as a
  transitive fixed point over the public members (methods, no-argument
  getters as bean properties, fields; inherited and static ones included)
  of every type a script can hold, starting from:
  1. `java.lang.Object`, `Throwable` and `Thread`, and the two stop types
     a script holds without any hop: a Java class named in source (or
     `type(x)` of a Java value) is a class proxy that exposes
     `java.lang.Class`'s methods (verified: `String.getMethods()` works),
     and a Java method named without calling it (`String.valueOf`,
     `system.tag.readBlocking`) is a reflected function that exposes its
     `argslist` (verified); their members are scanned too;
  2. every vetted Java class and every public class of every vetted
     package;
  3. the Java types that `system.*` functions return and `system.*`
     objects have, in every scope, as recorded in
     `crates/iglint_stubs/data/system.json` from a live gateway; a
     Python-level name there (`list`, `Dataset`, `any`, ...) stands for
     every Java type `stubs-gen/normalize.py` writes as that name, and the
     `system.bacnet.*` classes for every public class of
     `com.serotonin.bacnet4j.type`;
  4. the objects Ignition hands to scripts: every public class of the
     Perspective script package (`self`, view, page and session wrappers,
     property trees, event objects, message payloads), the Jython maps of
     `ignition.common.script.abc`, qualified values and quality codes, the
     tag event script package, Vision's script wrappers and `java.awt.event`,
     and the dataset, logger, alarm event, tag change event,
     `PropertyChangeEvent`, WebDev (`HttpServletRequest`, `...Response`,
     `HttpSession`, `GatewayContext`) and SFC chart scope classes listed in
     `INJECTED_CLASSES`.

  From each type the closure follows every type a member's declared type
  mentions (the erasure, generic type arguments, wildcard and type
  variable bounds, array elements), declared exception types (an `except`
  clause catches them), the argument types of the abstract methods of
  interface parameters (a Python callable passed for one is called with
  them) and public member classes. A member declared `Object` stays
  `Object`. Expansion stops at the stop types; every member whose type
  mentions one is recorded, in the Java method and the bean / field
  spellings, into `JAVA_REFLECTIVE_MEMBERS`, with one declaring member per
  name as provenance. For 8.3.3 that is 960 seed types and 2,636 scanned
  types (15 Jython types skipped as above, none that fails to load);
  among the names are `module`, `interfaces` and `classes` (class
  proxies), `argslist` (reflected functions), `getColumnTypes` (datasets),
  `getScriptManager` (the gateway context) and `type` (alarm properties).
  `iglint_python_semantic::dynamic` treats any of them, named on a value
  or as an identifier in a string literal in a sink position (see "String
  sinks" below), as introspective and fails closed. Jython's lookups by name on such Java-backed values (`invoke`,
  `__findattr__`, `__findattr_ex__`) are reflective accessors, understood
  only with a literal name.

  What the closure does not cover, precisely: members that only a run-time
  subclass declares (Jython resolves members on the run-time class; the
  closure follows declared types, and a value typed `Object` or held in a
  `Map` or `List` typed `Object` contributes nothing); classes that are
  not in the image (third-party modules, jars in `user-lib/pylib`, classes
  generated at run time); arguments that `system.*` functions pass to
  Python callbacks declared as `PyObject`; and Java methods that look a
  member up, or run code, from a string argument without a stop type in
  their signature (a review of the closure for `run*`, `exec*`, `eval*`,
  `compile*`, `invoke*`, `call*` and `parse*` methods taking a string
  found SQL and named-query runners, path and version parsers, gateway
  network service calls and Jython's `invoke`, besides `ScriptManager`,
  which is a stop type).
  `StackTraceElement` and `Thread` are deliberately not stop types: a
  stack-trace element gives only line numbers and class-name strings (a
  name becomes a class only through `Class.forName`, which is flagged, and
  line numbers are protected by line preservation), and a `Thread` is
  plain until its `ClassLoader` is named (`getContextClassLoader`, which
  is flagged). So `Thread.currentThread()` and reading a stack trace, both
  common in Ignition, stay plain.

  The precision cost is measured on the private corpus (its revision of
  2026-10-21, measured on 2026-10-23; the numbers depend on the
  revision). With the string sinks below, against main: findings 5,372 to
  5,378; VIEW015 179 to 178 (one finding lost, 56 unsafe fixes as
  before), SCR103 34 to 39 and REF012 19 to 21 (findings that literals
  such as `'class'` used to hide), every other rule unchanged; safe fixes
  unchanged (the corpus holds unknown code, so its SCR102 and VIEW015
  fixes were unsafe before). Scanning every string literal instead would
  have cost 42 findings (VIEW015 179 to 151), mostly through `type` and
  `module` in literals that are data.
* **Gateway stores, a third fixed point.** Ignition's gateway managers
  persist tag, project and gateway configuration, or run code, from data
  they are given: `TagManager.importTagsAsync` stores tags, with their event
  scripts and expressions, from a JSON string, and the WebDev
  `request['context']` (a `GatewayContext`) hands out every such manager.
  `system.*` was audited for this ("`system.*` functions that run code"),
  but these objects are reachable without `system.*`. The generator
  measures them: a first fixed point over the gateway seeds (the file
  closure's seeds, stopping only at the reflection stop types; 2,376 types
  for 8.3.3) finds the **store types**, every Ignition type whose simple
  name ends in `Manager`, `Provider`, `Registry` or `Service` and that has a
  public method of its own taking an argument whose name starts with a
  store verb (`import`, `configure`, `save`, `edit`, `push`, `commit`,
  `create`, `update`, `delete`, `remove`, `add`, `write`, `install`,
  `register`, `run`, `exec`, `execute`, `eval`, `compile`, `restore`,
  `rename`, `move`, `copy`, `apply`, `set`), plus `vetted::STORE_TYPES`
  (the gateway and Perspective contexts, the project, configuration and
  resource collection managers, resource collections, `ScriptManager`):
  54 types, among them `TagManager`, `GatewayTagManager`, `TagProvider`,
  `ManagedTagProvider`, `AlarmManager`, `ScheduleManager`,
  `DatasourceManager`, `SystemPropertiesManager`, `TagScriptManager` and
  `NamedQueryManager`. A third fixed point over the same seeds, with the
  store types as stops, records `JAVA_STORE_MEMBERS`, the members whose
  declared type mentions one: 12 names for 8.3.3 (`getTagManager`,
  `getGatewayContext`, `getPerspectiveContext`, `getScriptManager`,
  `getNamedQueryManager`, `getCookieManager` and their bean spellings, all
  on Perspective's `Session` or the HTTP client). `vetted::unknown_code`
  treats any of them named on any value but a vetted Python module, or as a
  literal `getattr` name, as unknown code. The WebDev context is an
  injected store, so it is not followed: in a WebDev handler, any use of
  the `request` parameter other than an item with a literal key that is not
  `context` (`request['params']`, `request.get('remainingPath')`) is
  unknown code with its site (`script_facts::webdev`). The regeneration
  test `generated_tables_match_vetted_inputs` covers `STORE_TYPES`;
  `gateway_stores_are_unknown`, `webdev::tests::context_uses` and the
  binary test `webdev_gateway_context_is_unknown_code` cover the routes.
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

* **Perspective script objects.** `self`, `self.view`, `self.page`,
  `self.session` and the `session` and `page` of session events are
  `PyDelegateObject`s over a `SafetyWrapper` (read in the gateway's
  bytecode: property tree names from `PropertyType` first, then custom
  methods and the component model delegate's script-callable methods,
  then the wrapper's public Java members). The generator emits
  `PERSPECTIVE_MEMBERS` (every member of the component, view, page and
  session wrappers in the method and bean spellings, with the kind of
  its declared type: a page or session, a view or component, a property
  tree, `Object`/`PyObject`, or a value), `PERSPECTIVE_PROPERTY_TYPES`
  and `PERSPECTIVE_DELEGATE_MEMBERS`, exposed by
  `iglint_python_semantic::perspective`. The access analysis classifies
  each member and `every_measured_perspective_member_is_classified`
  fails on a member it does not: on a page or session, members that
  return views, pages or components reach other views (the session
  member returns the session); on a component or view, every member is
  one of the understood forms; `getInfo` and `getProjectInfo` are JSON
  data (`Gson.toJsonTree` in the bytecode); the delegates' members return
  values or map data. A `PropertyTree` has no member that reaches its
  owner.
* **What an import finds.** `JAVA_PACKAGE_CLASSES` lists the public
  top-level classes of every vetted Java package and of the packages of
  the vetted classes, and `MODULE_ATTRS` every attribute (`dir()`) of
  every vetted Python module, `os` and `sys` that imports in the image. A
  Java class is vetted only when it is listed there (an import of a
  missing class fails), and `SCR102` removes an import safely only with
  this positive evidence (see "Import side effects").

### Names imports bind

Reviewer round 12 (K1): a library module whose only import was removed
safely returned `'ABSENT'` instead of `None` from a function reading
`__package__`. Jython's import computes the importing module's package
from the `__name__` of its globals and stores it as `__package__` before
it looks the module up. Measured with `oracle/run-oracle.sh --implicit`
inside the 8.3.3 image, through Ignition's own `ScriptManager` (the table
is in `ignition-format.md`, "Names that statements create";
`implicit_names_oracle.rs` checks the model against it):

* in a library module every absolute import statement that runs binds
  `__package__` (`None`, as `__name__` is the module's last path
  component), wherever it is and whether it succeeds: plain, dotted, `as`,
  `from`, `*`, `__future__`, Java, project library and failing imports;
  relative imports raise `ValueError` first;
* event scripts (function bodies and top-level code after the function)
  run without `__name__`, so their imports bind nothing unless the code
  binds `__name__` itself;
* an import that warns leaves `__warningregistry__` (relative imports in
  event scripts; any import once the code set a dotted `__name__` or a
  `__package__`);
* no other statement creates a name; `__doc__` comes only from a
  docstring, and no context has `__file__`.

The semantic model binds `__package__` in the module namespace at each
such import (`BindingKind::ImportPackage`, with the import's scope as its
site, like an assignment under `global`), so the safety net compares what
reads of it resolve to. Across scopes the model is flow-insensitive (an
import in a function that never runs still counts), so `SCR102` does not
rely on the net alone: removing a whole import statement that binds
`__package__` is unsafe whenever anything may observe the names above (see
the `SCR102` row). Tests: `imports_bind_package_in_library_modules`,
`the_net_sees_the_package_an_import_binds`,
`removing_the_import_that_binds_package_is_unsafe_when_observed`, the
binary test `imports_that_bind_a_read_package_are_kept` and the language
server test `quick_fix_for_an_import_whose_package_is_read_is_unsafe`.

### String sinks

A string literal names a member at run time only in a sink, so only
literals in sink positions are scanned for introspection attributes
(`tb_frame`, `f_locals`, ...) and the generated reflective names; any
other literal is data (`{'type': 'popup'}`, a log message that says
"module"). The sinks, verified on Jython:

* the name arguments of a direct call of a reflective accessor:
  `getattr`, `hasattr`, `setattr`, `delattr`, `__getattribute__`,
  `__getattr__`, `__setattr__`, `__delattr__`, Jython's `__findattr__`,
  `__findattr_ex__` and `invoke`, `operator.methodcaller` and every
  argument of `operator.attrgetter` (dotted names included);
* the receiver of `format` and `vformat`, called or used as a value
  (`'{0.f_back}'.format(g)`, `str` and `unicode` alike);
* code: the literal argument of `eval`, `execfile` and `input`, and the
  body of an `exec` statement.

The argument: every other way to look a member up from a string is an
escape whatever its argument, so no literal can reach a member without
passing through a sink position. A sink with a name or format string that
is not a literal at the call is an escape, and so is a sink used as a
value (`get = getattr`, `f = fmt.format`, `map(getattr, ...)`), called on
something else (`str.format(fmt, x)`, `object.__getattribute__(o, n)`,
`string.Formatter().format(...)`) or with unpacked arguments. A literal
that flows into a sink through a variable, a concatenation, `join`, `%`,
slicing, a container or a function return is not a literal at the sink,
so the sink is computed. `vars(o)` is an escape (verified:
`vars(Object)['getClass'](x)` is `x.getClass()`), `__dict__` is an
introspection attribute, and `compile` and `__import__` are escapes. Not
sinks, verified: `%` formatting and `string.Template` look keys up with
`__getitem__` (`'%(x)s' % o` needs a mapping), and `format(o, spec)`
takes a format spec, not field names. `dynamic::tests::string_sinks`
enumerates each sink with a literal introspection name, a plain literal
and every kind of computed argument, and the non-sinks.

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

**Other views.** A Perspective page hands out the views it shows
(`page.views`, `getViews()`, its primary `page.view`) and a session its
pages (`session.pages`, `getPages()`, `getPage(id)`); the views arrive as
view wrappers with their `custom` and `params` trees and `getChild(...)`.
So a script of one view, a session event or page startup script, a
session property script or a library function handed any of these
objects can read and write the custom properties and look up the
components of every other view. The access analysis
(`script_facts/access.rs`, "Other views") records such a use, and the
script layer counts it as unknown code with its site: a member of a page
or session that hands out views, pages or components (every member is
measured, see "Machine-measured facts"), a special name of any of these
objects, an item of a page or session, a component, view, page or session
used as a value the analysis does not follow (stored, put in a container,
passed to a builtin, returned), and a call that hands one to a name that
may be a library function whose followed body reaches other views, passes
the object on to code that is not followed, or that is not a plain
function (`resolve_perspective_calls`, after the index is built). Nothing
else hands out a page or session: no `system.perspective` function returns
one (`getSessionInfo` and `getProjectInfo` return data), and expressions
and property bindings name only `this`, `view`, `parent`, `page` and
`session` paths of their own view, page and session.
`other_views_through_pages_and_sessions_make_fixes_unsafe` (VIEW015),
`other_views_through_the_page_make_the_rename_unsafe` (NAME001), the
binary test `fix_keeps_properties_read_from_another_view` and the
language server test `quick_fix_for_a_property_read_from_another_view_is_unsafe`
cover each route.

### File reads

Every fix, of every rule, edits a file of the workspace: a script, a
`view.json`, a resource manifest. A script that reads that file observes
the fix. `import math` followed by
`print open('.../m/code.py').read().startswith('import math\n')` prints
`True`, and `False` after `SCR102`'s safe fix (verified on Jython in the
8.3.3 image); a script that reads the last line of another script sees
`SCR124` trim its trailing whitespace; a view event that runs
`json.loads(open(<its view.json>).read())` sees `VIEW015` remove a custom
property. An earlier version of this document claimed that whitespace
and comments cannot be observed: they cannot through the running code, but
they can through the file.

So **no fix is safe while code in the workspace may read a file that a fix
edits**, unless the project declares that none does. The engine applies
this after every rule (`rules::vetting::gate_file_reads`, on every path
that reports or applies fixes: `check`, `--fix`, `--diff`, the fix passes,
the language server's diagnostics, quick fixes and fix-all), so it covers
every rule, present and future, including JSON-only fixes. The unsafe
reason names the first such site in the workspace and the setting.

**What counts** (`ProjectIndex::may_read_workspace_files`, from
`References::reads_files`):

* a possible access in vetted code (`iglint_python_semantic::file_reads`,
  reported by `vetted::unknown_code` as `UnknownCode::file_read`). This is
  default deny, not a list of reads: every vetted facility that takes a
  path or a file descriptor counts unless **every** path argument is a
  literal outside the workspace, whatever the mode or the direction of
  the operation. Reads observe a fix. Writes and appends observe it too
  (`f = open(p, "a"); f.seek(0, 2); f.tell()` returns the size, verified
  on Jython in the 8.3.3 image) or change the files a fix edits; renames,
  copies and links make a workspace file visible under a literal outside
  path (`os.rename('<workspace>/m/code.py', '/tmp/m.py')` followed by
  `open('/tmp/m.py').read().startswith('import math\n')` prints `True`
  before `SCR102`'s fix and `False` after, verified); deletions and
  creations change what a fix edits. An earlier version exempted
  write-only modes of `open` and treated `os.rename`, `os.remove`,
  listing and existence checks as plain; review round 11 defeated that.
  The facilities (`file_reads::FILE_FACILITIES`):
  * the builtins `open` and `file` (the path; no mode is exempt);
  * every path-taking attribute of `os` and `os.path` that is vetted:
    `access`, `listdir`, `lstat`, `makedirs`, `mkdir`, `remove`,
    `removedirs`, `rename` and `renames` (both paths), `rmdir`, `stat`,
    `unlink`, `utime`, `walk`, and `os.path.exists`, `getatime`,
    `getctime`, `getmtime`, `getsize`, `isdir`, `isfile`, `islink`,
    `ismount`, `lexists`, `realpath`, `samefile`, `sameopenfile` and
    `walk`. Every vetted attribute of `os`, `os.path` and `sys` is either
    one of these or reviewed as path free (`file_reads::PATH_FREE`:
    constants, the environment, `getcwd`, `strerror`, `urandom`, the
    console streams, and the pure string functions of `os.path` such as
    `join`, `basename`, `splitext`, `normpath`, `abspath`), and
    `every_vetted_os_attribute_is_classified` fails on a vetted attribute
    that is neither or both. The other vetted modules take no path
    (reviewed: `json.load` and `json.dump`, `base64.encode` and
    `base64.decode` take file objects, whose source is counted where it
    is made);
  * Ignition: `system.file.readFileAsString`, `readFileAsBytes`,
    `writeFile`, `fileExists`, `getTempFile` (fine with a literal
    extension without a separator or `..`), `system.tag.exportTags`, the
    dataset and Vision exports and `printToImage`,
    `system.iec61850.readFile` and `writeFile` (local paths), the legacy
    Vision exports `system.db.exportCSV`, `exportExcel` and `exportHTML`,
    the legacy `system.net.httpGet`, `httpPost`, `httpPut` and
    `httpDelete` and `openURL`, which take any URL string (`file:`
    included, not verified otherwise, so failing closed; the `http*`
    functions count on every call anyway, see the outputs below), and, on
    every call, the file dialogs,
    `system.file.saveToFile`, `system.util.playSoundClip`,
    `system.report.executeAndDistribute` (its distribution actions write
    files) and `system.db.addDatasource` and `setDatasourceConnectURL` (a
    JDBC URL may name a database file). Every `system.*` function with a
    parameter whose name or type looks like a local file (`file`,
    `filename`, `filepath`, `url`, `uri`, `location`, `destination`,
    `dir`, `folder`, `wav`, a `File` type), or whose own name contains
    `file` or starts with `export` (for parameters Ignition does not name),
    is a facility or
    reviewed as not touching local files
    (`file_reads::SYSTEM_NOT_LOCAL_FILES`: OPC folders and endpoints, the
    browser's download name and navigation, `system.perspective.print`'s
    console destination, a report's format), checked by
    `every_system_file_parameter_is_classified`;
  * `system.*` functions whose **outputs** carry content that the gateway
    loaded from workspace resources, whatever their parameters
    (`file_reads::SYSTEM_OUTPUTS`, review round 12). A path-based audit
    misses them: `system.tag.getConfiguration('[default]T')` returns the
    tag's event scripts, comments and whitespace included, and
    `system.tag.exportTags('/tmp/x.json', ['[default]T'])` followed by
    `open('/tmp/x.json')` copies them to a file that a literal outside
    path then reads (both verified on 8.3.3). Every function and object
    of the stubs (683, all scopes) is classified by what its return
    value, the files it writes and what it hands to callbacks carry, and
    `every_system_function_output_is_classified` fails on one in no class
    or in two:
    * workspace content, on every use (called or as a value): tag
      configuration (`getConfiguration`, `exportTags` to a string or to
      any file, `query`, `browse`, `browseTags`, `browseTagsSimple`,
      whose `attributes` reveal which tags have scripts, `getAttribute`,
      `getAlarmStates`), project information with modification data
      (`system.perspective.getProjectInfo`), the audit log (it may record
      resource changes; failing closed), Vision windows and components,
      which hold their window's definition (`system.gui.getWindow`,
      `system.nav.openWindow`, `system.vision.getParentWindow`, ...),
      `system.file.getChooser` (it hands out local files) and the
      undocumented `system.device.getInstance` and
      `system.util.initialize`;
    * tag reads (`readBlocking`, `readAsync`, `read`, `readAll`,
      `getTagValue`, `getTagValues`): a path that names a configuration
      property returns that configuration (verified on 8.3.3:
      `readBlocking(['[default]T.eventScripts'])` and
      `getTagValue('[default]T.eventScripts')` return the script text).
      A read is fine only as a direct call whose path argument (the first,
      or `tagPaths=` / `tagPath=`) can only name values, with the rule of
      the `system.tag` writes (see "`system.*` functions that run code"):
      a literal, a concatenation ending in a literal with `/`, or a `%` or
      `format` string, whose text after the last `/` names no property,
      or `Value`, `Quality` or `Timestamp`. Computed paths count;
    * network clients (`system.net.httpClient`, `httpGet`, `httpPost`,
      `httpPut`, `httpDelete`, and `java.net.URL` with a network URL): the
      gateway that runs the script, or another gateway that shares the
      workspace's files, serves their content over HTTP (the 8.3 REST
      API's project and resource exports, WebDev resources), and no host
      name can be proven not to reach one (`localhost`, an alias, a
      proxy, the gateway's own DNS name). They count on every use. There
      is no list of trusted hosts: declaring a host harmless is the same
      declaration as `assume-scripts-do-not-read-workspace-files`, which
      the user can make;
    * values only (`file_reads::SYSTEM_VALUE_ONLY`, reviewed per
      package): run-time values that the running configuration produces
      (tag history, alarm events and states, query results, OPC values),
      settings values (a device's host name, a roster's users), names (no
      fix creates, renames or deletes a resource) and data the script
      passes in. A safe fix keeps behaviour, so it keeps them; what it
      changes is the representation of definitions (script text,
      comments, whitespace, unused imports and properties), which only
      the outputs above expose. The Ignition OPC UA server exposes tags
      as value nodes only (verified on 8.3.3 with exposed tag providers:
      no property nodes, and `[default]/T.eventScripts` is
      `Bad_NodeIdUnknown`), so OPC reads are values;
    * not applicable: the functions that run code
      (`vetted::SYSTEM_RUNS_CODE`), unknown code on every use anyway;
  * references in modelled configuration that hand tag configuration to
    a value: a tag path whose text after the last `/` (or `]`) has a
    `{...}` placeholder or names a property other than `Value`,
    `Quality` and `Timestamp`, or that is computed, in an expression
    (`{[.]T.eventScripts}`, `tag(...)`), a Perspective tag binding
    (direct or indirect; expression mode always counts), a reference or
    derived tag's `sourceTagPath` (literal or bound), an alarm property
    bound to a tag, or a gateway tag change script's paths
    (`summary::tag_path_names_value`). Verified on 8.3.3: an expression
    tag on `{[.]T.eventScripts}`, one on `tag('[.]T.eventScripts')` and a
    reference tag on `[.]T.eventScripts` all have the event script text
    as their value, so any value read (a script's `readBlocking`, a
    binding, the OPC UA server) would carry it; the reference is the
    read site, and value reads stay values;
  * vetted Java classes that make file handles from strings
    (`file_reads::JAVA_FILE_POLICY`): `java.io.File`, `FileInputStream`,
    `FileReader`, `FileOutputStream` and `FileWriter` (path arguments,
    reading and writing alike), `PrintWriter` and `java.util.Formatter`
    (fine without arguments, with an outside literal or with a direct
    constructor call of a vetted Java class without a file policy, as in
    `PrintWriter(StringWriter())`; anything else may be a file name),
    `java.net.URL` (only an outside `file:` literal is fine: a network URL
    is a network client, see above) and `URI` (an `http`, `https` or
    `ftp` literal is fine, a `file:` literal is a path; `URI` reads
    nothing by itself, and `toURL` is a measured file member). Classes that only take a handle
    or stream the script already holds (`Scanner`, `HttpURLConnection`,
    readers and writers wrapping a stream) are plain: the handle was
    counted where it was made. Which vetted classes take or are handles
    is measured (`JAVA_FILE_CLASSES`) and
    `every_measured_file_class_has_a_policy` fails on one without a
    policy;
  * any member named in `JAVA_FILE_MEMBERS`, on any value but a vetted
    Python module, as an attribute or as a literal `getattr` name: the
    members whose declared type mentions a file handle (`java.io.File`,
    `FileDescriptor`, `FileInputStream`, `FileReader`, `RandomAccessFile`,
    `java.nio.file.Path`, `FileSystem`, `FileStore`, `DirectoryStream`,
    `FileSystemProvider`, `BasicFileAttributes`, file channels,
    `ZipFile`, `URL`, `URI`, `URLConnection`, Ignition's
    `ProjectManager`, resource collections, resources, snapshots and
    resource data files, which hand out resources as the gateway loaded
    them from the files a fix edits, and, since review round 12, the
    configuration objects that carry that content: Perspective view,
    component, page, session, binding, event, script and project
    configurations, `PerspectiveProject` and its cache, `ViewModel`, tag
    configurations, `TagEventScripts`, alarm configurations and
    definitions, gateway event script configurations, named queries and
    decoded gateway resources; a probe of every Ignition type reachable
    from the seeds whose name mentions a configuration, definition,
    script, resource, snapshot, project or query chose them). It is measured by
    `stubs-gen/gen_vetted.py` as a second fixed point over the same seeds
    as the reflection closure (see "Machine-measured facts"), with the
    handle types as stops, except the handles vetted code can hold without
    a recorded hop (`File`, `FileInputStream`, `FileReader`, `URL`, `URI`),
    whose members are scanned so that navigating from one (`getParentFile`,
    `toPath`, `resolve`, `openConnection`) is a recorded hop too. A method
    that needs a handle the script cannot hold without a recorded hop
    (`LastModification.update(Resource, String)`) is not a first hop. Seeds
    are the gateway scope only: `system.*` return types of gateway
    variants and the injected objects without Vision's (see below). For
    8.3.3: 2,188 types scanned, 88 names (`getDataDir` on the WebDev
    context's system manager, `getProjectManager`, `getParentFile`,
    `toPath`, ...);
  * a file facility or Java file class used as a value (`o = open`,
    `map(open, paths)`, `F = File`): what is done with it is not followed;
* code that is not vetted or reads frames (`vetted::unknown_code`'s other
  uses, `dynamic::reads_frames`), imports that are neither vetted nor a
  project library module, resources and expressions whose code iglint does
  not read, and code hidden by a model problem: they may reach `open` under
  another name (`f_builtins`, `eval`, a module that is not vetted). Not
  code that only reaches other views (Perspective pages and sessions): it
  cannot read files. This is fail closed: an unvetted Java class (an
  Ignition class, say) may well read no file, but its methods that take a
  string cannot be measured for whether the string is a path;
* every Vision client script (client event scripts): the Vision client's
  objects are not seeds of the measurement, so a Vision client script
  counts as a possible read. Vision windows and templates are resources
  iglint does not read, so a workspace with Vision clients already counts;
  library code runs in a Vision client only when one of those exists.

**An access is fine** only when each of its paths is a string literal, a
concatenation of literals or `os.path.join` of literals, that
`file_reads::outside_workspace` proves names no file iglint edits. Files
iglint edits are resources, always below a module directory (`ignition`,
or a module id, dotted for every module of 8.3) of a project
(`.../projects/<name>/...`, or a project directory linted on its own) or a
gateway collection (`.../config/resources/<collection>/...`), plus the
manifests `project.json` and `resource.json`; the workspace may be
deployed anywhere, so the rule is about the path's shape, not the
workspace's location. A path is possibly in the workspace when it is
relative (the gateway's working directory is its installation
directory), starts with `~`, has a `..` component (a symbolic link makes
`..` leave a directory), a percent escape, or a component that is
(case-insensitively, trailing dots and spaces ignored as on Windows)
`projects`, `config`, `resources` or `ignition`, contains
`inductiveautomation`, is a directory name with a dot after its first
character (a module id), or is a manifest. `/tmp/report.csv` and
`C:\exports\x.csv` are outside. A literal prefix with a computed rest
(`'/tmp/' + name`, `os.path.join('/tmp', name)`, `'/tmp/%s' % name`) is
not accepted: the rest may hold `../` or, for `os.path.join`, an absolute
path that discards the prefix. The regression tests are
`vetted::tests::file_reads`,
`every_vetted_os_attribute_is_classified`,
`every_system_file_parameter_is_classified`,
`every_system_function_output_is_classified`,
`outputs_that_carry_workspace_content_are_reads`, `file_reads::tests::*`, the
rule tests `file_reads_make_the_fix_unsafe` (`SCR124`),
`file_reads_make_the_removal_unsafe` (`SCR102`),
`file_reads_make_fixes_unsafe` (`VIEW015`), the binary tests
`fixes_are_unsafe_when_scripts_read_workspace_files`,
`moves_and_appends_of_workspace_files_make_fixes_unsafe`,
`fixes_are_unsafe_when_scripts_read_tag_configuration` and
`reads_of_files_outside_the_workspace_keep_fixes_safe`, and the language
server tests `quick_fixes_are_unsafe_when_scripts_read_workspace_files`,
`quick_fixes_are_unsafe_when_scripts_move_or_append_to_workspace_files`
and `quick_fixes_are_unsafe_when_scripts_read_tag_configuration`.

**Why outside literals stay fine.** A literal path outside the workspace
can show workspace content only through a copy or an alias: a rename,
copy, hard or symbolic link that puts a workspace file there, or a write
of content that came from one. Every vetted way to make one is itself
counted. Either it takes the workspace path as an argument (`os.rename`,
`os.renames`, `File.renameTo` on a `File` made from a path), or the
content it writes came from a counted source: a read of a workspace
path, an output that carries workspace content (`system.tag.exportTags`
writes the tag configuration to the file it is given, whatever the path;
`getConfiguration`, a configuration property read, a network client or
a configuration object hand it to the script, which may write it
anywhere), or a value that a counted reference fills (an expression tag
on `{[.]T.eventScripts}`). So the script, expression or binding that
makes the copy already withholds safe fixes. An earlier version of this
argument covered only copies that name the workspace path; review round
12 defeated it with `exportTags` to `/tmp` followed by `open` of the same
literal. Vetted code cannot create links
(`os.link` and `os.symlink` are not vetted; `java.nio.file` is not
vetted). Symbolic links that exist before the script runs, made by an
administrator or another process (`/tmp/x -> .../projects/p`), are
outside the closed world like other processes (below): a literal path is
judged by its text, not by resolving links on the machine that lints,
which is not the gateway. Failing closed for every literal under a
temporary or world-writable directory was considered and rejected: it
would make `/tmp/report.csv` count while giving no guarantee for other
directories, where an administrator can make the same links.

**What fixes do to the files.** A fix writes the new text to a
temporary file next to the target and renames it over the target,
keeping its permissions (`engine::write_atomically`), so after the fix
the directory holds the same names; the temporary name exists only
while iglint runs. The rename makes a new inode (and, when iglint runs
as another user, a new owner). Listing and existence checks of
workspace paths therefore do not observe a fix by themselves, but they
count anyway under the default-deny rule above.

**Not covered**, outside the closed world like projects outside the
workspace: other processes that read the files (a database server on the
same machine, an editor, a file sync, another gateway's scripts), copies
the gateway makes on its own (scheduled gateway backups to a configured
folder, which a literal outside path could name, and logs), tag
providers and expressions outside the workspace that read the
configuration of workspace tags (a reference tag of a provider the
workspace does not hold), Java methods in
the measured closure that read a file named by a string argument without
a handle type in their signature (a review of every scanned method with
a string parameter and a name containing `read`, `load`, `open`,
`import`, `file`, `path`, `resource`, `stream` or `content` found only
the servlet context's web application resources, class path resources,
named queries as loaded by the gateway, which fixes do not edit, and
tag imports from a string of tag JSON), and project information that
Perspective hands out as data (`session.getProjectInfo()`: titles,
page and view paths, which no fix changes).

**The opt-in.** `[fix] assume-scripts-do-not-read-workspace-files = true`
in `iglint.toml` (default `false`) declares that no code reads the files
of the workspace; checking that is the user's responsibility. It lifts
only this gate: the closed-world conditions of `SCR102`, `VIEW015` and
`NAME001` (code that is not vetted, frame readers, other views) still
apply.

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
| `os` | Only path and file functions and constants (`path`, `sep`, `getcwd`, `listdir`, `stat`, `environ`, `getenv`, `makedirs`, `remove`, `rename`, `walk`, ...). Not processes or anything else. Every path-taking one counts as a possible file access (see "File reads"). |
| `sys` (also as Ignition's implicit global) | Only `version`, `version_info`, `hexversion`, `platform`, `maxint`, `maxsize`, `maxunicode`, `byteorder`, `float_info`, `stdout`, `stderr`, `exit`, `getdefaultencoding`, `getfilesystemencoding`, `getrecursionlimit`. Not `_getframe`, `modules`, `exc_info`, `path` (it decides which module an import loads), trace hooks. |
| `system` (Ignition's scripting API) | No `system.*` function of 8.3 evaluates Python code in the caller's frame or resolves a callable by name in it (checked against the generated stubs): functions that take callables call the value, message handlers are looked up among the project's handlers, not in the caller. Except the functions that store or run code from run-time values, which are not vetted (see "`system.*` functions that run code"). A `system` package used as a value (`t = system.tag`) is not vetted either. |
| `__future__` | Compile-time only. |
| `java.util` (except `ServiceLoader` and the resource bundles, which load classes by name), `java.util.concurrent` (and `atomic`, `locks`), `java.util.function`, `java.util.regex`, `java.util.stream`, `java.math`, `java.text`, `java.time` (and `chrono`, `format`, `temporal`), `java.nio.charset` | Data structures and formatting; no reflection. |
| `java.lang`: `Math`, `StrictMath`, `String`, `StringBuilder`, `StringBuffer`, the boxed number types, `Boolean`, `Character`, `Number`, `System` (except `load`, `loadLibrary`, `mapLibraryName`, security managers, `inheritedChannel`), `Exception` and common runtime exceptions | Not `Class`, `ClassLoader`, `Thread`, `Throwable`, `Runtime`, `Process*`, reflection, `invoke` or `StackWalker`. The exception classes hand out only stack traces (class, method, file and line), which safe fixes never change. |
| `java.io`: readers, writers, file and byte streams, `File`, `IOException` | Not object streams: deserialization instantiates classes named by the data. Their file accesses are checked separately (see "File reads"). |
| `java.net`: `URL`, `URI`, `URLEncoder`, `URLDecoder`, `InetAddress`, `HttpURLConnection`, common exceptions | Not class loaders. |
| Builtins: every builtin except those below; every exception class | `open` and `file` are checked separately, in every mode (see "File reads"). Not `eval`, `execfile`, `compile`, `__import__`, `reload`, `input` (it evaluates what it reads), `locals`, `globals`, `vars`, `dir`, `help` (it imports modules by name). `getattr`, `setattr`, `delattr` and `hasattr` only as direct calls with a literal name (route 2 of `dynamic`). |

Everything else is unknown: `cPickle`, `pickle`, `copy_reg`, `shelve`,
`marshal`, `warnings`, `logging`, `traceback`, `inspect`, `timeit`, `code`,
`codeop`, `runpy`, `pkgutil`, `doctest`, `pdb`, `profile`, `trace`,
`unittest`, `socket`, `xml`, `org.python.*`, `com.inductiveautomation.*`
(Ignition's script manager can run code), relative imports, `import
java...` of whole packages, and any Java class or module not listed. The
list is a precision trade-off, not a claim that those are harmful: adding
an entry needs the same check on Jython.

### `system.*` functions that run code

Every function of `crates/iglint_stubs` (678, all scopes) was audited for
"makes Python code from a run-time value run" and "looks up project code
by a run-time name". Not vetted (`vetted::SYSTEM_RUNS_CODE`, unknown code
on any use, called or not):

| Function | Why |
|---|---|
| `system.tag.configure`, `addTag`, `editTag`, `editTags`, `editAlarmConfig`, `configureManagedTag`, `configureManagedProvider` | Tag configurations hold expressions (`runScript`) and tag event scripts |
| `system.tag.importTags`, `system.tag.loadFromFile`, `system.groups.loadFromFile` | Import tag or transaction group configurations from a file |
| `system.tag.copy`, `system.tag.move` | Copy or move tags, with their scripts, from any provider (`move` since review round 12) |
| `system.util.execute`, `system.system.execute` | Runs an operating system command |
| `system.project.requestScan`, `system.vision.updateProject` | Loads project resources as they are now on disk or published |
| `system.eam.runTask` | Runs an EAM task, which may run scripts configured on the gateway |
| `system.util.WRAPPED_CONTEXT`, `system.system.WRAPPED_CONTEXT`, `system.alarm.SERIALIZER` | Objects that hand out the script context or a serializer of arbitrary classes |

`system.tag` writes (`write`, `writeAll`, `writeAllSynchronous`,
`writeAsync`, `writeBlocking`, `writeSynchronous`, `writeToTag`,
`writeToTagSynchronous`, `writeToTags`) can store code too: a path may
name a configuration property (`Tag.expression`, `Tag.eventScripts`).
They are vetted only as direct calls whose path argument (first argument
or `tagPaths=`) is a literal path, or a list or tuple of them, whose text
after the last `/` (or `]`) names the tag's value (no property, or
`Value`, `Quality`, `Timestamp`); a concatenation ending in such a literal
that contains `/` (`base + '/Speed'`); or a `%` or `format` string whose
text after the last `/` is such a literal. Any other path, unpacked
arguments, or the function used as a value is unknown code.

Vetted after the audit: message sends (`system.util.sendMessage`,
`sendRequest`, `system.perspective.sendMessage`) run handlers of the
workspace, which are analysed; `system.report.executeReport` and
`system.sfc.startChart` run reports and charts, which are unknown code as
resources (below); `system.db.*` runs SQL, not Python; the remaining
functions take values. Not covered: files that `system.file.writeFile`
writes into a project directory take effect only after a resource scan
(`requestScan`, not vetted) or a gateway restart; projects and gateway
tags outside the workspace are outside the closed world.

### Expressions and resources iglint does not read

Expressions run Python through `runScript(script, [pollRate],
[args...])`, the only expression function of 8.3 that does (Vision's
`objectScript` exists only in Vision clients). Its first argument is a
Python expression evaluated in the project's scripting context, called
with `args` when it is callable. In every expression iglint models
(Perspective bindings and transforms, session properties, expression
tags, derived tag getters and setters, bound tag and alarm properties):

* a literal dotted name (`runScript("lib.util.json.dumps", 0, x)`) is a
  library reference, like the same name in a script: the project index
  counts it as a use of every module attribute along the path;
* any other literal is code: it is parsed and analysed like a script
  (vetting, frame reads, library references), and if it does not parse it
  is unknown code;
* a computed first argument is unknown code.

Resources of kinds iglint identifies but does not read hold code or
expressions it cannot analyse: reports, alarm pipelines, SFC charts,
Vision windows, templates and other Vision resources, transaction groups,
event streams, the deprecated app script library, pre-8.3 aggregate event
scripts and resources of unknown modules. Each is unknown code. So is any
JSON string of a modelled resource that mentions `runScript` but is not an
expression or script iglint reads. The site of each is named in unsafe
fixes (`script_facts::code_sources`).

A view, session property or tag file part that has the wrong JSON type is
skipped by the model (`VIEW021` reports the view and session ones). When
the skipped part is a place for scripts (a component, `events`,
`scripts`, `propConfig`, an action, binding or change script, a custom
method without a string name, a script that is not a string, a tag) and
contains a string, the code there is unknown code
(`ModelProblem::hides_code`).

Resource manifests and data files outside every project and gateway
configuration collection are not loaded by Ignition (verified on 8.3.3: a
collection directory without `config-mode.json` is ignored, its tags do
not exist, and a `core` directory without one keeps the gateway from
starting). Their scripts never run, so they are not unknown code; `REF009`
reports each such file, so that nothing iglint lists goes unread without a
finding (`every_listed_file_is_analysed_or_reported`).

### Perspective fields and component names

Which binding and transform fields Perspective parses, and which members
of the component script object return the component's name, are
measured in the Ignition 8.3.3 image by `stubs-gen/gen_perspective_fields.py`
(`stubs-gen/perspective-fields.sh`, Jython and the image's jars, no
gateway) into `crates/iglint_project/src/generated/perspective_fields.rs`:

* **Fields.** Every binding and transform type registered by
  `BindingRegistryImpl` is built through the registry from configurations
  generated from the module's own JSON schemas (every alternative and
  every enumerated value, with Java enum spellings where Perspective uses
  `Enum.valueOf`), repaired where a schema default is rejected, with a
  recording `BindingContext`. One position at a time (every value, keys
  and nested values of free-form objects, and identifier-like string
  constants of the binding classes as extra keys) holds a marker
  reference; a field is an expression when the marker reaches the
  element's property reference parser, a property path when its raw
  text does, and it carries the enumerated value that decides it
  (`tag` `mode`, HTTP `auth.type`, map `inputType`/`outputType`, polling
  `enabled`). Completeness: every Perspective method in any jar of the
  image that calls `PerspectiveExpression.create`,
  `PerspectiveExpressionFactory.apply`, `PerspectiveTransformExpression.create`,
  `ELParserHarness.parse` or `createPropertyReference` must have been on
  the stack of a recorded marker or be classified as not reading view
  configuration (the parsers themselves, `{...}` references inside an
  expression, the `property(path)` function, the model's own reference
  implementations, Designer code); otherwise the generator fails. So no
  event action, component configuration or binding type parses an
  expression iglint does not model. A type that cannot be built without a
  gateway (the script transform) is accepted only when no method of its
  package calls a parser. The view model reads every listed expression
  field (`every_measured_field_is_modelled`); `generated_fields_match_the_generator`
  fails when the generator changes without regenerating.
* **Name members.** A return-value flow analysis of the bytecode marks a
  method as returning the name when a value it returns may derive from
  `PerspectiveElement.getName()` (through calls, string building and
  locals); it runs over every public member of
  `ComponentModelScriptWrapper$SafetyWrapper` and of every component model
  delegate and finds `name` and `getName`. The generator checks itself on
  `PerspectiveElement.getQualifiedPath`, which builds a string from the
  name. The access analysis records a use of a measured name member on a
  component as a read of its `meta` `name`.

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
| `NAME001` | Rename a component and every reference to it in the view | No reference to the component at all (references are resolved in every binding and transform field Perspective parses, see "Perspective fields and component names"); no script reads the component's name (`meta.name`, or the measured name members `name` and `getName()`, on the component itself or on a component the access analysis reaches through `children`, `getChildren()`, loops and indexes; unknown members read everything); no script looks children up with a computed name or has a string literal equal to the name; every script parses; no script of the view hands a component to code outside the view (library call, message payload, `getGlobals()`, an opaque call such as `locals()`), reaches a frame builtin in any way, calls an unnamed value or calls a library name that is not a plain function, class or literal constant: a library function that receives a component, or `eval` under another name, can call `getChild('Name')` itself; every script of the view uses only vetted code (see "Threat model"); no string of the view that is not a modelled expression, property binding path or script holds a `{...}` reference-like token; no reference has whitespace inside its braces (`{ ../Name.props.x }`, a tag path to Perspective); and no string of the view (values and keys, expressions included, scripts and component names excluded) holds the name as a word where the fix does not update it | Unsafe when references exist, a component may escape, a script reads the name, an unread string may hold a reference or the name, or a script uses code that is not vetted (`cPickle.loads` of an `eval` opcode); the fix names the first such place. No fix when a lookup cannot be rewritten or the new name is taken. Another view's script (or any code handed a page, session or one of its views) can look the component up: any such use in the workspace is unknown code under the closed-world rule. No other resource names a view's components: property references resolve in their own view, page or session |
| `VIEW012` | Set, add or remove `access` in `propConfig` | Never: the access mode decides whether the browser receives the property and whether it may write it | Unsafe |
| `VIEW013` | Set `persistent: false` (and remove the stored value) | No stored value: the property starts out absent either way; and no code in the workspace may read workspace files ("File reads") | Unsafe when a stored value is removed: scripts and expressions that run before the binding delivers (query bindings are asynchronous) see a different value |
| `VIEW015` | Remove an unused custom property and its `propConfig` entries | Nothing reads or writes the property, and the view's own scripts prove it: resolved reads and writes cover expressions, bindings (bidirectional ones write) and scripts, analysed default-deny (every use of a component, the view, a page or session, a property container or an alias of one must be one of the forms listed under "`VIEW015`: understood forms" below; anything else reads and writes everything it reaches); a script that reads frames or reaches a frame builtin in any way reads everything, and every library name a script calls or uses as a value is checked to be a plain function, class or literal constant (or missing); properties with a binding or change script are never reported | Unsafe when a script, a library function it calls or a bidirectional binding writes the property (the write recreates it at run time without the `access` and `persistent` settings of the removed `propConfig` entry); unsafe when the finding relies on following a value into library functions (inter-procedural: the function's body, a function that does not exist, the depth limit); unsafe for a view linted outside its project whose scripts call names that may be library functions; unsafe when a script of the view, or a library module it imports or calls, uses code that is not vetted; unsafe when any code in the workspace reaches other views through a page or session (closed world, "Other views") |
| `SCR102` | Remove unused import names or statements; the safe fix keeps the emptied lines, the unsafe fix removes them | Not in a class body (never reported); not a module-level name the module may read dynamically (never reported); not a top-level name of a library module that another resource reads (`pkg.mod.name`, a literal `getattr` or `hasattr` name) or uses as a value (a default argument, a container, `getGlobals()`, `sys.modules`; never reported); a top-level library import is provably unused by other resources and no resource of the project uses code that is not vetted; every removed import is known to succeed and to be effect-free: a measured Java class or package of the vetted packages, or a plain `import x` / non-package `from x import y` of a standard module on the verified list that was measured to import (and to have `y`), with no project library package of that name; the module uses only vetted code and calls no library name that may be a frame builtin; the import is not the only binding of a name still read in its scope or a nested scope; it does not come before a `from __future__` import; and, when the whole statement goes from a library module (where every import that runs binds `__package__`, see "Names imports bind"), nothing may observe `__package__` or `__warningregistry__`: no read or binding of either in the module, no binding of `__name__`, no read of the module's globals by name, and no other resource reading them, the module's `__dict__` or the module as a value (`external_use`, unknown counts as a reader) | Unsafe (project library modules, third-party and non-JDK Java packages, Java classes or module attributes not measured to exist, `socket`, `logging`, dotted imports, relative imports, unknown project, a library name that is not a plain function, class or literal constant, code that is not vetted, a name that would resolve differently, a misplaced future import that would become valid, a `__package__` that code may observe) |
| `SCR103` | Remove `x = value`, or only `x = ` when the value calls or binds something | Never | Unsafe: the value may be wanted under another name, and a removed attribute or subscript read, operator, comparison, `in` or `%` formatting can raise or run code (getters, special methods). The statement is removed only when the value calls nothing (no call, no backticks, which call `repr`, no `yield`) and binds nothing (no list comprehension, whose variables Python 2 leaks), so no pointless statement (`SCR114`) is left |
| `SCR110` | `is` to `==`, `is not` to `!=` | Never: the result changes whenever identity and equality differ (`int('1000') is 1000` is `False` in Jython), and `==` calls `__eq__` / Java `equals` | Unsafe |
| `SCR115` | `except A, B:` to `except (A, B):` | Never: the handler then catches `B` and no longer rebinds the name | Unsafe. The finding itself requires `B` to resolve (ignoring the handler) to a class statement, a capitalized `from` import or a builtin exception, and the handler body not to read it |
| `SCR123` | Remove unused suppression codes or the whole comment (a comment alone on its line leaves the line empty) | Only codes are removed (the rest of the comment stays), or the comment holds nothing but the suppression. Comments do not run, and no line moves; and no code in the workspace may read workspace files ("File reads") | Unsafe when the whole comment would go and it also holds a reason, a `# pylint:` directive or a `coding:` declaration; that fix removes the line |
| `SCR124` | Remove trailing whitespace | The script parses, the whitespace is outside string literals and outside the generated header (a backslash continuation followed by spaces is a syntax error, so it is never touched); and no code in the workspace may read workspace files ("File reads") | No finding when the script does not parse; unsafe while code may read workspace files |
| `SCR206` | Remove a `shared.` / `project.` prefix | Never: in Ignition 7 the two prefixes named different libraries | Unsafe |
| `SCR301` | Rewrite or insert the event script header | Never: code may rely on the written parameters or defaults | Unsafe |
| `SCR900` | ign-lint's trailing-whitespace fix (every line `rstrip()`ed) | The script parses and no removed whitespace is inside a string literal (multi-line strings); and no code in the workspace may read workspace files ("File reads") | Unsafe |

Every row above also needs the condition of "File reads": while code in
the workspace may read workspace files, every fix is unsafe, whatever the
rule (`NAME001`, `SCR102` and `VIEW015` included), unless the project sets
`[fix] assume-scripts-do-not-read-workspace-files = true`.

Rules not listed offer no fix. The language server uses the same
applicability: unsafe code actions are titled `(unsafe)`, and the
fix-all action applies unsafe fixes only when configured to. When code
that is not vetted, or that reads a frame, is what keeps a `SCR102`,
`VIEW015` or `NAME001` fix from being safe, the fix carries the reason
and the construct: the first use in the script itself (`re.sys` at
`m/code.py:2:12`), else, under the closed-world rule, the first use in the
workspace by path and offset (`ProjectIndex::first_unknown_code`). The
text output adds a `note`, JSON an `unsafe_reason` object in `fix`, SARIF
a related location, and the language server a related location and the
code action title `(unsafe: <reason>)`. A workspace site is filled in after
the cache from the current index, so a finding restored from the cache
names the site as it is now; moving the site does not change any summary. Every path
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
attributes named in source or in a string literal in a sink position (`.f_locals`,
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
| `VIEW013` | read `view.json` itself; the running scripts cannot see the `persistent` flag, and without a stored value the property starts out absent either way | "File reads" (`dynamic_scripts_do_not_change_the_fix` for the running scripts) |
| `VIEW015` | read or write any property | Default-deny accesses, frame routes, callee checks, rebinding through exposure and `func_globals`; inter-procedural findings unsafe; code that is not vetted makes the fix unsafe (`dynamic_patterns_read_everything`, `unvetted_code_makes_fixes_unsafe`) |
| `SCR102` | read the import by name (`lib.util.ev('os')`, `capture()['os']`, `getattr(me, n)` on the module itself) | Scopes with escape routes or frame builtins are dynamic (no finding); a computed `getattr` on a module object keeps module imports; a library name that may be a frame builtin, or code that is not vetted, makes the fix unsafe (`library_frame_builtins_and_computed_getattr`, `frame_builtins_under_other_names_keep_imports`, `review_round_six_repros`) |
| `SCR123` | read the script's file (`open(path).read()`); through the running code nothing: comments do not run, and the safe fix keeps every line, so line numbers (`f_lineno`, tracebacks, `warnings`, Java stack traces) do not move | "File reads": dynamic code may call `open` under another name, so it makes the fix unsafe unless the project declares that no script reads workspace files (`safe_removal_keeps_lines`) |
| `SCR124`, `SCR900` | read the script's file; through the running code nothing: trailing whitespace outside string literals is not in any value, line numbers do not move, and Jython has no source for scripts compiled from strings (`inspect.getsource` raises `IOError`, tracebacks show no source lines; verified on Ignition 8.3's Jython) | "File reads" (`dynamic_code_makes_the_fix_unsafe_unless_declared`, `file_reads_make_the_fix_unsafe`) |

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
* **Import side effects.** Removing an import is safe only with positive
  evidence that the import succeeds and has no effect: a Java class
  measured to exist in the image (a public top-level class of a vetted
  Java package or of the package of a vetted class, `JAVA_PACKAGE_CLASSES`)
  or such a package, and a standard library module verified on Jython not
  to start threads, register handlers or print (`PURE_STDLIB_MODULES` in
  `unused_import.rs`) that was measured to import in the image, with, for
  `from m import n`, `n` measured to be an attribute of `m`
  (`MODULE_ATTRS`). Dotted imports of Python modules are never safe (they
  make the submodule an attribute of its package for every other script).
  An earlier version accepted any capitalised name of a vetted Java
  package: removing `from java.util import MissingClass` turned a failed
  call into a successful one when the caller caught the `ImportError`
  (verified on Jython: `failed` before the fix, `succeeded` after). Project
  library modules are never safe to remove: importing them runs their top
  level, which in Ignition often registers loggers, caches or message
  handlers.
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
    `.page`, `.session` and their getters (a host: only its own property
    trees, names, info, `.print`, `.close`, `.isConnected`,
    `.refreshBinding`, `.invokeOnQueue` and `.session` are understood on
    it; its other members reach other views, see "Closed world"), the
    harmless members `.props`, `.meta`, `.position`, `.name`,
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
  * a loop variable bound only by `for c in <children>:` (any
    component), and truth and identity tests (`if c:`, `not c`, `c is
    None`) of a component, view, page or session, which read no property;
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
