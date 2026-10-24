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
   use as reading and writing everything; every introspection route of
   `iglint_python_semantic::dynamic` withholds the safe fixes that depend
   on what code reads, and the routes that read or write names by
   themselves also make every scope of the module read its locals and
   write its globals (see "Frame routes: findings and fixes"); callee
   checks resolve library names to plain functions, classes and literal
   constants.
3. **The post-fix safety net** (`crate::safety`, run by the engine for
   `--fix`, `--diff` and the language server's fix-all). A safe fix on
   Python code is dropped when, for any script of the file, it moves a
   line, changes whether the script compiles (syntax and compile errors,
   in either direction), changes the tokens of a script whose compile
   status is not known (no error is certain and its size bounds lie on
   both sides of a limit, see "Size limits" under "Decisions"; an edit
   that leaves every token where it was, proven by lexing before and
   after, compiles to the same code whatever the status), changes a
   docstring or the `__future__` imports, changes the scopes (generator
   status, `global` declarations, `import *`, `exec`, frame reads) or
   whether the script has an escape route of either kind (see "Frame
   routes: findings and fixes"), or changes what any remaining name resolves
   to (local, enclosing function, global, builtin, injected, and where a
   class body's lookup continues), which kinds of bindings it may find,
   or whether it may be unbound, or removes a read of a name (how many
   references of each kind a name has is compared, both ways), or changes
   the number literals or the number of string literals; the
   interpreter's implicit reads count (see "Name resolution, measured").
   A layout-only fix (whitespace or a comment: `SCR123`, `SCR124`,
   `SCR900`'s trailing-whitespace fix) must in addition leave every token
   of every script as it is, relative to the other fixes applied with it
   ("Layout-only fixes" under "Decisions"). Names that statements bind without naming them are
   bindings of the semantic model too: `__module__` in class bodies and
   the `__package__` that every import binds in a library module's
   globals (see "Names imports bind"), so removing the only import of a
   module that reads `__package__` changes what the read finds. It runs
   on every storage format: `.py` library modules
   and event files, and scripts in `view.json` and other resource JSON
   (event actions, transforms, change scripts, message handlers, custom
   methods, session and tag event scripts), each compiled with its
   generated header; `every_check_runs_for_every_script_kind` enumerates
   them. For a `view.json` the net also compares the view's
   reference-resolution map before and after the safe fixes, in memory
   (`safety::resolution`, review round 21, B1): every property reference
   of every modelled expression and of property and indirect tag bindings
   (resolved as `BIND005` resolves them: target, category, property,
   whether it is defined, ambiguous or unchecked, or why it does not
   resolve), tag paths, every `getSibling`/`getChild` lookup of every
   script (receiver and result as the access analysis follows them, as
   `SCR305` does) and the literal view paths, in document order.
   Components are identified by their place in the view (their JSON
   pointer), not by their names, so a rename that updates every reference
   to the renamed component leaves the map as it was; any other change
   (a reference or lookup that found nothing now finds a component, one
   that found a component now finds another or nothing, a reference added
   or removed) drops the fix, which is then reported unsafe with the
   reference and both resolutions. This covers every safe fix that edits
   a view (`NAME001`, `VIEW013`, `VIEW015`, and the script fixes in view
   scripts); Python fixes keep the name-resolution check above.
   `a_rename_that_makes_references_resolve_changes_the_map` and
   `safe_fixes_keep_every_resolution` (generated views with random names
   and references) test it.

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
* **Member classes.** `JAVA_MEMBER_CLASSES` lists, for every class of
  `JAVA_PACKAGE_CLASSES`, the public member classes declared by the
  class, a superclass or an interface (the attributes Jython resolves to
  a class object, `Map.Entry`), so that reading one as a value counts as
  holding a class (see "Shared objects").
* **What an import finds.** `JAVA_PACKAGE_CLASSES` lists the public
  top-level classes of every vetted Java package and of the packages of
  the vetted classes, and `MODULE_ATTRS` every attribute (`dir()`) of
  every vetted Python module, `os` and `sys` that imports in the image. A
  Java class is vetted only when it is listed there (an import of a
  missing class fails), and `SCR102` removes an import safely only with
  this positive evidence (see "Import side effects").

### Shared objects

Review round 22 (Codex, blocker): `import math`, `del math.pi`, then an
unused `from math import pi` in a function whose `ImportError` the caller
catches. Removing the import (SCR102's safe fix) changed what the script
printed: Jython printed `missing` before and `7` after (verified on the
8.3.3 image).

Every measured fact above describes an object as the image loads it: the
attributes of a module (`MODULE_ATTRS`, `MODULE_VETTED_ATTRS`), the
members and member classes of a Java class (`JAVA_PACKAGE_CLASSES`,
`JAVA_MEMBER_CLASSES`, the reflection, file and store closures), the
functions of `system.*` (the stubs) and the builtins. These objects are
singletons that every script of the gateway shares: a module lives once
in `sys.modules`, a Java class has one proxy per JVM, `system` and
`__builtin__` are the same object for every script. Measured on 8.3.3:
`del math.pi`, `del m.e` after `import math as m`, `del os.path.join`
and `setattr(math, 'tau', 6)` change the module for every importer;
`ArrayList.foo = 1`, `ArrayList.size = lambda self: 42` and
`type(ArrayList()).bar = 2` change the Java class proxy (a `final`
static field refuses a store with `IllegalAccessException`); a Python
class of a vetted module takes new attributes (`Template.delimiter =
'%'`); `del __builtin__.len` makes `len` a `NameError` everywhere;
`sys.modules['math'] = None` makes `import math` fail. Built-in types
refuse (`str.foo = 1` is a `TypeError`), and a Java package refused `del
ju.ArrayList`.

**The measured facts hold only while no code changes the shared objects
they describe; any code that may change one is unknown code**, with its
site, under the closed-world rule (every safe fix that needs it is
unsafe, and like other unknown code it counts as a possible file read).
`vetted::unknown_code` reports, in every scope and through every name
the semantic model resolves (aliases, `global` names, class bodies,
nested functions, flow-insensitively):

* a store, deletion or augmented assignment of an attribute (any target
  form: assignment, `del`, `+=`, `for`, `with`, `except`) whose chain
  root may be a vetted module (`math`, `os`, and `os.path` reached as
  `os.path` or `from os import path`), `sys`, `system` or `fpmi`, a Java
  class, a function, class or constant of a vetted module, a builtin, or
  anything not vetted. The rest of the chain does not matter:
  `string.Template.delimiter = '%'` changes a class the module hands out;
* `__setattr__`, `__delattr__` and `__dict__` named in such a chain
  (`math.__setattr__('pi', 1)`, `object.__setattr__(x, 'a', v)`);
* `setattr` and `delattr` (literal names; any other use already
  escapes) whose first argument may be such an object, or is not a name
  or a chain rooted at one (`setattr(type(x), ...)`);
* a Java class used as a value (`c = System`, `f(String)`, `return
  System`) and a member class read as a value (`e = Map.Entry`; member
  classes are measured, `JAVA_MEMBER_CLASSES`): whoever receives it may
  change it. Uses that only read a class stay vetted: a callee, the
  object of an attribute read, an `except` clause, a comparison operand,
  the class arguments of `isinstance` and `issubclass`, the argument of
  `str`, `repr` and `unicode`, a `print` value and the right operand of
  `%` (alone or in a tuple or list);
* `type(x)` with one argument other than in those read-only places: it
  gives the class of a Java value without naming it;
* module objects used as values, `reload`, `sys.modules`, `__builtin__`
  (not vetted, so its import is unknown code: `__builtin__.len = f`,
  `del __builtin__.open`, `__builtin__.__import__ = f`), `__builtins__`,
  `vars`, `globals` and `__import__` were already unknown code.

Project library modules are shared too, but their names have their own
analysis: a store into a library module (`core.util.fmt = f`, `del
core.util.fmt`, `setattr(core.util, 'fmt', f)`, `core.util.LIMIT += 1`;
deletions count the same as stores) makes the name rebound
(`library_stores`, `ProjectIndex::library_name_stored`). What a library
module imports from elsewhere is not its own: the script layer records
every object changed through a chain that may be a library name, and
every such chain used as a value, with the site
(`References::library_changes`, `References::library_values`), and the
index (`ProjectIndex::shared_object_site`) counts as unknown code a
change of anything reached through a name a library module imports
that is not a library module, and a value that is such a shared object
(`vetted::holds_shared_object`: a module, a Java class or member class,
`system`, anything not vetted) or a library module or package one of
whose modules imports one: `del lib.math.pi`, `q.math.pi = 1` after
`import lib as q`, `from lib import math as mm` then `del mm.pi`, `k =
lib.math`, `f(lib)`, `setattr(lib.math, 'pi', 1)` and `lib.ArrayList.size
= 1` when `lib` has `import math` or `from java.util import ArrayList`.
A name of a module whose names cannot be known counts too. Reading a
vetted value through one stays vetted (`return lib.os.sep`).

Not covered, by argument rather than by a list: Java instances (a
`System.out` stream, an enum constant) and instances of vetted Python
classes are values, not described by any measured fact except their
declared types, which setting a public field keeps; a Python class or
function of a vetted module kept as a value (`t = Template`) is not
followed, because no measured fact describes its attributes (only that
the module has it), so changing it through an alias invalidates nothing
a fix relies on (direct changes still count, above). `os.environ` is a
mutable mapping that no fact describes.

Measured on the private corpus (its revision of 2026-10-24): findings
and fix applicabilities are unchanged (2,928 findings; the corpus already
held unknown code, so its SCR102, VIEW015 and NAME001 fixes were unsafe
before); 9 unsafe reasons now name a site in the script itself instead of
the closed world. The new forms occur 2 times as a Java class used as a
value, 5 times as `type(x)` kept, and in 22 resources as a library module
or name holding a shared object used as a value; no store, deletion,
`setattr` or `__setattr__` into a shared object occurs.

Tests: `changing_a_shared_object_is_unknown` (every form above, and the
read-only forms that stay vetted), the binary test
`changing_a_shared_object_keeps_the_import` (the repro: JSON
applicability and reason with its site, `--fix` leaves the file
unchanged, a warm cache, a run over the file alone, the change in
another module; aliases, `os.path`, a Java static field, `__builtin__`
and a library module's import) and the language server test
`quick_fix_after_a_shared_module_change_is_not_preferred`.

#### The binding rule

Review round 23 (Opus, blocker): `class K: import math` stores the module
as the attribute `K.math`. `K` is a plain value (a class the code
defines), and the walker does not check attribute reads of plain values,
so `del K.math.pi`, `K.sys.modules['math'] = None`, `K.os.system(...)`,
`K.System.loadLibrary(...)` and `K.os.open` / `K.os.read` of a file a fix
edits stayed vetted, through `K()`, `self`, a classmethod's `cls`, a
parameter, an alias, a subclass, a nested class and `l.K` of another
module alike. On the 8.3.3 image the first printed `missing` before
`SCR102`'s safe fix and `7` after.

The cause was not a missing form but a missing rule. **An object the
vetting tracks (a vetted module, `os`, `sys`, `system`, a Java class or
member class, a function, class or constant of a vetted module, a
builtin, a project library module, anything not vetted) may be held only
by a name whose every use the walker analyses. Every other holder
receives it as a value**, and a value is checked where it is computed:
a module object or a binding that is not vetted used as a value anywhere
(only a callee or the object of an attribute read is not a value) is
unknown code; a Java class or the builtin `type` is unknown code unless
it is only read, in a read-only place (a callee, the object of an
attribute read, an operand of `is` or `is not`, an `except` clause, the
class arguments of `isinstance`, the second argument of `issubclass`, the
arguments of `super`, the argument of `str`/`repr`/`unicode`, `print`, the
right operand of `%` on a string literal; see "Classes in operators"
below); and a file facility used as a value is a possible file read.
(`vetted::Walk::value_use`: the code fails closed for module objects,
which no read-only place admits.) That covers every binding form that evaluates an expression:
assignments in any scope (class bodies included), default arguments,
`for`, `with` and `except` targets, comprehension variables that leak
from a Python 2 class body, attribute stores (`self.x = sys`, `obj.x =
math`), subscript stores, containers, `return` and `yield`. Closures
need nothing more: a free name's uses are in the code and are analysed
there, and a function's cells and globals are introspection routes.

An import binds the object without an expression the walker sees. Its
name is analysed when it lives in a function or a script's module scope;
a library module's top-level names are followed by the project index
(`ProjectIndex::shared_object_site`, round 22). A class body's names are
attributes of the class, its instances, its subclasses and every alias,
none of which the walker follows (`vetted::attribute_namespace`). So
**every import in a class body is unknown code at the import**, whatever
it imports (`import`, `from ... import`, a library module, a star
import). This is a precision trade-off, not a claim that `class K: from
math import floor` is harmful; the private corpus has no import in a
class body.

The project index follows the same rule across modules: a function,
class or variable of a library module stands for a shared object
(`reaches_shared`) when the module's code may store one where it is
reachable (`module_may_hold_shared`: the module has unknown code, its
names are not known, or one of its library values reaches a shared
object). `del l.K.math.pi` in another module is then reported at the
change, not only at `l`'s import.

Attribute reads of plain values were audited for other ways to reach a
tracked object. Instances and functions get attributes only by stores,
whose values are checked; base classes that are vetted Python classes
hand out only attributes the subclass reads (`__bases__`, `__mro__` and
`__base__` are introspection routes); library modules are the index's.
What remained was the class of a value: `x.__class__` of a Java value is
its Java class (`ArrayList().__class__.foo = 1` and
`getattr(l, '__class__').baz = 3` change `ArrayList` for every script,
measured on 8.3.3), and `t = type` made `t(x)` escape the `type(x)`
check. `x.__class__`, `x.__objclass__`, `getattr(x, '__class__')` and
the builtin `type` used as a value are now handled like `type(x)`:
unknown code unless only read, and a store through them at any depth
(`type(o).a.b = 1`) is not a read. A `super(...)` proxy refuses stores
and deletions (`AttributeError`, measured), so its arguments are only
read.

Measured on the private corpus (its revision of 2026-10-24): findings,
fix applicabilities and unsafe reasons are byte-identical before and
after (2,928 findings with the default selection, 4,007 with every rule).
It has no import or star import in a class body and no attribute or
subscript store of an imported object; it reads `__class__` 18 times and
names `type` outside a call twice, and none of these is the first
unknown site of any script (so no script became unknown code).

Tests: `class_attributes_hold_no_tracked_object` (every unknown-code,
shared-change and file-read form through a module name, generated
through `K.m`, `K().m`, `self.m`, `cls.m`, a parameter, an alias, a
subclass, a nested class and a class a function returns, each unknown at
the import, and the same routes to a plain attribute vetted),
`class_attributes_of_values_are_followed`, the binary test
`class_attribute_imports_keep_the_import` (JSON applicability and reason,
`--fix` leaves the file unchanged, a warm cache, the file alone, the
file-read gate, and `del l.K.math.pi`, `k = l.K` and a nested class of
another module) and the language server test
`quick_fix_after_a_class_attribute_module_change_is_not_preferred`.

#### Classes in operators

Review round 24 (Codex and Opus): comparisons and `%` handed a Java class
to special methods that user code may define. `E() == AbstractMap` calls
`E.__eq__` with the class, which may `del o.SimpleEntry` for every later
script; on the 8.3.3 image a later `AbstractMap.SimpleEntry` then raises
`AttributeError`. Measured there as well: `!=` (`__ne__`), `%` with a
value on the left (`__mod__`), `in` (`__contains__`) and `issubclass(C,
U)` (the metaclass's `__subclasscheck__` receives `C`). So these are not
read-only places any more (fail closed): an operand of a comparison is
read only when every operator next to it is `is` or `is not` (identity
runs no special method), `%` only when its left operand is a string
literal (`str.__mod__` formats the class with `str` or `repr`, and a
Java class proxy is no subclass of `str`, so no `__rmod__` of it runs
first), and of `issubclass` only the second argument (its metaclass
receives the first). `isinstance(x, C)` keeps its class arguments: their
own metaclass (a Java class proxy's is Jython's) receives `x`, which is
checked as a value. The same holds for `type(x)` and `x.__class__`:
`type(x) == C` is unknown code, `type(x) is C` is a read.

Measured on the private corpus (its revision of 2026-10-24): findings,
fix applicabilities and unsafe reasons byte-identical before and after
(5,405 findings with every rule). Tests:
`classes_reaching_special_methods_are_unknown` (every form above, with
the site) and the read-only forms in `changing_a_shared_object_is_unknown`
and `class_attributes_of_values_are_followed`.

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

### Name resolution, measured

Review round 13: two safe `SCR102` removals broke modules because the
semantic model resolved class bodies like function bodies. Jython's
compiler decides, for every name occurrence, how the generated code
reaches it, and `oracle/scope/` records that decision from the bytecode
Jython generates (`PyFrame.getname`, `getglobal`, `getlocal`, `getderef`
and their store and delete forms, with the spelling the code uses, and
every attribute name). The model's own view of the same program
(`iglint_python_semantic::scope_dump`, built from its references and
bindings) must match it: the `scope_oracle` test replays a stratified
sample and hand-written cases; `oracle/scope/run.sh random` runs a fresh
batch of generated programs (class bodies at module level, in functions
and in classes, private names in every binding form, `global` in classes
and nested functions, comprehensions, generator expressions and lambdas
in class bodies, `exec`, `import *`, reads before bindings, control flow
that ends blocks, generators). What it established:

* a class body reads by name: a name the class binds anywhere is looked
  up in the class namespace, then in the module's globals and the
  builtins, never in an enclosing function. The model's reference keeps
  the class bindings that reach it and, flow-insensitively, every module
  binding and the builtin (`Reference::fallback`); every consumer that
  asks what a name may stand for (`SCR101`, `SCR102` and its
  last-binding guard, vetting, the safety net) sees both;
* private names (`__x`) are mangled to `_C__x` in class bodies only
  (every binding form, also under `global`, which the symbol table does
  not mangle), and attribute names in every scope inside a class; names
  in methods are not. Bindings and references carry the run-time
  spelling (`name`) and the source spelling (`source_name`);
* free variables skip class bodies, a free name that an enclosing
  function declares `global` is global, and a class nested in two
  functions rejects unqualified `exec` with free names and `import *`;
* code after a statement that does not complete is generated but
  unreachable (its accesses never happen), except after a `yield` in a
  generator, which resumes there; an `except` clause whose `try` body is
  only `global` statements is unreachable; code after `return`, `raise`,
  `continue` or a both-ways-ending `if` is not generated at all.

The interpreter also reads names the code does not name. Measured with
`oracle/run-oracle.sh --reads` (a module namespace that records every
lookup; `implicit_reads_oracle` checks the model never misses one):
`__name__` when a class body starts and when a function, lambda or
generator expression is created, `__metaclass__` after a class body
without bases, `__name__`, `__package__` and `__path__` at every
import, `__debug__` and `AssertionError` at `assert`. The model records
them as `ReferenceKind::Implicit` references: they keep the bindings they
read alive (`import math as __metaclass__` is used), rules about names the
author wrote skip them, and the safety net compares them (except the
import's own reads, which go with the import). Jython ignores a module's
own `__builtins__` binding for builtin lookups (measured), so it is not an
implicit read.

The safety net compares, per name read in each scope, the resolution,
the class fallback, whether the read may be unbound and the kinds of the
bindings it may find (with the kind of scope that holds each), so a fix
that removes the binding a read finds is caught even when the read
resolves the same way afterwards.

Measured on 2026-10-24: 100,000 generated programs and the 1,642 scripts
of the private project (its revision of that day), 0 differences; the
private project's findings and fixes are unchanged by the change.

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

### Frame routes: findings and fixes

Review round 24 (Opus and Codex, recall): any of the 145 measured
reflective Java member names (`.type`, `.base`, `.fields`, `.module`,
`.methods`, `.package`, `.validator`, `.traceback`, `getType()`,
`getColumnType(i)`, `__class__`) anywhere in a module made the whole
module a frame escape: every scope read its locals, so `SCR102`,
`SCR103`, `REF002` and `REF012` reported nothing in it (`def g(o): return
o.type` silenced an unused import, an unused variable and an unused
function in `f`). A route there is the first hop of a chain that could
end in a frame, which matters for a fix that must be safe whatever the
code does, but it does not make a finding likely false.

The routes of `iglint_python_semantic::dynamic` are therefore split
(`dynamic::Route`). Every route of either kind (`dynamic::frame_escape`,
`SemanticModel::frame_escape`) makes the safe fixes that depend on what
code reads unsafe, exactly as before: `SCR102`'s fix names the route as
its site, the module's code is unknown for the closed world
(`script_facts::refs`, `ModuleSymbols::unknown_code`), the access
analysis of `VIEW015` and `NAME001` reads and writes everything, library
symbols are variables and the namespace is dynamic for every fix-side
consumer (`ModuleSymbols` itself), and the safety net drops a safe fix
that adds or removes a route. Only routes that read or write a frame's or
a module's names by themselves (`dynamic::frame_observer`,
`Route::Observes`) make every scope read its locals and write its
globals, which silences findings; the symbols for findings
(`ModuleSymbols::for_findings`, used by `REF002`, `REF012` and `REF003`)
leave out the other routes. `SCR103`'s fix is unsafe in any case.

| Route | Example | Findings silenced | Fixes |
|---|---|---|---|
| a frame builtin used as a value, or with unpacked arguments | `e = eval`, `f(vars)`, `eval(*a)` | yes: it reads the frame of whoever calls it | unsafe |
| the builtins module | `import __builtin__`, `__builtins__` | yes: it hands out `eval` and `locals` | unsafe |
| `__import__` or `importlib.import_module` of a frame-reaching module, of a computed or relative name, or used as a value; `imp`'s loaders | `__import__('__builtin__')`, `__import__(n)`, `importlib.import_module('__builtin__')`, `from importlib import import_module`, `imp.load_source(n, p)` | yes (`importlib.import_module('__builtin__').locals()` reads the caller's frame, measured on 8.3.3) | unsafe |
| frames, the module table and the builtins | `sys._getframe`, `sys._current_frames`, `sys.modules`, `sys.builtins`, `sys.getBuiltins()`, `sys.getDefaultBuiltins()`, `sys.modules_reloading`, `inspect.currentframe`, `inspect.stack`, `inspect.getouterframes`, `gc.get_referrers`, `from sys import *` | yes (`sys.builtins['locals']()` reads the caller's frame, measured on 8.3.3) | unsafe |
| trace hooks | `sys.settrace`, `sys.setprofile` | yes: hooks receive frames | unsafe |
| frame, traceback and generator links, function globals | `.f_locals`, `.f_globals`, `.f_back`, `.f_builtins`, `.f_trace`, `.tb_frame`, `.gi_frame`, `.func_globals`, `.__globals__`, also as a literal in a sink | yes | unsafe |
| a frame-reaching module as a value, its namespace, or another module's `sys` | `x = sys`, `getattr(sys, n)`, `f(importlib)`, `sys.__dict__`, `re.sys`, `importlib.sys` | yes: its frame functions are no longer named | unsafe |
| the interpreter's classes | `from org.python.core import Py`, `PyFrame`, `ThreadState`, `PySystemState`, `import org.python.core` | yes: `Py.getFrame()` | unsafe |
| a reflective accessor or format method under another name | `get = getattr`, `map(getattr, ...)`, `f = template.format` | yes: every call of it is hidden | unsafe |
| measured reflective Java members | `.type`, `.fields`, `.module`, `getColumnType(i)`, `.__class__`, `.traceback` | no: a first hop into Java reflection | unsafe |
| Java reflection | `.getClass()`, `Class.forName`, `.loadClass`, `from java.lang import Class`, `java.lang.reflect`, `javax.script` | no | unsafe |
| a computed attribute name or format string | `getattr(o, n)`, `setattr(o, n, v)`, `'{0}'.format` on a computed string, `vars(o)`, `getattr(o, 'class')` | no: it reads what it is given | unsafe |
| the exception state and tracebacks as text | `sys.exc_info()`, `sys.exc_traceback`, `import traceback`, `.tb_lineno`, `.tb_next` | no: a frame needs `tb_frame` (a route of its own) | unsafe |
| line numbers, code objects, class hierarchies, method internals | `.f_lineno`, `.co_varnames`, `.func_code`, `.__dict__`, `.__mro__`, `.__subclasses__`, `.im_self`, `inspect.getmembers` | no: none of them reads a frame's names | unsafe |
| importing a module loader, loading a plain module by a literal name, `compile`, Jython's value classes | `import importlib`, `import imp`, `imp.new_module(n)`, `compile(...)`, `__import__('os')`, `importlib.import_module('json')`, `from org.python.core import PyString` | no: what they hand out is used through further routes | unsafe |

Not routes, and handled by the scope that runs them (`symbols.rs`):
`locals()`, `vars()`, `dir()`, `eval(s)`, `execfile(f)` and `exec` called
directly make their own scope read its locals; `exec` without `in` and
`from x import *` in a scope make it dynamic; writes through `globals()`
make the module dynamic. A string literal names a member only in a sink
("String sinks"); a literal in a sink takes the route of the strongest
name it holds.

What a finding may then miss is code that reads a frame through a chain
that starts with a first hop and names nothing that observes on the way
(Java reflection from `.getClass()` to `Py.getFrame()` and `getLocals()`,
or `getattr` with computed names `'gi_frame'` and `'f_globals'`). Such a
chain makes the finding false; it is accepted for findings, never for
fixes. `SCR104` (`redefined-unused`) and `SCR118`
(`used-before-assignment`) read the same scope flags and follow the same
split; `SCR307`'s rebinding check still counts every route.

Measured on the private corpus (its revision of 2026-10-24, every rule,
`--isolated`): findings 5,380 to 5,405; `SCR102` 5 to 10, `SCR103` 39 to
55, `REF002` 75 to 76, `REF012` 21 to 24, every other rule unchanged; no
fix applicability or unsafe reason of an existing finding changed. Every
one of the 25 new findings was checked by hand against the code: all are
true (unused imports and variables, never-referenced functions, a call of
a function the module does not define), and every new finding with a fix
has an unsafe fix. The routes that had silenced them were reflective
member names (`.type`, `class` in a literal sink), `import traceback`,
`sys.exc_info()`, computed `getattr`/`setattr`/`hasattr`, and Java
reflection to load a driver.

Review round 25 (Opus, M2): `importlib.import_module('__builtin__')`
was a first hop while `__import__('__builtin__')` observed, so a function
that read its `os` through `.locals()` had `os` reported unused. A direct
call of `importlib.import_module` is now judged like `__import__`, and
every other use of it, `imp`'s loaders (`load_module`, `load_source`,
`load_compiled`, `load_dynamic`, `load_package`: they run and hand out
any file's code) and the other ways to the same objects measured on
8.3.3's Jython (`sys.builtins` and its getters, `sys.modules_reloading`,
the namespace of `sys`, `importlib` or `imp`, and the `sys` attribute
that `re`, `os` and `importlib` hand out) observe. `pkgutil` and `runpy`
loaders cannot load `__builtin__` (measured: `ImportError`), and
`imp.find_module('__builtin__')` fails, so they stay as they were (code
that is not vetted, which makes fixes unsafe).

Tests: `dynamic::tests::routes_are_classified` (the table, route by
route), `unused_import::tests::module_loaders_that_hand_out_the_builtins_observe`, `unused_import::tests::frame_access_keeps_imports` and
`literals_name_members_only_in_sinks` (findings kept, fixes unsafe with
the route as the site), `unused_library_function::tests::escaped_modules_are_skipped`
and the binary test `fix_only_routes_keep_findings` (the reviewer's
repro: `SCR102`, `SCR103` and `REF012` reported, `REF002` against a
module with only such a route).

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

**The workspace is found from the root, not from the linted paths.**
Linting one file (the pre-commit hook passes the changed files, an editor
opens one folder, `cd projects/A && iglint check --fix .`) analyses the
same workspace as linting the whole repository: every project and gateway
collection below the workspace root of each linted path
(`iglint_linter::discovery::workspace_roots`), and reports and fixes only
the linted files. A gateway tag script that uses `m.os` keeps `import os`
in `m` used, and a tag script that is not vetted makes the fix unsafe,
whichever file is linted. The root of a path is the **outermost** of
these candidates, each counted only when it contains the path and is not
inside (or equal to) a project or collection:

* the configuration file's directory;
* every Ignition layout root around the path: the directory that holds
  the layout around the enclosing project or collection
  (`<root>/projects/<project>`, `<root>/config/resources/<collection>`,
  widened to its parent when that holds `projects/` too,
  `<root>/services/<gateway>/config/resources/<collection>`), and every
  ancestor of the path that itself holds a project in `projects/` or a
  collection in `config/resources/` or
  `services/<gateway>/config/resources/`. The search through the
  ancestors stops at the top of the outermost Git work tree that
  contains the path; outside a Git work tree, at the outermost of the
  configuration file's
  directory and the working directory that contain the path (with
  neither, only the layout around the enclosing container counts);
* the top of every Git work tree that contains the path, when any
  project or collection lies anywhere below it. The search walks the
  work tree like the workspace walk (below) and stops at the first
  container (or symbolic link to one); it runs once per work tree and
  run. A search that reaches the walk's directory limit (200,000
  directories) counts as finding one: the workspace walk then stops at
  the same limit and the closed world is unknown. Work trees nest: a
  submodule (its `.git` is a file that points into the enclosing
  repository's `.git/modules`) or a repository made with `git init`
  inside another work tree is part of the enclosing work tree, whose
  other directories may hold the rest of the gateway (review round 16:
  with only the nearest `.git`, a run over `ign/projects/A/.../m/code.py`
  of a nested repository `ign` offered `SCR102`'s safe fix while the run
  over the outer repository, whose gateway tag script uses `m.os.sep`,
  reported nothing). Every enclosing work tree is a candidate, so the
  outermost one that holds a container is the root, and the search for
  layout roots goes up to it. A consequence: a home directory that is a
  Git work tree (dotfiles) makes every repository below it one
  workspace when it holds other projects; the closed world is then
  larger, or unknown at the walk's limit, which only withholds fixes.

  A **linked work tree** is not nested in anything (review round 17,
  Opus). `git worktree add` makes a directory whose `.git` is a file
  that points at `<git dir>/worktrees/<name>`, a directory that holds a
  `commondir` file (`iglint_project::workspace::git_top`); it is a
  checkout of its own, of another branch, wherever it lies. Before, a
  work tree in `.worktrees/feat` or `.claude/worktrees/x` of the main
  checkout had the main checkout as its root, whose walk skips hidden
  directories: the file-read gate and the closed world were evaluated
  over the main checkout's code (a feature branch's `open(p).read()` did
  not make the work tree's SCR123 and SCR124 fixes unsafe, and `--fix`
  applied them), REF009 said the walk did not reach the work tree's
  manifests, its REF002 and REF012 findings were missed and baselines
  were keyed by `.worktrees/feat/...` paths. So the search for work
  trees stops at a linked work tree (it is the outermost work tree of
  its paths), and the workspace walk and the walk of linted files do not
  enter a linked work tree below their root, hidden or not: the main
  checkout's runs do not include its files. Other `.git` files keep the
  behaviour above: a submodule's points into
  `<parent>/.git/modules/...`, and a repository made with
  `--separate-git-dir` points at a whole Git directory elsewhere; both
  have no `commondir` and are repositories inside the enclosing work
  tree. A work tree of a bare repository is a linked work tree like any
  other; the bare repository itself has no work tree. A `.git` file
  that cannot be read or does not point at a Git directory counts as a
  repository. iglint reads the `.git` entries on disk only, not
  `GIT_DIR` or `GIT_WORK_TREE`: a Git hook sets them and a shell does
  not, and the root must not depend on how iglint is started. A work
  tree with no `.git` entry at all (a bare repository checked out with
  `--work-tree`, dotfiles) is not a Git work tree for the search, so the
  configuration file and the working directory decide.

**A root candidate must reach the linted path.** Each candidate above
counts only when the workspace walk from it reaches the path's project
or collection (`iglint_project::workspace::walk_reaches`): no directory
on the way is skipped by name (hidden, `node_modules`, `target`,
`__pycache__`) or is a linked work tree before the walk finds a
container. Otherwise the nearest candidate that does reach it is the
root (a project in `node_modules/pkg/projects/P` gets
`node_modules/pkg`, not the repository), and when no candidate reaches
it, the path has no root (closed world unknown, below). A symbolic link
to a directory on the way counts as reached: the walk records the link,
and the files behind it get the treatment of files behind a link
(below). Two roots, one inside the other, are both kept when the outer
one's walk does not reach the inner one. And fail closed: a linted file
whose project or collection on disk the walk from the chosen root did
not reach anyway (`Workspace::unreached_container`, for example a
language server folder whose walk skips the hidden directory of a file
opened from it) gets no fix (`rules::vetting::gate_shared_files`, like a
file behind a directory link), and `REF009` says "`<file>` is inside
`<container>`, but the workspace walk did not reach it (...); iglint
lints it on its own, without the rest of its gateway, and offers no
fixes for it", never that the gateway does not load it. The language
server gives such a file a folder of its own, whose root reaches it.

Without any candidate and without a configuration file, the working
directory is the root when it contains the path and holds a project, a
collection, `projects/`, `services/` or `config/resources/` directly. So
in a Git repository with `projects/` and
`gateway/data/config/resources/`, linting a tag file of the gateway, a
library module, the project directory or the repository gives the same
root, the repository, as does `src/projects/A` next to
`gateway/config/resources/core`. The language server finds the root of
each workspace folder the same way. `--show-settings` prints the roots
(`Workspace root:`). A consequence: a repository that is not an Ignition
workspace but holds projects as test fixtures (iglint's own) is one
workspace, so its fixtures share one closed world; the ign-lint parity
test copies each fixture out of the work tree to lint it on its own.

**The workspace walk fails closed.** Exclusions do not shrink the closed
world: `exclude`, the ign-lint whitelist and `.gitignore` decide which
files are linted, but an excluded or ignored resource still runs on the
gateway, so it still counts for the closed world, for the uses of
library names and for every other cross-resource fact. Below a root,
every directory that holds `project.json` or `config-mode.json` is a
container, whatever its name (projects named `target` and
`node_modules` load on an 8.3.3 gateway), so it is linted and analysed. Only directories that are not
containers are skipped by name: hidden ones (`.git`), `node_modules`,
`target` and `__pycache__` (a container below them is not where the
gateway looks for one). Inside containers nothing is skipped by name: an
8.3.3 gateway loads hidden projects (`projects/.p`), hidden resources and
folders (`script-python/.hidden/`, `script-python/pkg/.inner/`,
`views/.folder/V`) and resources named `target`, `node_modules` or
`__pycache__` (verified with the gateway's project manager and imports),
so the workspace walk and the walk of linted files enter them. Hidden
files are skipped: the gateway reads only the files a manifest names.
**Symbolic links, as the gateway reads them.** Measured on a throwaway
8.3.3 gateway (review round 16), with the project scripts imported from
a WebDev endpoint of each project and the collection's tags read after
a restart:

* symbolic links to **directories** are not followed. A project whose
  `ignition/script-python/lib`, `ignition/script-python` or `ignition`
  directory is a link (to another project's folder or to a directory
  outside `data/`) gets `ImportError: No module named lib`; a project
  directory that is a link (`projects/S -> ../Sreal`) never starts; a
  tag folder of a collection that is a link
  (`core/ignition/tag-definition/default -> ...`) has no tags, and the
  gateway then rewrote the `unary-resource.json` it found through the
  link with an empty `files` list; a `core` collection that is a link
  keeps the gateway from starting (`Unable to create 'core' resource
  collection ... exists but is not empty`);
* symbolic links to **files** are read: a library `code.py` that is a
  link to another project's file (or to a file outside `data/`) runs
  that file, and linked `project.json`, `resource.json`,
  `config-mode.json` and `tags.json` load as if they were copies. Hard
  links are the same file under two names.

iglint matches this. The workspace walk does not follow symbolic links
to directories. A link anywhere inside a project or collection, a
project or collection directory that is a link, and a link outside them
that leads to a directory holding a project or collection (a probe
walks the target, with the same directory limit and no links followed)
is recorded in `Workspace::directory_links` and as a directory that was
not read, with the cause "symbolic link to a directory: Ignition 8.3.3
does not follow symbolic links to directories, so the gateway does not
load what it holds at this path". What a link holds is not part of the
workspace (the gateway does not run it at that path), but a deployment
that resolves links (a copy, an archive, a container image built from
the repository) would load it, so it counts as unknown code for the
closed world and for the file-read gate, like any directory that was not
read (below). Other links outside every container (a linked
documentation folder) are ignored. The walk of linted files still
follows links, so the files behind a link are linted on their own
(standalone, like files outside every project), `REF009` reports each
one as "behind `<link>`, a symbolic link to a directory", and no fix is
offered for them (`rules::vetting::gate_shared_files` drops it): the edit
would land in the directory the link points to, which may be outside
the repository and loaded by another gateway. A literal path that names
the link target is no longer a way around the file-read gate either,
since the link itself makes every safe fix unsafe. Since nothing is
followed, there are no link cycles; a link back to an ancestor that
holds projects is a directory link like any other.

Symbolic links to files are files of the workspace (`FileEntry::symlink`).
The gateway runs the file a link points to, for every resource that
reaches it, so a fix of one path changes all of them: review round 16
found `SCR102`'s safe fix removing `import os` from project `P`'s
`lib/a`, which project `Q` reaches through a link and uses as
`lib.a.os.sep`. Every file is therefore also known by its identity
(device and inode on Unix, the canonical path elsewhere;
`Workspace::shared_file`, computed for the whole workspace on first use),
and **a safe fix that edits a file that is a symbolic link, the target
of a symbolic link of the workspace, or a file with more than one hard
link is unsafe**, with the other paths named ("the fix edits `p`, which
is the same file as `q` (a symbolic link to it): every resource that
reaches the file runs the edited text, and the fix is analysed only for
this path"; hard links outside the workspace are counted). This is the
simplest sound rule: analysing a fix for every resource that reaches the
file would need every rule to merge the facts of several resources.
`assume-scripts-do-not-read-workspace-files` does not lift it. The gate
runs before the file-read gate, on every path that reports or applies
fixes.

A directory that could hold Ignition resources but is not read
(it cannot be listed, a broken link, a symbolic link to a directory, the
directory limit) is recorded in `Workspace::unread`, never taken as
empty: while one exists, the closed world is unknown
(`ProjectIndex::unread_directory`, part of `has_any_unknown_code`), so
the safe fixes of `SCR102`, `VIEW015` and `NAME001` are unsafe with the
reason "closed world unknown: `<path>` could hold Ignition resources but
was not read (<why>), so code there is not known", and code there may
read the files a fix edits, so every other safe fix is unsafe through
the file-read gate with the same path and cause. A resource file named
on the command line inside a project or collection that the walk did not
reach is reported by `REF009` as such (with the directory that was not
read, when there is one), not as a file outside every project.

When no root is found around a linted path (a project directory linted
from elsewhere with nothing around it, a loose file outside every Git work
tree and configuration), the closed world is unknown
(`ProjectIndex::mark_closed_world_unknown`): every safe fix that needs it
(`SCR102`, `VIEW015`, `NAME001`) is unsafe, with the reason "closed world
unknown". The scripts of gateway collections and files outside any project
use the same closed world as project scripts (the runtime is the same).
The binary tests `fixes_see_the_whole_workspace_whatever_is_linted` (for
six fixture layouts and three kinds of content, every file of each
fixture linted on its own from the top, from another directory and from
its own directory gives the findings and fix applicabilities of the run
over the whole fixture), `closed_world_reasons_do_not_depend_on_what_is_linted`,
`fixes_that_need_the_closed_world_are_unsafe_without_a_root`,
`unusual_project_and_collection_directories_are_linted`,
`directories_that_cannot_be_read_make_the_closed_world_unknown`,
`symbolic_links_to_directories_are_not_followed` (a linked project,
collection, library folder, `script-python` and `ignition` directory),
`fixes_of_files_reached_through_links_are_unsafe` (a link inside the
workspace, a link to a file outside it, a hard link),
`manifests_that_are_links_are_read` and
`nested_repositories_belong_to_the_enclosing_work_tree` (the layouts of
the first test include a nested repository and a real `git submodule
add`, and real `git worktree add` work trees in `.worktrees/x`,
`.claude/worktrees/x`, a directory that is not hidden and outside the
repository: every file of a work tree, with a content of its own,
linted from the work tree's top, its own directory and outside gives the
findings of a copy of the work tree made its own repository, and the
main checkout's runs show none of its files),
`files_below_skipped_directories_get_a_root_that_reaches_them`, the
`iglint_project` tests of `workspace::tests`, and the language server
tests `a_folder_inside_the_workspace_sees_the_whole_workspace` and
`quick_fix_of_a_file_reached_through_a_link_is_unsafe` and
`a_linked_work_tree_below_the_folder_is_its_own_workspace`, and
`live::tests::files_of_projects_the_walk_did_not_reach_get_no_fixes`
cover each way of linting part of a workspace.

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
count anyway under the default-deny rule above. Links are kept (review
round 16: a rename over a link replaced it with a regular file): a fix
of a path that is a symbolic link is written through to the file the
link points to (resolved, then written the same way next to that file),
as an editor that saves through the link does, so every path that
reaches the file sees the fix; a file with several hard links is
rewritten in place (truncated and written, not atomically), so every
name keeps the same file. Both are only ever unsafe fixes (above), so
they are written only with `--unsafe-fixes` or a quick fix the user
picks. Fixes of files behind a symbolic link to a directory are not
offered at all.

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
| `system` (Ignition's scripting API, also as its legacy alias `fpmi`) | No `system.*` function of 8.3 evaluates Python code in the caller's frame or resolves a callable by name in it (checked against the generated stubs): functions that take callables call the value, message handlers are looked up among the project's handlers, not in the caller. Except the functions that store or run code from run-time values, which are not vetted (see "`system.*` functions that run code"). Only members the stubs list are vetted (see "`system.*` members the stubs do not list"); a `system` package used as a value (`t = system.tag`) and any store into or deletion from the API (`system.tag.readBlocking = f`, which replaces the function for every script) are not vetted either, nor is a change of any other shared object (see "Shared objects"). |
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
`Value`, `Quality`, `Timestamp`; a property that is empty or only dots,
as in `T.` or `..`, is none for Ignition 8.3.3's `TagPathParser`); a concatenation ending in such a literal
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

`system.perspective.navigate(url=...)` sends the browser to the URL
(`window.location.href`, or `window.open` with `newTab`, in the 8.3.3
client bundle), and a `javascript:` URL runs code in the page (see
"Client-side code"). The call is vetted only when its `url` (the second
positional argument or the keyword) is absent or a literal that runs no
script: relative, or `http`, `https`, `mailto`, `tel` or `ftp` after the
browser's clean-up (spaces and control characters around it, tabs and
line breaks inside it, letter case; `client_code::url_runs_no_script`).
Used as a value it is unknown. The audit of `system.perspective` for
functions that hand the client a URL, HTML or code found only this one
(`download` hands it a file to save; `openPopup`, `openDock` and
`navigate`'s `page` and `view` name project resources).

#### `system.*` members the stubs do not list

Review round 18 (Opus) found the private project calling
`system.perspective.runJavaScriptAsync(...)`, a function of a third-party
module: its shorter overload finds the page through the caller's `self`
(a frame read) and runs browser JavaScript that can read component
properties. The audit above covers the stubs only, so a member they do
not list is not vetted: `vetted::Walk::check_system` records every
`system.*` chain whose first member that is not a package is not a
function, constant or object of the stubs (`system.thirdparty.evalHere`,
a package that does not exist, `system.tag.nope`) in
`UnknownCode::system`, with the chain and its range. Each is unknown code
for the closed world and a possible file read
(`References::unvetted_names`, resolved by the project index), and the
unsafe reason names the site ("`system.thirdparty.readFile (not in the
Ignition 8.3 scripting API)` at `m/code.py:2:12`"). What follows a listed
function, constant or object (`system.net.httpClient().get`) is a value.
`SCR201`'s `allow` does not vet a path: it silences the rule.

**The opt-in.** `[fix] vetted-system-functions = [...]` (default empty)
lists `system.*` paths that the project declares unable to evaluate code,
read a frame or read workspace files; an entry also covers what is read
from it (`system.mes.getThing` covers `system.mes.getThing.x`, not
`system.mes.getThings`), and wildcards are rejected. Checking the
entries is the user's responsibility. `[fix] vetted-expression-functions`
does the same for expression functions (below), case-insensitively. Both
are part of the cache context.

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

Expression functions the 8.3 gateway does not register (a third-party
module's; Vision's `objectScript`, which exists only in Vision clients;
a simulation module's functions that run Python) are code iglint does not
read: every call of one, in every modelled expression that parses, is
unknown code and a possible file read with its site
(`summary::unknown_expression_functions`), unless the project lists it
under `[fix] vetted-expression-functions`. `BIND002`'s `extra-functions`
only silences that rule.

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

### Client-side code

JavaScript in a Perspective page reads the page's DOM and the client's
property store, writes the properties the client may write, opens other
pages of the session's origin (`window.open` hands it the new window)
and calls the gateway with the session's credentials, and the gateway
serves workspace content over HTTP. So a component or action that can run
code the view does not show is code iglint does not read. Scope, decided
fail closed: such code may reach every view of its page and, through
windows of the same origin and the gateway, every page of the session, so
it counts for the whole workspace (unknown code for the closed world, and
a possible file read), not only for its view. `summary::client_code`
records it with its site, from the policy in `iglint_stubs::client_code`,
measured on the client bundles of the 8.3.3 image
(`Perspective-module.modl`, `app-3.3.3.jar`, `mounted/js/*.js`; React
18.2, which renders `javascript:` URLs as written):

* a component type that is not in the measured 8.3 component list (a
  third-party module's, or a missing type): its client code is not known;
* always: `ia.display.iframe` (it reads a `srcDoc` property the schema
  does not list and shows `src` otherwise; a same-origin document runs
  with the page's origin), `ia.display.map` and `ia.display.google-map`
  (their map libraries render popup and tooltip texts as HTML), the
  amCharts components (`ia.chart.xy`, `pie`, `gauge`, `simple-gauge`)
  and the time series bundle (`ia.chart.timeseries`, `powerchart`,
  `chartrangeselector`), whose bundles write `innerHTML`,
  `ia.display.pdf-viewer` (its renderer can evaluate code),
  `ia.reporting.report-viewer` (reports are not read) and the menu
  components (`ia.navigation.horizontalmenu`, `menutree`, `navlinks`),
  which open item targets that are not project paths with `window.open`.
  The entries marked "not reviewed further" in the table were not traced
  to a property: counting them is the fail-closed choice;
* `ia.display.markdown` renders its source with `react-markdown`, passing
  every key of `props.markdown` except `escapeHtml`, and
  `allowDangerousHtml: !escapeHtml` (an absent `escapeHtml` renders raw
  HTML; `allowDangerousHtml` or `transformLinkUri` in `props.markdown`
  turn the guards off; `renderers` such as `{"image": "iframe"}` load
  URLs as documents). It counts unless `props.markdown` is a literal
  object with `escapeHtml: true`, only the schema's keys and empty
  `renderers`;
* `ia.navigation.link` makes `props.url` the anchor's `href` (a path that
  starts with `/` becomes a project path): it counts unless the URL is
  absent or a literal that runs no script (`url_runs_no_script`, as for
  `system.perspective.navigate`);
* `ia.shapes.svg` creates the element type and passes every attribute it
  does not know that `props.elements` names: it counts unless every
  element is a literal of a plain type (shapes, groups, gradients, text,
  clip paths, masks, markers, filter primitives, `use`; not `script`,
  `a`, `foreignObject`, `image` or the animation elements) whose links
  are `#` references and which has no `on...` attribute;
* a navigate action (`nav`) whose `config.url` is not a literal that runs
  no script.

A Markdown component, link or SVG drawing whose literal configuration is
plain counts all the same when something can change it at run time: a
`propConfig` entry of the component on `props` or on the property, or any
script in its view (only the view's own scripts, and code they hand one
of its components, can write its properties; other views reach it only
through a page or session, which is unknown code already). Every other
component of the 8.3 list reads named properties (`getPropsReducer`) and
renders them as text, numbers, styles or project paths; the review of the
components bundle found no other HTML sink (`dangerouslySetInnerHTML`
only in Markdown, `innerHTML` only with constant text) and no other URL
passed to `href`, `window.open` or `location`.

Tests: `client_code::tests`, the rule tests
`unknown_system_and_expression_functions_and_client_code_are_unknown`
(`SCR102`), `unknown_functions_and_browser_code_make_fixes_unsafe`
(`VIEW015`), `unknown_functions_and_browser_code_make_the_rename_unsafe`
(`NAME001`), `unlisted_system_functions_may_read_files` (`SCR124`),
`vetted::tests::unlisted_system_members_are_unknown`, the binary test
`unknown_functions_and_browser_code_keep_fixes_unsafe` and the language
server test `quick_fix_with_unknown_functions_or_browser_code_is_unsafe`.

Measured on the private project (its revision of 2026-10-24): 3 uses of
`system.*` members the stubs do not list (under 2 package prefixes), 1
call of an expression function the gateway does not register, and among
287 views 28 components of types outside the 8.3 list, 16 charts, 1
inline frame, 1 menu, 10 Markdown components, 2 links, 692 SVG drawings
and 22 navigate actions with a URL. Its findings and fix applicabilities
are unchanged (2,928 findings with the default rules; its fixes were
unsafe before, through other unknown code).

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
| `NAME001` | Rename a component and every reference to it in the view | No reference to the component at all (references are resolved in every binding and transform field Perspective parses, see "Perspective fields and component names"); no script reads the component's name (`meta.name`, or the measured name members `name` and `getName()`, on the component itself or on a component the access analysis reaches through `children`, `getChildren()`, loops and indexes; unknown members read everything); no script looks children up with a computed name or has a string literal equal to the name; every script parses; no script of the view hands a component to code outside the view (library call, message payload, `getGlobals()`, an opaque call such as `locals()`), reaches a frame builtin in any way, calls an unnamed value or calls a library name that is not a plain function, class or literal constant: a library function that receives a component, or `eval` under another name, can call `getChild('Name')` itself; every script of the view uses only vetted code (see "Threat model"); no string of the view that is not a modelled expression, property binding path or script holds a `{...}` reference-like token; no reference has whitespace inside its braces (`{ ../Name.props.x }`, a tag path to Perspective); no string of the view (values and keys, expressions included, scripts and component names excluded) holds the name as a word where the fix does not update it; and the new name gets the same guards (review round 21, B1): no string of the view (as above, minus the fixed values of Perspective's schema, which are never read as a name: known component types from the generated stubs, binding and transform types measured in `perspective_fields` and the keys on the way to their measured fields, known action types, action scopes, `meta` keys other than `name`; the old name's check skips them too) and no string literal of a script (lookup arguments included, resolved or not) holds the new name as a word, in any spelling that differs from it only in case (Perspective's `findChildByName` compares with `String.equals`; case variants count to stay on the safe side), other than the old name itself, so no broken reference, binding path or lookup starts to find the renamed component | Unsafe when references exist, a component may escape, a script reads the name, an unread string may hold a reference or the name, the new name already occurs in the view, or a script uses code that is not vetted (`cPickle.loads` of an `eval` opcode); the fix names the first such place. The safety net's resolution map ("Threat model", 3) checks the result independently. No fix when a lookup cannot be rewritten or the new name is taken. Another view's script (or any code handed a page, session or one of its views) can look the component up: any such use in the workspace is unknown code under the closed-world rule. No other resource names a view's components: property references resolve in their own view, page or session |
| `VIEW012` | Set, add or remove `access` in `propConfig` | Never: the access mode decides whether the browser receives the property and whether it may write it | Unsafe |
| `VIEW013` | Set `persistent: false` (and remove the stored value) | No stored value: the property starts out absent either way; and no code in the workspace may read workspace files ("File reads") | Unsafe when a stored value is removed: scripts and expressions that run before the binding delivers (query bindings are asynchronous) see a different value |
| `VIEW015` | Remove an unused custom property and its `propConfig` entries | Nothing reads or writes the property, and the view's own scripts prove it: resolved reads and writes cover expressions, bindings (bidirectional ones write) and scripts, analysed default-deny (every use of a component, the view, a page or session, a property container or an alias of one must be one of the forms listed under "`VIEW015`: understood forms" below; anything else reads and writes everything it reaches); a script that reads frames or reaches a frame builtin in any way reads everything, and every library name a script calls or uses as a value is checked to be a plain function, class or literal constant (or missing); properties with a binding or change script are never reported | Unsafe when a script, a library function it calls or a bidirectional binding writes the property (the write recreates it at run time without the `access` and `persistent` settings of the removed `propConfig` entry); unsafe when the finding relies on following a value into library functions (inter-procedural: the function's body, a function that does not exist, the depth limit); unsafe for a view linted outside its project whose scripts call names that may be library functions; unsafe when a script of the view, or a library module it imports or calls, uses code that is not vetted; unsafe when any code in the workspace reaches other views through a page or session (closed world, "Other views") |
| `SCR102` | Remove unused import names or statements; the safe fix keeps the emptied lines, the unsafe fix removes them | Not in a class body (never reported); not a module-level name the module may read dynamically (never reported); not a top-level name of a library module that another resource reads (`pkg.mod.name`, a literal `getattr` or `hasattr` name) or uses as a value (a default argument, a container, `getGlobals()`, `sys.modules`; never reported); a top-level library import is provably unused by other resources and no resource of the project uses code that is not vetted; every removed import is known to succeed and to be effect-free: a measured Java class or package of the vetted packages, or a plain `import x` / non-package `from x import y` of a standard module on the verified list that was measured to import (and to have `y`), with no project library package of that name; the module uses only vetted code and calls no library name that may be a frame builtin; the import is not the only binding of a name still read in its scope or a nested scope; it does not come before a `from __future__` import; and, when the whole statement goes from a library module (where every import that runs binds `__package__`, see "Names imports bind"), nothing may observe `__package__` or `__warningregistry__`: no read or binding of either in the module, no binding of `__name__`, no read of the module's globals by name, and no other resource reading them, the module's `__dict__` or the module as a value (`external_use`, unknown counts as a reader) | Unsafe (project library modules, third-party and non-JDK Java packages, Java classes or module attributes not measured to exist, `socket`, `logging`, dotted imports, relative imports, unknown project, a library name that is not a plain function, class or literal constant, code that is not vetted, a name that would resolve differently, a misplaced future import that would become valid, a `__package__` that code may observe) |
| `SCR103` | Remove `x = value`, or only `x = ` when the value calls or binds something | Never | Unsafe: the value may be wanted under another name, and a removed attribute or subscript read, operator, comparison, `in` or `%` formatting can raise or run code (getters, special methods). The statement is removed only when the value calls nothing (no call, no backticks, which call `repr`, no `yield`) and binds nothing (no list comprehension, whose variables Python 2 leaks), so no pointless statement (`SCR114`) is left |
| `SCR110` | `is` to `==`, `is not` to `!=` | Never: the result changes whenever identity and equality differ (`int('1000') is 1000` is `False` in Jython), and `==` calls `__eq__` / Java `equals` | Unsafe |
| `SCR115` | `except A, B:` to `except (A, B):` | Never: the handler then catches `B` and no longer rebinds the name | Unsafe. The finding itself requires `B` to resolve (ignoring the handler) to a class statement, a capitalized `from` import or a builtin exception, and the handler body not to read it |
| `SCR123` | Remove unused suppression codes or the whole comment (a comment alone on its line leaves the line empty) | Only codes are removed (the rest of the comment stays), or the comment holds nothing but the suppression. Comments do not run, and no line moves; the fix is layout-only, so every token of the script must stay as it is ("Layout-only fixes" under "Decisions"); and no code in the workspace may read workspace files ("File reads") | Unsafe when the whole comment would go and it also holds a reason, a `# pylint:` directive or a `coding:` declaration; that fix removes the line. Unsafe, with the net's reason, when removing the comment and the whitespace before it changes a token (after a stray backslash, which then joins the next line) |
| `SCR124` | Remove trailing whitespace | The script parses, the whitespace is outside string literals (also outside a string Jython drops, after a backslash: that whitespace is part of the string and not reported) and outside the generated header; the fix is layout-only, so every token of the script must stay as it is ("Layout-only fixes" under "Decisions"); and no code in the workspace may read workspace files ("File reads") | No finding when the script does not parse; unsafe, with the net's reason, when the whitespace follows a stray backslash (Jython drops the backslash with the next character; without the whitespace it joins the next line); unsafe while code may read workspace files |
| `SCR206` | Remove a `shared.` / `project.` prefix | Never: in Ignition 7 the two prefixes named different libraries | Unsafe |
| `SCR301` | Rewrite or insert the event script header | Never: code may rely on the written parameters or defaults | Unsafe |
| `SCR900` | ign-lint's trailing-whitespace fix (every line `rstrip()`ed) | The script parses and no removed whitespace is inside a string literal (multi-line strings, or after a backslash in a string Jython drops); the fix is layout-only, so every token must stay as it is ("Layout-only fixes" under "Decisions"); and no code in the workspace may read workspace files ("File reads") | Unsafe |

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
(`Live::fix_one`). A safe fix that fails the net is not applied. So that
what is reported is what is applied (review round 18, N1), every path
that reports fixes (`check` without `--fix`, the findings left after
`--fix`, the language server's diagnostics and quick fixes) runs the net
first, as `fix_file` does: all safe fixes of the file together, and when
the net drops them, in halves until a group passes or holds one fix
(`safety::mark_rejected_safe_fixes`, see "Size limits" under
"Decisions").
A safe fix the net drops on its own is reported as unsafe with the net's
reason ("the safe fix could change what the code does (...)"), offered as
an unsafe quick fix, and applied only with `--unsafe-fixes`, like every
unsafe fix; one the net drops only together with others stays safe, and
a later fix pass applies it once the others are applied or dropped
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
`iglint_python_semantic::dynamic` (escape routes: every route withholds
the safe fixes, and those that observe names make every scope of a
module read and write names dynamically),
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
| `SCR102` | read the import by name (`lib.util.ev('os')`, `capture()['os']`, `getattr(me, n)` on the module itself) | Scopes with escape routes that observe names, or frame builtins, are dynamic (no finding); any other escape route makes the fix unsafe ("Frame routes: findings and fixes"); a computed `getattr` on a module object keeps module imports; a library name that may be a frame builtin, or code that is not vetted, makes the fix unsafe (`library_frame_builtins_and_computed_getattr`, `frame_builtins_under_other_names_keep_imports`, `review_round_six_repros`, `frame_access_keeps_imports`) |
| `SCR123` | read the script's file (`open(path).read()`); through the running code nothing: comments do not run, and the safe fix keeps every line, so line numbers (`f_lineno`, tracebacks, `warnings`, Java stack traces) do not move | "File reads": dynamic code may call `open` under another name, so it makes the fix unsafe unless the project declares that no script reads workspace files (`safe_removal_keeps_lines`) |
| `SCR124`, `SCR900` | read the script's file; through the running code nothing: trailing whitespace that the lexer skips is not in any value, line numbers do not move, and Jython has no source for scripts compiled from strings (`inspect.getsource` raises `IOError`, tracebacks show no source lines; verified on Ignition 8.3's Jython) | "File reads" (`dynamic_code_makes_the_fix_unsafe_unless_declared`, `file_reads_make_the_fix_unsafe`); whitespace the lexer reads: "Layout-only fixes" |

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
* **Size limits.** `SCR002`'s figures are lower bounds (it reports only
  what certainly fails), so comparing compile status with them alone does
  not see a fix near a limit: review round 18 (Opus, B2) removed an
  import from a function of 2,176 `a = abs(i % 7)` lines, which Jython
  rejects ("Module or method too large") and accepts without the import
  (65,530 bytes), and `--fix` applied it. Every Java method of the
  module's class (each code unit, `<init>`, which builds a code object
  per compiled unit, `call_function`, a branch per compiled unit, and the
  `set$$N` methods that fill large constant displays) and the constant
  pool therefore also have a sound upper bound (`limits::upper`). Where
  the lower bound fits a limit and the upper bound does not
  (`limits::size_unknown`), the compile status is not known, and the
  safety net drops (so `check` reports as unsafe, see the end of "Line
  numbers") every safe fix that changes that script's tokens, with the
  bounds in the reason. The 100,000-character look-ahead of the first two
  lines is exact and needs no upper bound.

  Review round 24 (performance): the net then rejected the safe fixes of
  such a script together and checked each alone, each check parsing and
  bounding the whole script, so the time grew with the square of its size
  (the reviewer's 1,400 lines with trailing whitespace took 3.2 s, 12,000
  lines 95 s, 16,000 more than 120 s). Three changes, none of which lets a
  fix through that changes what Jython compiles:

  1. An edit that leaves every token where it was compiles to the same
     code, whatever the status (`safety::same_compilation`, proven by
     lexing both versions, not assumed): the same tokens with the same
     kind, text, indentation width, line and column (the token that ends
     a line by its line only: its column moves with trailing whitespace,
     its text holds the blank lines after it, and Jython's code records
     lines, not columns), the same position Jython gives each, the same
     lexical errors, the same encoding declaration and the same verdict
     on the first two lines' length. Trailing whitespace outside strings
     and edits inside comments are such edits; the net checks lines
     separately.
  2. A script with a certain error (a syntax or compile error, or a
     lower size bound above a limit) fails to compile: its status is
     known whatever the upper bounds say, so `size_unknown` matters only
     for a script with no certain error (`safety::status_unknown`).
     Lower bounds are sound, so a lower bound above a limit is a failure;
     this also holds when the mirror stops early. The net compares which
     limit a certain size error is for, not its numbers, which move with
     any edit: a fix that keeps the script failing keeps its status, and
     the other checks still apply. The engine's health check compares
     `SCR002`'s size errors the same way since review round 25 (M1):
     before, it took a size error whose byte count moved for a new error
     and refused a fix that `check` reported safe
     (`iglint_python_semantic::size_error_key`, the message without its
     numbers, used by both; the kind of code, module, class body or
     function, and the limit are kept).
  3. The safe fixes that fail together are halved into groups until a
     group passes or holds one fix, in `safety::mark_rejected_safe_fixes`
     and in `engine::fix_file` (which keeps the order and the rule that
     only a fix that fails on its own is blocked): with `k` failing fixes
     among `n`, O(k log n) evaluations, never more than `2n`. Before
     that, a safe fix that may change a token of a script whose status is
     not known is dropped without evaluating the net
     (`safety::UnknownStatus`: an edit that overlaps a token, or whose
     removed or inserted text is not blank and not in a comment); every
     other fix is still evaluated. The language server goes through the
     same two functions.

  Measured in release builds on one machine (`--select SCR102,SCR124`,
  a module of 1,400 statements padded with comment lines, every line
  ending in whitespace), `check` before and after: 1,400 lines 1.7 s to
  0.03 s, 12,000 lines 18 s to 0.12 s, 16,000 lines 26 s to 0.16 s
  (`--fix` 1.0 s); 2,000 functions whose every safe fix fails more than
  150 s to 0.4 s. The release test
  `stress::unknown_compile_status_is_not_quadratic` keeps `check` and
  `--fix` of these shapes, and of a module whose 2,000 safe fixes all
  fail, under 20 s each.

  The first upper bound (round 18) charged the largest cost measured per
  construct with a quarter on top. Review round 19 (Opus, blocker) showed
  that it undercounted closures by up to 4.6 times: Jython builds a
  closure's cells at every creation, about 10.5 bytes per free variable,
  while the bound charged a flat cost per creation and the free variables
  once per unit (30 lambdas over 150 parameters: 48,723 bytes, bound
  11,062; with 25 such lambdas and an import, Jython rejects a function
  that compiles without the import, and `SCR102`'s safe fix removed it).
  The measured programs never had many free variables, so no calibration
  found it. The upper bound is now derived from Jython's code generator
  itself.

  *Source.* Jython 2.7.3's compiler (`org.python.compiler.CodeCompiler`,
  `Module`, `Code`, `ScopeInfo`, `ScopesCompiler`, `ArgListCompiler`,
  from the `v2.7.3` tag of github.com/jython/jython). The Ignition 8.3.3
  image ships `jython-ia-2.7.3.5.jar`; `javap -c -p` (from the
  `eclipse-temurin:17-jdk-alpine` image) of every class of
  `org.python.compiler` in both jars differs only in `checkcast`
  instructions of the compiler's own generic code (`visitDict`,
  `visitImportFrom`, `ArgListCompiler.visitArgs`, `ScopeInfo.cook`), none
  of which changes what it emits; `Code` and `Module` are byte for byte
  the same.

  *Method.* `limits/mirror.rs` walks the tree the way `CodeCompiler`
  does: the same visitor per node kind, the same temporaries in the same
  order (`Code.getLocal` takes the lowest free JVM local, so the size of
  every `aload` and `astore` is known: 1 byte up to local 3, 2 up to
  255, 4 with `wide`), the same record of the operand stack
  (`stackProduce`, which a `yield` saves), the same stack of exception
  handlers, and each nested unit compiled where its creating code is,
  once per copy of that code. Where the generator's choice depends on
  something not modelled (whether a name is local, global, a cell or
  free; the constant pool index of an `ldc`), it charges the larger
  alternative. Instructions: `iconst` 1, 2 (`bipush`) or 3 (`sipush`,
  `ldc_w`) by value; `ldc` 3; field and method references, `new`,
  `anewarray`, `checkcast`, `instanceof` 3; `invokeinterface` 5; a jump 3
  while the method stays below 32,000 bytes, otherwise 5 for a `goto`
  and 8 for a conditional jump whose span may exceed a 16-bit offset (ASM
  widens them to `goto_w`, and a conditional jump to its inverse around a
  `goto_w`). Each term, with the emission site and every multiplicity:

  | Emission site | Bytes charged | Repeated |
  |---|---|---|
  | `setline` | 7 (`aload_1`, `iconst` of the line, `invokevirtual`) | per statement, and per `IfExp`, lambda, generator expression, `yield` and synthetic statement of a comprehension |
  | `Code.ldc` of a string the generator emits (every name, attribute, keyword, imported name and module, class name, string literal, and in `<init>` every variable, cell and free variable name and the code object's name) | 3 up to 16,000 UTF-16 units; beyond, a `StringBuilder`: 13 + `iconst` of the length + 6 per chunk of 16,000. A name counts as long as Jython may spell it: mangled (`_<class>__x`) wherever a class may mangle it, a tuple parameter as `(a, (b, c))` | per string, at each emission below |
  | `visitName` | 4 (`aload_1`, `invokevirtual`) and the `ldc` of the name (or `iconst` of its index, never more); a store adds the `aload` of the value | per name |
  | `visitNum`, `visitStr` (`Module.*Constant.get`) | `iconst`/`ldc`/`ldc2_w` and `invokestatic`, at most 6; a string of more than 16,000 UTF-16 units goes through a `StringBuilder` (`Code.ldc`): 13 + `iconst` + 6 per chunk | per literal |
  | `visitAttribute` | value, `ldc` of the attribute name, `invokevirtual` (store: and `aload`) | per attribute |
  | `visitSubscript`, `Slice` | value, index or the parts of a simple slice (`aconst_null` for a missing one), `invokevirtual` (store: and `aload`); `visitExtSlice` an array of the dimensions, `new PyTuple` and `freeArray`; `visitSlice` `getstatic None` per missing part and a temporary | per subscript, per dimension |
  | `visitCall`, `invokeNoKeywords` | up to 4 positional arguments on the stack: `aload_2`, `invokevirtual` (an attribute call adds `ldc` and `__getattr__`); otherwise `makeArray` of the values and `makeStrings` of the keywords, the star arguments or `aconst_null`, `dup2_x2`, `pop2`, `_callextra` or `__call__`, `freeArrayRef` | per argument, per keyword |
  | `makeArray` | `iconst`, `anewarray`, `astore`; per element the value, `aload`, `swap`, `iconst`, `swap`, `aastore` | per element (call arguments, defaults, bases, displays in a generator, dimensions) |
  | `makeStrings` | `iconst`, `anewarray`, `astore`; per name `aload`, `iconst`, the `ldc` of the name, `aastore` | per keyword, per name of a `from` import |
  | `loadArray` (displays outside generators) | `iconst`, `anewarray`; per element `dup`, `iconst`, the value, `aastore`. 100 or more elements that are all number or string literals (`Module.emitPrimitiveArraySetters`): `dup` and `invokestatic` per 4,096 elements here, and a `set$$N` method of `aload_0`, `iconst`, the constant and `aastore` per element | per element |
  | `visitBoolOp` | `dup`, `invokevirtual __nonzero__`, a jump, `pop` | per operand after the first |
  | `visitCompare` | two temporaries; per comparator but the last `aload`, `swap`, `dup`, `astore`, the comparison, `dup`, `astore`, `__nonzero__`, a jump; the last `aload`, `swap`, the comparison | per comparator |
  | `visitBinOp`, `visitUnaryOp`, `visitRepr` | `invokevirtual` | per operator |
  | `visitIfExp` | line, test, `__nonzero__`, two jumps | per expression |
  | `visitListComp`, `finishComp` | `new PyList` and `append` (13), `set` of `_[n]`, then a `for` loop per generator, an `if` per condition (with its `goto`: the `else` list exists, empty), the call statement and `del _[n]`, each with its line | per generator, per condition |
  | creation of a unit (`visitFunctionDef`, `visitLambda`, `visitClassDef`, `visitGeneratorExp`, `visitSetComp`, `visitDictComp`) | the defaults or bases (`makeArray`), `new PyFunction` or `Py.makeClass`, the code object (`getstatic`), the docstring or `aconst_null`; a generator expression its first iterable and the call | per creation, per copy of the creating code |
  | `makeClosure` | `iconst`, `anewarray`, `astore`; per free variable `aload`, `iconst`, `aload_1`, a `getfield f_back` per class body between the unit and the frame that holds the cell, `iconst`, `invokevirtual getclosure`, `aastore`; `aload` | per creation, per free variable, per `setup_closure` call (each call appends the free variables again, so the n-th compiled copy of a creation in a `finally` body passes n times as many) |
  | free variables (`ScopeInfo.cook`, `setup_closure`) | at most the names of the unit's subtree that a function-like unit around it binds (without those the unit binds itself when it is function-like); a unit between the creator and the cell passes the cell on and pays for it at its own creation (`limits/scopes.rs`) | per unit on the way |
  | `applyDecorators` | `storeTop`; per decorator the value, then `aload_2`, `aload`, `invokevirtual __call__`, `astore`; `aload` | per decorator |
  | `visitAssign`, `set` | one target: `astore`, the store, `aconst_null`, `astore`; several: `astore` and each store | per target |
  | `seqSet` (unpacking) | `aload`, `iconst`, `invokestatic unpackSequence`, `astore`; per element `aload`, `iconst`, `aaload` and a `set` of it | per element, at every nesting level |
  | `visitAugAssign` | the target's parts into 2 temporaries (a simple slice 4, `saveAugTmps`), the load, the value, the operator, the result into a temporary, the parts back (`restoreAugTmps`), the store | per statement |
  | `visitPrint` | per value the destination's `aload`, the value, `invokestatic` | per value |
  | `visitImport`, `visitImportFrom` | per name the `ldc` of the dotted name, `aload_1`, `iconst`, `invokestatic` and a `set` of the alias or first part; a `from` import the `ldc` of the module, `makeStrings` of the names, `importFrom`, and per name `aload`, `iconst`, `aaload` and a `set` of the alias or name | per name |
  | `visitDelete`, `visitExec`, `visitAssert`, `visitRaise` | each part, `invokestatic`/`athrow`; `assert` the `__debug__` test, two jumps and `AssertionError` | per target, per part |
  | `visitIf`, `doTest` | line of the test, test, `__nonzero__`, `ifeq`, `goto` past the `else` after a body that does not exit | per `if` and `elif` |
  | `visitWhile`, `visitFor` | two jumps; `for` two temporaries, `__iter__`, `__iternext__`, the line twice | per loop |
  | `visitTryExcept`, `exceptionTest` | `setException` into a temporary; per handler the type test, the name's `set`, the body and a `goto`; `aload`, `athrow` | per handler |
  | `visitTryFinally`, `inlineFinally`, `doFinallysDownTo` | the `finally` body at the normal exit (when the body does not exit), in the handler, and at every `return`, `break` and `continue` that leaves the `try`, each copy compiled with only the handlers outside it: nested `try`/`finally` bodies with ways out multiply | per way out, per nesting level |
  | `visitWith` | 12 bytes of set-up, the handler (23), the exit call (9) at the normal exit and at every way out of the body | per way out |
  | `visitReturn`, `visitBreak`, `visitContinue` | the `finally` bodies and `with` exits of every handler down to the loop (`return`: all), then `goto`, or the value through the return local, `f_lasti = -1`, `areturn` | per statement |
  | `visitYield` | the line; `saveStack` (per recorded stack item `aload`, `swap`, `iconst`, `swap`, `aastore`; per thread state `pop`) and `restoreStack`; `f_lasti`; `saveLocals` (per active temporary `aload`, `iconst`, `aload`, `aastore`) and `restoreLocals` (`aload`, `iconst`, `aaload`, `checkcast`, `astore`); the restart's 18 bytes. The active temporaries are every JVM local live at the `yield`: two per enclosing `for`, the manager of each enclosing `with`, the exception local of each enclosing `try`/`finally`, the temporaries of an enclosing comparison or call | per `yield`, per live temporary and stack item |
  | `parse` | a class body's `__module__` (14) and docstring; per parameter that is a cell `aload_1`, two `iconst`, `invokevirtual to_cell`; the `with` exits array; `f_lasti = -1`, `None`, `areturn` (a class body `getf_locals`); a generator's `tableswitch` (16 and 4 per `yield`) and the `goto` to it | per unit |
  | `ArgListCompiler.appendInitCode` | the load of the tuple's name (`(a, b)`) and the unpacking of tuple parameters, prepended to the function's statement list at every compile: the n-th copy of a `def` in a `finally` body has it n times | per copy |
  | `<init>` (`PyCodeConstant.put`) | per compiled unit `iconst` of the argument count, the variable names, cell and free variables (`makeStrings`, the free variables per `setup_closure` call, each name's `ldc` by its length), the code object's name (`ldc`), line, flags, `Py.newCode`, `putstatic`; `aload_0`, `invokespecial` and `return` once | per compiled unit, per name |
  | `call_function` (`Module.addFunctions`) | 25, and `tableswitch` entry, `invokevirtual`, `areturn` per compiled unit (8) | per compiled unit |

  The constant pool's bound counts every distinct string and name (also
  as Jython mangles private names in a class), chunks of long strings and
  names,
  the names Jython makes (`_[n]`, `_(n)`, `_(x)`, `(a, b)`), numbers, 24
  entries per compiled unit, line numbers beyond `sipush` and 600 entries
  for the runtime's classes, methods and fields, with a twentieth on top.
  Sizes are `Saturating<u64>` (`limits::Bytes`) and `limits` denies
  unchecked arithmetic: review round 19 (Codex) found that 64 nested
  `try: ... finally: return 2` wrapped the old bound to a small number in
  release builds (and panicked in debug builds), so the fix of such a
  script was safe. Release builds keep `overflow-checks` off: turning
  them on measured no difference on the private project and 2 to 4% more
  time on the synthetic corpora of 500 and 5,000 views. The mirror stops
  after 4,000,000 node visits (2^50
  copies of a `finally` body); then every upper bound counts as unknown,
  and so does the script's compile status unless a lower bound already
  exceeds a limit (`SCR002` reports it; see round 24 above).

  *Calibration.* `tests/method_size_oracle.rs` compares every Java method
  of every generated program with the oracle (`oracle/run-oracle.sh
  --method-size`, which records every method of the class and its
  constant pool count), in Jython's order and by name, and the constant
  pool; it reports the smallest margin per family of programs. Besides
  the random programs, the repetitions of single statements, programs
  near each limit and the reviews' repros, `adversarial.rs` generates a
  family per term of the table: closures over up to 300 free variables
  created up to 60 times, three levels deep and through class bodies, in
  loops and in `finally` bodies; nested `try`/`finally` and `with` with
  ways out at every level; `yield` under up to ten loops, `try` and
  `with` blocks and inside calls, comparisons and displays; calls of up
  to 300 arguments and 50 keywords; unpacking of up to 200 nested
  targets; up to 30 decorators, 100 defaults and 50 bases; comparison and
  boolean chains of 200; displays below and above 100 and 4,096
  elements, constant and not, in generators; strings of up to 75,000
  characters and long integers; imports of up to 300 names; augmented
  assignments to every kind of target; every other statement; JVM locals
  beyond 255; the random statements over 300 names with nested units;
  methods beyond 32,000 bytes; and combinations. A calibration run with
  ten times the committed programs per family and 3,000 random bodies
  near 64 KB (10,000 programs, 7,747 that Jython's code generator
  compiles, 131,835 code units) found no method and no pool above its
  bound, and no lower bound above Jython's size. The smallest margins
  (upper bound over size, for methods of 200 bytes or more): closures
  1.004, displays 1.000 (4 bytes), random bodies near 64 KB 1.000 (0
  bytes: the mirror is exact for them), names 1.042, yields 1.023,
  `finally` 1.111, `with` 1.056; for methods of 1,000 bytes or more the
  median is 1.07, and the closest bound above a unit of more than 60,000
  bytes is 109 bytes above it. Pool bounds are at least 1.05 times the
  count. The committed data (`tests/oracle/method_size.expected`, a
  stratified sample of 3,710 programs, the code generator's output
  without the redundant `units` line and method numbers) gives the same
  margins.

  The new programs also found that `SCR002`'s lower bound was not sound
  everywhere, now corrected and checked against every unit and every
  `<init>` of the calibration run: a simple slice (`a[b:c]`, passed to
  `__getslice__` as its parts), stores without a temporary (several
  targets, loop and `with` targets) and unpacking, `del` of a tuple, an
  `if`, `with` or `try` whose body ends, `exec` with both parts,
  comprehensions with several generators, `from` imports, and `<init>`
  in small modules (the code object of each unit is now counted per
  instruction at its shortest encoding). `SCR002` reports a body only
  when the corrected lower bound exceeds the limit, with 0 false reports
  on the calibration run.

  The measurement found a limit `SCR002` did not model in round 18:
  Jython fails from about 765 functions of eight parameters on `<init>`.
  `SCR002` reports it from the lower bound of `<init>` (per unit the
  instructions of its code object at their shortest encoding, 29 to 32
  bytes and 5 or 6 per parameter or assigned local of a function): on
  generated modules of 1,000 to 2,400 mixed functions, decorated
  functions with defaults, classes, methods, nested functions, lambdas,
  generator expressions and constants, it reports 79 of the 98 of the
  committed sample that Jython rejects and none that compile; the verdict
  between the bounds is open. The constant pool is reported whenever its
  lower bound (two entries per distinct string literal) exceeds the
  limit, and open between the bounds. On the private project no script
  has an open verdict (1,600 scripts: 458 library modules and the scripts
  of its views; the largest unit's upper bound is 23,246 bytes, where the
  round 18 bound said 50,874), and the findings and their applicability
  are the same as before (all its fixes are unsafe for other reasons).

  *Long names (review round 20, Codex, blocker).* The mirror charged a
  flat `ldc` per name. `Code.ldc` builds a string of more than 16,000
  UTF-16 units with a `StringBuilder`, for names as for string literals,
  and the mirror modelled that only for literals: a function of 1,683
  reads of an attribute of 16,001 characters had an upper bound below
  the limit, Jython rejects it with an `import math` and compiles it with
  the import's line blanked (65,528 bytes), and `SCR102`'s safe fix
  blanked the import. On the oracle 1,000 such reads take 38,889 bytes,
  where the bound said 21,045. Every string the generator loads is now
  charged by its length (the `Code.ldc` row of the table): the bound of
  the repro's function is 72,388 bytes without the import and 72,414 with
  it, so its verdict is open. Two more layers fail closed. A script with
  an identifier (a name, attribute, keyword, imported name, module or
  alias, `global` name, function, class or parameter name, in code Jython
  generates or not) of more than 1,000 characters
  (`limits::IDENTIFIER_LIMIT`) has an unknown compile status whatever the
  bounds say ("identifier of N characters: Jython's code size cannot be
  bounded precisely"): Jython changes how it encodes a name at 16,000
  units and limits method names at 65,535 bytes, so this keeps every fix
  a factor of 16 away from any error in the model of either, and real
  code has no such names (the longest identifier of the private
  project's 1,600 scripts has 79 characters). String literals are not
  limited: the mirror charges them exactly, and real scripts hold long
  ones. And `SCR002` reports a `def` or `class` whose Java method name
  (`<name>$<number>`, mangled in a class) exceeds the 65,535 bytes of a
  class file constant (`UTF8 string too large`), measured exactly for
  functions, classes, methods, nested functions and private methods
  (`tests/limits_oracle.rs`); every other position of a name, and
  docstrings and string literals, compile at 1,000,000 characters
  (`docs/internal/ignition-format.md`, "Size limits").

  The calibration has an adversarial family for names (`long-names`, 30
  programs): every position above (attribute loads, stores, augmented
  assignments, calls and `del`, local, global and deleted names,
  keywords with and without star arguments, every form of import, `global`,
  nested `def` and `class` names, parameters, tuple parameters, star
  parameters, lambda parameters, cells and free variables, docstrings,
  `str` and `unicode` literals, comprehension and generator targets,
  `except` and `with` targets, `getattr` with a literal, private names in
  a class, `yield` of an attribute) at 15,999 to 70,000 characters, and
  dense functions of 1,000 to 2,300 reads of names of 16,001 or 32,001
  characters near the method limit; and the review's repro and probe. No
  method, `<init>` or pool is above its bound (the family fails without
  the change: 38 bounds below Jython's size). Smallest margins: methods
  of 200 bytes or more 1.070 (22 bytes), `<init>` 1.083 (11 bytes),
  constant pools 3.3.
* **Layout-only fixes (review round 25, B1).** `SCR123`, `SCR124` and
  `SCR900`'s trailing-whitespace fix remove whitespace or comment text.
  They were safe on the argument that the lexer skips that text, which
  does not hold everywhere. Jython's lexer silently drops a single-quoted
  string that a line break ends before its closing quote, together with
  the line break, and lexes on; a backslash in it escapes the whitespace
  after it, so `v4 = 'x  \ ` followed by `y' # c` is `v4 = y` (and
  `y = 5`, `v4 = 'ab\ `, `y + 1` gives `v4 == 6`). Without that
  whitespace the backslash continues the string on the next line:
  `v4 = 'x  y'`. A backslash outside a string that is not followed by a
  line break is dropped with the next character, so `print 1, \ ` then
  `2` prints `1` and evaluates `2`, and without the space prints `1 2`;
  removing a comment after `x = 1 \ ` takes the whitespace before it and
  joins the lines. All measured on Ignition 8.3.3's Jython
  (`oracle/run-oracle.sh --run`). iglint lexed these right (no `SCR001`,
  `SCR101` for `y`), but `SCR124` treated the whitespace as trailing
  whitespace, and the net did not see the change: it compared compile
  status, scopes and the references that remain, and the fix only removed
  the read of `y` and added a string. Three changes:

  1. Layout-only fixes say so (`Fix::layout`), and the net requires that
     the scripts of the text with only the other safe fixes applied and
     of the text with all of them compile to the same code, proven by
     lexing (`safety::same_compilation`: the same tokens, kinds, texts,
     indentation, lines and columns, line ends and dedents by line only).
     Otherwise the fix is reported unsafe with the net's reason ("it
     removes whitespace or comment text that Jython's lexer reads") and
     `--fix` does not apply it. This is the fact such a fix claims, so it
     is checked directly, in every storage format, and whatever else the
     net finds.
  2. `SCR124` and `SCR900` treat whitespace after an escaping backslash in
     a string Jython drops as part of the string: `SCR124` does not
     report it, and `SCR900`'s fix is unsafe.
  3. The facts the net compares for every safe fix are stronger: a read
     that goes (or one of several reads of a name) and a change of the
     number literals or of how many string literals a script has are
     changes, so a fix of another kind that turns code into a string or
     back no longer passes on the names that remain. The text of string
     literals is not compared: `NAME001` renames inside them, and the
     view's resolution map checks those.

  A comparison of whole token streams outside the edited ranges was
  considered for every safe fix; it is not needed for the fixes that
  change tokens by design (`SCR102` deletes an import statement, `NAME001`
  edits string literals), whose effects are what the other facts
  describe, and for layout-only fixes the stronger token identity above
  applies. The differential test
  `rules::script::layout_oracle` (module documentation for the commands)
  generates library modules full of dropped strings, stray backslashes,
  form feeds, carriage returns, comments and suppressions after them,
  continuations, brackets and blocks, fixes them with the safe fixes of
  `SCR101`, `SCR123` and `SCR124`, and has Jython run both versions,
  recording the output, the exception with its line and every global:
  8,000 programs (seeds 25 to 28), 7,296 changed by the fixes, no
  difference. With changes 1 to 3 switched off, the same run of seed 25
  found 714 programs whose behaviour the fixes changed. A sample of 303
  pairs is replayed without Docker (`replayed_sample_is_unchanged`).

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
  inheritable project, an incomplete parent chain, a path with no
  workspace root (see "Closed world"), or resources whose scripts iglint
  does not model make cross-resource facts unknown; fixes that depend on
  them are unsafe.
* **Every unsafe fix says why.** Each unsafe fix carries a reason (the
  code that keeps it from being safe, or what the fix itself may change),
  shown in every output format and in the language server's quick fix
  titles. The rule tests check it for every fix their fixtures produce
  (`test_support::assert_unsafe_fixes_have_reasons`).
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
