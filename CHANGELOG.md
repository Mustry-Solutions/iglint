# Changelog

All notable changes to iglint are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and versions follow
[Semantic Versioning](https://semver.org/). Rule codes and names are stable
from 0.1.0 on.

## [Unreleased]

### Changed

- `docs/rules/SCR101.md` says which code is not checked at all: a `.py`
  file outside any project and the tag scripts of gateway resources, where
  any name could be a library package iglint does not know.
- `bench/README.md` and `docs/compat/ign-lint.md` quote the private
  project's parity and aggregate numbers as of its revision of 2026-10-21
  (measured 2026-10-23; the parity table and pylint comparison in
  `bench/README.md` re-measured 2026-10-24 with ign-lint 0.6.4 and
  0.6.3, on a copy of the project), and say that they depend on the
  revision.
- Faster runs: about 3x on a 300-view project and 2x on 5,000 views
  (whitelist lookups, JSON formatting detected once per file, mimalloc,
  no teardown at exit, `iglint ign-lint` builds ign-lint's node model only
  for the options that show it). Peak memory on 5,000 views is down from
  1.9 GB to 1.1 GB: parse trees live for one phase only, and JSON arrays
  and objects are allocated at their exact size.
- The cache keeps a unit's results when another file changes without
  changing the facts the project index is built from (most edits inside
  one view or script), instead of discarding every entry.
- SCR900 runs pylint while the native rules run and, on POSIX, forks one
  worker per CPU from a single Python process; `jobs` is the number of
  workers.
- VIEW015 analyses script accesses default-deny: every use of a
  component, the view, a page or session, a property container or an
  alias of one must be one of the enumerated forms (listed in
  `docs/internal/fix-safety.md`); anything else reads and writes
  everything it reaches. Findings that rely on following values into
  library functions keep being reported, but their fix is unsafe; only
  findings the view proves on its own keep the safe fix.

### Added

- `SCR307 redefined-event-handler` (error): a gateway timer, tag change,
  message, startup, shutdown or update file, or a WebDev handler, whose
  last module-level binding of the handler name Ignition cannot call with
  its canonical arguments (`def handleTimerEvent(event):` after the
  header, a value that is not callable, `del`). Ignition runs the module
  code and calls the last binding by position (verified on an 8.3.3
  gateway), so such a script never runs; `SCR104` only warned about the
  redefinition (review round 14).

- `[fix] assume-scripts-do-not-read-workspace-files` (default `false`)
  in `iglint.toml`: declares that no script reads the files of the
  workspace, which lifts the file-read condition below (and only that).
  The user's responsibility; shown by `--show-settings`.
- `stubs-gen/perspective-fields.sh` measures, inside the Ignition image,
  which binding and transform fields Perspective parses as expressions or
  property paths and which members of the component script object return
  its name, into `crates/iglint_project/src/generated/perspective_fields.rs`
  (`generated_fields_match_the_generator` fails when the generator changes
  without regenerating, `every_measured_field_is_modelled` when the view
  model misses a measured field).
- VIEW021 `invalid-view-structure` reports the parts of a view (and of
  the session properties) that have the wrong JSON type, once at the
  offending value: `"root": []`, `"children": "x"`, `"script": 7`,
  `"customMethods": [{"name": 5}]`, `events`, `scripts` or `propConfig`
  that are not objects, and more.
- SCR003 `compile-warning` reports what Jython's compiler issues a
  `SyntaxWarning` for, with Jython's message: `global x` after an
  assignment to or a use of `x` in the same scope, and `import *` below
  module level (in Perspective and event scripts, every `import *`). The
  set and the messages are checked against Ignition's Jython
  (`oracle/run-oracle.sh --warnings`, the `compile_warning_oracle` test).
- An unsafe fix of SCR102, VIEW015 or NAME001 says why when code that is
  not vetted, or that reads a frame, keeps it from being safe: the
  construct and its location, in the script or (closed world) the first
  such use in the workspace. Shown as a note in the text output, as
  `fix.unsafe_reason` in JSON, as a related location in SARIF, and in the
  language server's related information and code action title.
- `docs/compat/ign-lint.md` lists what changed between ign-lint 0.6.3 and
  0.6.4; configurations of both versions are accepted.

- SCR900 option `disable`. A configuration read from an ign-lint
  `rule_config.json` sets it to the messages ign-lint's combined file
  disables (`unused-argument`, `missing-docstring`,
  `redefined-outer-name`, `function-redefined`), also with a `pylintrc`
  of its own.
- Benchmark and parity harness in `bench/` (see bench/README.md).
- `SCR124` trailing-whitespace (off by default, safe fix).
- `SCR900` fixes pylint's `trailing-whitespace` like ign-lint's
  `PylintScriptRule` (every script line `rstrip()`ed).

### Fixed

- Large views took superlinear time (review round 14): 80,000 components
  took 11.5 s, 46 times as long as 10,000. The view model looked up the
  `propConfig` entries, properties and events of each component by
  scanning the whole view, `NAME001` collected the names of every sibling,
  looked up references and searched every string of the view once per
  renamed component, and `VIEW018` grouped expressions with a linear
  search. Lookups are now indexed: 80,000 components take 1.1 s (all
  rules: 1.3 s), 160,000 take 2.3 s. A release-only stress test checks
  that time grows linearly for views and modules.
- The scope oracle compares Python 2 tuple parameters too (review round
  14): the dump has the argument Jython loads (`load fast (b, (c, d))`)
  and the names its prologue stores, and the generator makes tuple
  parameters, nested ones included (a fresh run: 20,053 programs, 6,602
  of them with tuple parameters, 0 differences).
- Linting part of a workspace (one file, as the pre-commit hooks do, a
  project directory, `cd projects/A && iglint check --fix .`, standard
  input, a language server folder opened on one project) analysed only
  the projects around the linted paths, so a safe `SCR102` fix removed an
  import that a gateway tag script used as `m.os`, and a tag script that
  is not vetted did not make the fix unsafe (review round 14). The
  workspace (closed world, uses of library names, every cross-resource
  fact) is now everything below the workspace root of the linted paths:
  the configuration file's directory, the directory that holds the
  `projects/` or `config/resources/` layout, the Git work tree or the
  working directory. Excluded and ignored files still count, since they
  still run. Without a root the closed world is unknown and the fixes
  that need it are unsafe. `--show-settings` prints the workspace roots.
  A warm single-file run on a 3,000-file workspace takes about 55 ms
  (17 ms before, when it saw only one project).
- Every unsafe fix says why (review round 14): `SCR102` imports not known
  to be free of effects, names still read after the import or before a
  `from __future__` import, names other resources may use; `SCR103`,
  `SCR110`, `SCR115`, `SCR123`, `SCR206`, `SCR301`, `SCR900`, `VIEW012`,
  `VIEW013`, `VIEW015` and `NAME001` fixes that are unsafe by nature.
- `SCR002` missed "Module or method too large" far below its threshold of
  10,922 statement lines: Jython fails from 680 lines of
  `d["kN"] = {"a": N, "b": [1, 2, 3]}` (review round 14). It now adds a
  lower bound of the bytecode of every statement and expression,
  measured on Jython's code generator and checked against every body of
  about 2,300 generated programs (`oracle/run-oracle.sh --method-size`),
  and reports a body from about 740 such lines.

- `SCR102` removed imports that class bodies read, safely, and `--fix`
  broke the module (review round 13). A class body reads by name: a name
  it binds anywhere is looked up in the class namespace and then in the
  module's globals and the builtins, never in an enclosing function, so
  `x = m.pi` before the class's own `m = 1` reads the module's `import
  math as m`. And Jython mangles private names in class bodies (`__m` is
  `_D__m` in class `D`, also for import aliases, `def`, `class`, `global`,
  `except`, `with` and `for` names) and attribute names anywhere inside a
  class, but not names in methods. `SCR101` reported such names as
  undefined (`from math import pi as _D__pi` read as `__pi` in the class);
  it now shows the mangled spelling when it does report one. The semantic
  model resolves names the way Jython's compiler does, checked by a new
  differential oracle (`oracle/scope/`): which accessor Jython's code
  generator picks for every name read, store and delete of a program
  (by name, global, fast local, cell or free variable) with its run-time
  spelling, and every attribute name; 100,000 generated programs, the
  1,642 scripts of the private project and hand-written cases agree.
- Imports the interpreter reads without the code naming them counted as
  unused (review round 13, M1): `import math as __metaclass__` decides
  how a class without bases is created, and `SCR102` removed it safely.
  The model now records the implicit reads, measured with
  `oracle/run-oracle.sh --reads`: `__name__` when a class body starts
  and when a function, lambda or generator expression is created,
  `__metaclass__` after a class body without bases, `__name__`,
  `__package__` and `__path__` at every import, `__debug__` and
  `AssertionError` at `assert`. The safe-fix net also compares which
  kinds of bindings each read may find, not only how it resolves.
- `SCR002` missed compile failures in code Jython generates but cannot
  reach (review round 13, M2): a `yield` in a list comprehension, or in
  the first iterable of a set or dict comprehension, after a `try` that
  returns from both its body and `finally` (and 47 other control flow
  prefixes) crashes the bytecode verifier, because a generator resumes
  after every `yield`. Reachability is now one model for the compile
  errors and the scope oracle, checked against a cross product of every
  block-ending prefix, every compile error construct and 14 nesting
  contexts (`oracle/compile_cross.py`, 5,796 cases). `SCR002` also
  reports a private name in a class whose name is only underscores
  (Jython's compiler crashes), `exec` with free names and `import *` in
  a class nested in two functions, and `return` with a value after
  `yield` in module and class bodies, as Jython does.
- `--diff` wrote hunk headers that did not match their lines for the most
  common `SCR102` safe fix (an emptied import line before a blank line:
  `@@ -1,3 +2,2 @@`), which `git apply` and `patch` rejected. iglint now
  computes the hunks itself; a property test applies random diffs (blank
  line edits, CRLF, missing final newlines, several hunks) with both
  tools and checks every header.

- Every fix was safe although scripts read the content of the files it
  edits without naming them (review round 12). The `system.*` audit
  classified functions by their parameters only; it now classifies each
  of the 683 functions and objects of the stubs by what its outputs
  carry, and a test fails on one that is not classified. Possible reads
  now include `system.tag.getConfiguration`, `exportTags` to any path
  (`exportTags('/tmp/x.json', ...)` then `open('/tmp/x.json')` showed a
  tag event script's comments), `query`, `browse`, `getAttribute`,
  `getAlarmStates`, tag reads (`readBlocking`, `read`, `getTagValue`,
  ...) of a path that may name a configuration property such as
  `T.eventScripts` (literal value, `Quality` and `Timestamp` paths stay
  fine), `system.perspective.getProjectInfo`, the audit log, Vision
  windows, and every network client (`system.net.httpClient`, the `http*`
  functions, `java.net.URL` with a network URL): a gateway sharing the
  workspace's files serves them over HTTP. Expressions, Perspective tag
  bindings, reference and derived tags, tag-bound alarm properties and
  tag change scripts whose tag path may name a configuration property
  (`{[.]T.eventScripts}`, `tag(path)` with a computed path) count too,
  since the value they produce carries the configuration. The measured
  Java members that hand out such content (`getEventScripts`,
  `findViewConfig`, `getPropertyConfig`, ...) count like file handles.
  `system.tag.move` counts as code that stores tag scripts, like `copy`,
  and the legacy `system.db.exportCSV`, `exportExcel` and `exportHTML`
  as file facilities. All verified on Ignition 8.3.3.
- SCR112 missed `*` widths and precisions with a dict as the only
  operand (`'%*s' % {'x': 1}`, `'%.*f' % {'x': 1}`), which raise
  `TypeError: * wants int` on Jython.
- `bench/parity.py` compares the severity of matched findings and
  explains the three documented differences on the bundled fixtures;
  `cargo test -p iglint --test bench_parity` fails on any unexplained
  finding or severity difference.
- SCR102's safe fix removed the only import of a library module that
  reads `__package__`: in a library module every import statement that
  runs (failing, Java and `__future__` imports too, in any scope) binds
  `__package__` in the module's globals, so a function returning it gave
  `None` before the fix and raised `NameError` after (measured on Ignition
  8.3.3 through its own `ScriptManager`, `oracle/run-oracle.sh
  --implicit`, recorded in
  `crates/iglint_python_semantic/tests/oracle/implicit_names.expected`).
  The semantic model now binds `__package__` at each such import, so the
  safe-fix net sees the change, and the fix is unsafe whenever anything
  may observe `__package__` or `__warningregistry__`: a read or binding in
  the module, a binding of `__name__`, a read of the globals by name, or
  another resource reading them, the module's `__dict__` or the module as
  a value. Event scripts run without `__name__`, and their imports bind
  nothing (unchanged). SCR101 now reports a `__package__` that no import
  of the library module binds.
- SCR112 missed format specs with characters after the type
  (`'{0:dd}'.format(1)`, `'{0:%Y}'.format(1)`, `'{0:.2.2}'.format(1.5)`,
  `'{0:s.}'.format('ab')`, `'{0:#x#}'.format(5)`) and a `.` without
  digits (`'{0:5.}'.format(1)`), which Jython rejects for every literal
  ("Invalid conversion specification", "Format specifier missing
  precision"); objects with a `__format__` of their own (`datetime`) are
  still left alone. Jython also never reads an alignment character as a
  fill (`'{0:<<}'` is alignment `<` and type `<`), unlike CPython; SCR112
  now parses it that way. `oracle/format-spec.sh` generates characters
  after the type too: all 5,414,400 cases match Jython, and the replayed
  sample was redrawn.
- The rule catalogue (`docs/internal/rules.md`) labelled `NAME001`'s fix
  unsafe although it offers safe renames, and `VIEW013`'s although it is
  safe when there is no stored value. Rules whose fixes are safe or
  unsafe per finding (`NAME001`, `VIEW013`, `VIEW015`, `SCR102`,
  `SCR123`, `SCR900`) are now `S/U`; each rule declares its fix safety in
  the registry (`fix_safety`), the catalogue test checks the column
  against it, and the rule pages show it.
- `iglint check <dir>` on a directory without an Ignition project (a
  folder of loose `.py` files) printed only "All checks passed!". It now
  warns that no Ignition project or lintable files were found under it
  (exit status still `0`); a `.py` file named on the command line is
  still linted as a standalone script.
- `bench/__pycache__` is no longer committed; `__pycache__/` and `*.pyc`
  are ignored.
- File access is default deny for the file-read condition of every fix:
  every vetted facility that takes a path counts unless every path
  argument is a literal outside the workspace, whatever its mode or
  direction. `os.rename` of a module to `/tmp/m.py` followed by a read
  of `/tmp/m.py` kept `SCR102`'s fix safe, and `open(p, "a")` then
  `tell()` (which reveals the size) counted as write-only; now `open`
  and `file` in every mode, `os.rename`, `renames`, `remove`, `unlink`,
  `makedirs`, `mkdir`, `rmdir`, `utime`, `listdir`, `walk`,
  `os.path.exists`, `isfile`, `isdir`, `realpath` and the other
  path-taking attributes, `system.file.writeFile`, `fileExists`,
  `getTempFile`, `system.tag.exportTags`, the dataset exports and the
  other `system.*` functions that name local files, and the Java
  `FileOutputStream`, `FileWriter`, `PrintWriter` and `Formatter` count.
  Every vetted attribute of `os`, `os.path` and `sys`, and every
  `system.*` function with a file-like parameter, is classified, and a
  test fails on one that is not.
- Fixes of every rule were safe even when a script read the file they
  edit: `open(...).read()` of a module saw `SCR102` remove an import,
  `SCR124` trim a comment, `SCR123` remove a suppression and `VIEW015`
  remove a custom property from a `view.json` the view's own event reads.
  While any code in the workspace may read a workspace file (a read of
  `open`, `file`, `os.stat`, `os.access`, `os.path.getsize` and the other
  metadata functions, `system.file.readFileAsString` and friends, the
  legacy `system.net.http*` functions, `java.io.File`, `FileInputStream`,
  `FileReader`, `java.net.URL` or `URI` whose path is not a literal
  outside the workspace, a measured Java member that hands out a file
  handle such as `getParentFile` or `getProjectManager`, a Vision client
  script, or code that is not vetted or reads frames), every fix is now
  unsafe, and the unsafe reason names the first such site and the new
  setting. Reads of literal paths outside the workspace (`/tmp/x.csv`),
  write-only opens, directory listings and existence checks keep fixes
  safe. `docs/internal/fix-safety.md` has a new "File reads" section and
  no longer claims that whitespace and comments cannot be observed.
- Gateway managers reachable without `system.*` store or run code and
  configuration (`request['context'].getTagManager().importTagsAsync(...)`
  in a WebDev handler imports tags with their scripts from JSON). The
  generator now measures the gateway types that store or run code and
  configuration from data and the members that hand one out
  (`getTagManager`, `getGatewayContext`, `getScriptManager`, ...); naming
  one, or using a WebDev handler's `request` in a way that may reach its
  `context`, is unknown code for the closed world, with its site.
- SCR112 missed format specs that raise for a literal argument:
  `'{0:z}'.format(1)`, `'{0:.2}'.format(1)`, `'{0:+s}'.format('ab')`. It
  now checks the whole spec per literal type (presentation type, sign,
  alignment, `#`, `,`, zero padding, precision), matching Jython on every
  one of the generated cases (`oracle/format-spec.sh`, 5,414,400 since
  the entry below; a stratified sample of 4,000 is replayed by the tests).
- SCR102's documentation no longer says that removing an import of a
  missing JDK class is a remaining effect of the safe fix (that removal is
  unsafe since the previous round).
- Review round 10 (views): every field of a Perspective binding or
  transform that Ignition parses as an expression is now an expression of
  the view model, so all BIND rules, VIEW015 and NAME001 see it: HTTP
  bodies, header values and authorization values, tag history date
  ranges, most-recent values and return sizes, map transform fallbacks,
  list items of expression structures and indirect tag binding
  references. The fields are measured in the Ignition 8.3.3 image
  (`stubs-gen/gen_perspective_fields.py`, see "Added"). An HTTP `url` is
  always parsed, so a literal URL is reported (BIND001) instead of being
  skipped: Perspective rejects it ("Error parsing url: Invalid
  expression").
- NAME001 no longer offers a safe rename that leaves references dangling:
  references in the newly modelled fields are renamed (the fix is then
  unsafe, as for every referenced component), and the safe rename is
  withheld, naming the place, when a script reads the component's name
  through `name` or `getName()` (also for a component reached through
  `children`, `getChildren()`, a loop or an index; the name members are
  measured), when a string iglint does not read as an expression holds a
  `{...}` reference, when a reference is written with whitespace inside
  its braces, or when a string holds the component's name where the fix
  does not update it.
- BIND005 reports `{ view.custom.x }`: whitespace inside the braces makes
  Perspective read a property reference as a tag path (checked against
  Ignition 8.3.3's parsers). VIEW015 counts such a reference as a read of
  the property, so its fix never deletes it.

- Pages and sessions hand out other views (`page.views`, `getViews()`,
  `page.view`, `session.pages`, `getPages()`, `getPage(id)`), with their
  custom properties and components. Such a use anywhere in the workspace
  (view, component and session property scripts, session and page event
  scripts, library functions handed a component, view, page or session)
  is unknown code for the closed world, so VIEW015 and NAME001 fixes
  become unsafe and name the site. So is a component, view, page or
  session used as a value the analysis does not follow. The members of
  the page, session, view and component script objects are measured in
  the Ignition 8.3.3 image (`stubs-gen/gen_vetted.py`) and each is
  classified. Loops over children and truth or identity tests of
  components are followed instead of reading everything.
- SCR102: removing an import is safe only with positive evidence that it
  succeeds. Java classes must be measured to exist in the image (public
  classes of the vetted packages), and `from m import n` needs `n` to be
  a measured attribute of `m`; removing `from java.util import Missing`
  changed whether a caller's `except` ran.
- SCR102 no longer reports a library module's import that another module
  reads through the module object: `getattr(a.b, "json")` and
  `hasattr(...)` with a literal name are uses of `a.b.json`, and a module
  used as a value (a default argument, a container, `getGlobals()`,
  `sys.modules["a.b"]`, also reached as `re.sys.modules`) may have any of
  its names read.
- REF009 reports resource manifests and data files outside every project
  and gateway configuration collection (a `config/resources/core` tree
  without `config-mode.json`): Ignition does not load them (verified on
  8.3.3; a `core` directory without the manifest keeps the gateway from
  starting), and iglint does not read their tag event scripts.
- Malformed view parts that may hold scripts (a component, `events`,
  `scripts`, `propConfig`, a custom method without a string name, a
  script that is not a string) are unknown code for the closed world;
  malformed tag folders too.
- SCR112 reports literal arguments of the wrong type: numeric
  conversions and `*` with strings, complex numbers, containers or
  `None` (`'%d' % 'ab'`), `%c` out of range, and presentation types the
  literal does not support in `str.format` (`'{:d}'.format('a')`,
  `'{:s}'.format(1)`, any type but `s` after `!r` or `!s`). Checked
  against 3,000 generated cases on Jython: no finding on code that
  formats.
- Code that runs from expressions and run-time strings counts for the
  closed world. `runScript` with a computed script, or with code that is
  not vetted or does not parse, is unknown code; literal code is analysed
  like a script and a literal dotted name is a library reference.
  Resources iglint does not read (reports, alarm pipelines, SFC charts,
  Vision, transaction groups, event streams, legacy and unknown resources)
  and `runScript` text outside modelled expressions are unknown code. An
  audit of every `system.*` function marks those that store or run code
  from run-time values (`system.tag.configure`, `importTags`,
  `system.util.execute`, `system.project.requestScan`, ...) as not
  vetted, and `system.tag` writes as vetted only with value paths.
- The machine-measured Java reflection table now covers every type a
  script can hold: a transitive fixed point over the public members of
  `Object`, `Throwable`, `Thread`, class proxies and reflected functions,
  the vetted classes and packages, the types `system.*` returns in every
  scope and the objects Ignition hands to scripts (Perspective wrappers
  and events, datasets, qualified values, loggers, tag, alarm, WebDev,
  Vision and SFC objects), through return, field, generic, exception and
  callback types, stopping at reflection and interpreter types
  (`ScriptManager`, `PySystemState`, frames, modules). It grows from 43 to
  145 names (`module`, `argslist`, `getColumnTypes`, `getScriptManager`,
  `type`, ...), each with the member it was found on. Jython's lookups by
  name (`invoke`, `__findattr__`) are reflective accessors. String
  literals are scanned for these names and the introspection attributes
  only in sink positions (accessor names, literal format strings, code
  for `eval` and `exec`); a sink with any other argument, and `vars(o)`,
  is an escape. Literals that are data (`{'type': 'popup'}`) no longer
  hide findings.
- The vetted-code guard resolves names through the semantic model's
  scopes, flow-insensitively, instead of in the order its walk met the
  imports. A function defined before `import re` that reads `re.sys` (and
  from there `eval`) was not seen, so SCR102 removed an import that code
  read by name and VIEW015 removed a property; now every binding of a name
  in the scope it resolves to counts, a single unvetted one makes the use
  unknown, and names the model cannot resolve fail closed.
- SCR002 reports a `yield` inside a list comprehension, or in the first
  iterable of a set or dict comprehension: Jython's compiler crashes on it
  (`java.lang.VerifyError`). The exact crashing and compiling forms were
  mapped against the compile oracle.
- The vetted-code guard no longer trusts hand lists for two routes that
  defeated them. Java reflection reached through a bean property
  (`x.class`, `x.classLoader`, `x.methods`) and vetted modules that hand
  out `sys` (a `PySystemState` in Jython: `re.sys`, `calendar.sys`) are
  now caught by tables measured inside the Ignition image
  (`stubs-gen/gen_vetted.py`, `stubs-gen/run.sh --vetted`); subclassing a
  Java class is unknown code too.
- Safety scans decode a `str` literal that is not valid UTF-8 as Latin-1
  (Jython's byte-to-char mapping) instead of reading it as empty, so a
  `'{0.f_locals}\xff'` format string or `getChild` argument can no longer
  slip a safe fix through.
- Closed world: a script that reads a frame or the interpreter, anywhere
  in the shared-runtime workspace, now blocks the safe fixes of SCR102
  (including function-local imports), VIEW015 and NAME001, not only when it
  is in the same resource.
- `--fix` reaches a fixpoint in one run: a safe fix rejected only in
  combination with another is retried in a later pass, so removing unused
  imports whose joint deletion would promote a docstring no longer needs a
  second run. `--fix` reports a clear error if fixes do not converge.
- Safe fixes now assume only vetted code (`docs/internal/fix-safety.md`,
  "Threat model"), instead of a list of dynamic routes that six review
  rounds each defeated. A script or library module keeps the safe fixes
  of SCR102, VIEW015 and NAME001 only when every module, Java class,
  builtin and module attribute it uses is on a default-deny allowlist
  (checked on Jython: none can evaluate code, read a frame or look up
  names from data), and every library module it imports or calls is
  vetted too. `cPickle` (whose `c__builtin__` opcodes call `eval` or
  `globals()` in the caller's frame), `warnings`, `logging`, `traceback`,
  `org.python.*`, `com.inductiveautomation.*`, `java.lang.Thread`,
  `Throwable`, `Class` and anything else not listed make those fixes
  unsafe; findings are unchanged. Library-module imports also need every
  resource of the project to be vetted.
- Safe fixes never move a line: SCR102 and SCR123 empty the line of a
  deleted statement or comment and keep it (the line-removing fix is now
  the unsafe one). Line numbers read through `warnings`, `logging`
  records, tracebacks or Java stack traces no longer change under
  `--fix`.
- A central safety net drops any safe fix that would move a line of a
  script, change whether a script compiles (in either direction: removing
  `import os` before a misplaced `from __future__ import division` made it
  valid), change a docstring, the `__future__` imports, the scopes or a
  generator, or what any remaining name resolves to (removing
  `import math as len` after `print len(x)` in a function turned an
  `UnboundLocalError` into a builtin call). It covers `.py` files and
  every script embedded in view and resource JSON (two deletions in a
  view script no longer promote a string to a docstring).
- SCR102 no longer offers a safe fix for an import that is the only
  binding of a name still read in its scope, or that comes before a
  `from __future__` import.
- Frame detection treats reflective accessors used under another name or
  as values (`get = getattr`, `map(getattr, ...)`,
  `operator.attrgetter` as a value), `getattr(o, 'class')`, format
  strings that are not literals (`fmt.format(o)` reads the attributes the
  string names), `__subclasses__`, `__mro__`, `__bases__` and Java class
  loaders as frame routes, besides introspection attributes named in
  source or strings, reflective accessors with computed names and the
  interpreter and reflection APIs.
- SCR002 reports a recursive construct once, not once per nesting level.
- Language server quick fixes go through the same fix function as
  fix-all and `--fix`: a fix that would make the file worse, or a safe fix
  that fails the safety net, is no longer offered.
- VIEW015 no longer removes properties that a library function reads into
  a local it never uses (`x = c.custom.foo`, also inside `try`), calls
  (`f = c.custom.foo; f()`) or reads through an alias of the container
  (`cu = c.custom; x = cu.foo`); that a script or library function reads
  through a method of the property wrapper used as a value
  (`map(self.custom.get, ['foo'])`, `g = c.custom.get; g('foo')`,
  `invokeAsynchronous(c.custom.keys)`), through a page or session, an
  unknown member of a component or a component method used as a value;
  that `__builtin__.eval`, `from __builtin__ import locals as l` or a
  library variable holding `eval` read through the frame; or that a
  library function reads after being replaced through an alias
  (`alias = handle; alias.func_code = ...`, a helper that receives it),
  `importlib.import_module`, `func_globals` or `__globals__`.
- SCR102 and the other unused-name rules no longer remove imports or
  names that a frame builtin reached under another name can read
  (`from __builtin__ import locals as capture`, `e = eval`,
  `__builtin__.eval`, `sys._getframe`).
- Safe fixes of NAME001, SCR102 and SCR123 become unsafe when a script
  may read names or frames dynamically: `eval` or `locals` under another
  name (also a library variable holding one, checked through the project
  index), calls of unnamed values (`factory()()`, `handlers[k]()`), or,
  for SCR123, a deleted line in a script that reads frames. SCR102 keeps
  module imports when the module reads its globals with a computed
  `getattr`. VIEW019 and BIND unresolved property references treat such
  names as writing anything. NAME001 and VIEW019 now run in the project
  phase.
- SCR002 no longer reports invalid assignment targets in unreachable
  code that Jython generates but never runs into (after `break`, a
  `try` whose body and handlers or whose `finally` do not complete, a
  loop whose `else` does not complete).
- SCR112 reports `%c` with a string literal that is not one character
  (`'%c' % 'ab'`) and checks format strings concatenated with `+`
  (`('%s' + '%s') % (1,)`).
- Deleting a statement (SCR102 import removal and other deleting fixes) no
  longer changes a docstring: removing the first statement of a module,
  class or function body would promote a following bare string to the
  body's `__doc__`. The shared deletion helper keeps a `pass`, and a
  post-fix safety net drops any `.py` fix that changes the set of
  docstrings, covering several deletions that together expose a later
  string.
- The command line `--select` now wins over `[[overrides]]` as the README
  promises: it replaces the selection everywhere, so an override's
  `select` / `extend-select` can no longer re-add a rule the command line
  left out, and `--ignore` removes a rule an override selects. This holds
  in ign-lint compatibility mode (the `view.json` scope overrides) too.
- Library-name detection in script accesses now resolves every name
  through its real Python 2.7 scope (the semantic model), so a name bound
  only in a nested function, class or comprehension no longer hides a
  library reference of the same spelling in the enclosing scope. Before,
  `def f():` with a nested `def dummy(lib): pass` made `lib.util.ev(...)`
  invisible, so SCR102 removed an import and VIEW015 removed a property
  that the call reached. Module-level names, imports and injected library
  packages stay library-name candidates, so the module's own functions are
  still followed.
- The rule catalogue (`docs/internal/rules.md`) Fix column is kept in
  step with the registry by a test: `SCR110`, `SCR115`, `VIEW012` and
  `VIEW013` were labelled wrongly (safe or none) although their fixes are
  unsafe.

- `VIEW015` no longer trusts the body of a library function that may be
  replaced at run time, so its safe fix no longer removes properties the
  function that actually runs reads. A module's top-level name is no
  longer a function (for every rule) when the module may rebind it:
  `exec` (also inside a function, where the code may declare a name
  `global`), `eval`, `execfile`, writes to `globals()` or
  `sys.modules[__name__]`, a later `from x import *`, or a replaced
  `func_code` / `func_defaults`. The project index records library names
  that scripts assign, delete or `setattr` (also through aliases,
  `__dict__`, `sys.modules['pkg.mod']` and literal `exec` / `eval` code):
  `VIEW015` and `REF003` do not use the definitions of such names, and
  `REF002` does not report a name another script creates. `VIEW015` also
  treats functions of modules used as values (or reached through
  `sys.modules` or code run from strings) as unknown.
- Literal code run with `exec`, `eval` or `compile` counts as references:
  a library name used only there is no longer unused for `SCR102` (whose
  safe fix removed the import) and `REF012`. `execfile` and
  `__import__` with a literal name make the modules they reach escape.
- `SCR002` counts only the code Jython generates: nothing after `return`,
  `raise`, `continue` or an `if`/`else` whose branches all end that way
  (not for the module's own statement list). A function with 12,000 dead
  `pass` lines, or 40,000 dead string literals, is no longer reported.
  Errors of Jython's code generator (`'break' outside loop`, `'return'
  outside function` in a class body, `import *` in a function, a bare
  `except:` that is not last, ...) are no longer reported in dead code,
  which Jython compiles.
- `VIEW015` no longer follows a library function's parameter stored in a
  name the function declares `global` as a local alias: the store is an
  escape, so the safe fix no longer removes a property that another
  function of the module reads later through the global. A `**kwargs`
  or `*args` parameter named like a library package is no longer taken
  for the package, and code that reaches stack frames
  (`sys._getframe()`, `inspect.currentframe()`, `.f_locals`) counts as
  reading everything, in library functions and in view scripts.
- `SCR102` no longer reports a top-level import of a library module that
  another resource star-imports (`from pkg.mod import *` binds it there):
  every name not starting with `_`, or only the names of `__all__` when
  the module assigns it unconditionally, counts as used.
- `SCR103`'s fix keeps the value of a list comprehension (removes only
  `x = `): Python 2 leaks its variables, so removing the statement broke
  code that read them later. Backticks count as a call of `repr`, for
  `SCR103` and `SCR114` (which no longer reports backtick or list
  comprehension statements).
- `--diff` output applies with GNU `patch -p1` when a path holds spaces
  (`views/My View/...`): the file name is followed by a tab, as `diff -u`
  writes it. Paths with `"`, `\` or control characters are quoted like
  git does. Both `patch` and `git apply` accept the result.
- `REF009` reports a Python file that is not the resource's script once,
  on the file, instead of a second time on `resource.json` as an
  unlisted file (listing it would not make the gateway load it).
- `SCR112` reports `.format` calls Jython always fails on that it
  missed: a negative field index (`'{-1}'.format(1, 2)`,
  `ArrayIndexOutOfBoundsException`), a format spec width or precision too
  large for a Java `int` (`'{0:2147483648}'.format(1)`, `ValueError`) for
  literal arguments and `!r` / `!s` fields, and missing arguments when
  `*args` or `**kwargs` are literals (`'{0}{1}'.format(*(1,))`,
  `'{a}'.format(1, **{})`).
- `SCR112` treats a `.format` field too large for a Java `int`
  (`{2147483648}`, also `{2147483648.x}` and in nested specs) as the
  keyword it is in Jython: no finding when `**kwargs` may supply it.

- `iglint ign-lint --version` prints the ign-lint version it emulates:
  `ign-lint 0.6.4 (iglint 0.1.0 compatibility mode)`.
- The oracle scripts name their containers (`iglint-oracle-<pid>-...`)
  and remove them when interrupted.
- `VIEW015` follows the library functions view scripts hand `self`, the
  view or a property container to (`lib.util.handle(self)`,
  `handle(self=self)`): a function that does not exist reads nothing (the
  call fails; `REF002` reports it), and an existing one is followed
  through its body (`param.view.custom.x`, `param.getSibling('A')`,
  further library calls up to 4 deep). Anything that cannot be followed,
  and functions a child project overrides, still count as reading
  everything. The fix is unsafe when something writes the property.
- `iglint migrate` keeps ign-lint's scope: the rules ign-lint adds to
  iglint's defaults (above all `SCR900`) are selected for `view.json`
  files only, through an `[[overrides]]` entry, instead of for the whole
  repository. pylint no longer runs on library modules and WebDev
  scripts after a migration (thousands of extra messages, among them
  `W0311` from ign-lint's tab-indented rcfile on space-indented code),
  and the migrated `iglint check` is several times faster. `iglint check`
  with a `rule_config.json` uses the same scope.
- Resources without `resource.json` are no longer silently skipped: a
  directory that holds the main file of a resource type (`view.json`
  below `views/`, a library `code.py`, an event script, a style class,
  tags, ...) is linted as that resource, and `view.json` and Python files
  outside every resource directory are linted standalone (with the
  project's library names). `REF009` reports that the gateway does not
  load them; references do not resolve to them and they are not
  inherited. `iglint ign-lint` lints such views too. Python files of
  resource types iglint does not model (and WebDev file content) are
  named in the run summary (`note: ...: not linted: <reason>`) instead of
  being counted as checked.
- Safe fixes no longer change behaviour (audit in
  `docs/internal/fix-safety.md`): `SCR102` leaves imports in class bodies
  alone and its fix is safe only for imports without side effects (JDK
  classes, verified standard modules); `SCR115` resolves the handler name
  like Python and its fix is unsafe; `NAME001` renames are unsafe when a
  view script hands a component to code outside the view; `SCR110`,
  `VIEW012`, and `VIEW013` with a stored value, have unsafe fixes; `SCR123`
  is unsafe when the comment holds more than the suppression.
- `SCR103`'s fix removes the whole assignment when the value calls nothing,
  instead of leaving a pointless statement.
- `SCR112` follows Jython's format grammar: one length modifier at most
  (`%lls` fails), `*` values of `%%` are consumed, huge `str.format`
  indexes are keywords (no overflow), signed indexes, `Missing ']'` and
  recursion errors.
- `SCR108` folds `True`, `False` and `None` only where they are builtins.
- `SCR900` passes every argument of the `pylint` command to pylint, also
  when forking (quoted words are supported), and `category-mapping` wins
  over the rule severity for WebDev scripts too.
- `SCR204` names documented replacements Ignition does not name itself
  (`system.tag.readBlocking` for `system.tag.read`, ...).
- Deeply nested or very long scripts no longer abort the run with a stack
  overflow (`elif` chains, `a.b.c...`, `x[0][0]...`, `1 + 1 + ...`, prefix
  operators, in modules and in view scripts, also through the language
  server). Long chains are parsed with loops and freed without recursion,
  and linting runs on threads with 256 MiB stacks (reserved, not
  committed). A statement deeper than 16,384 tree levels is reported as
  SCR001 "expression nested too deeply" (Ignition's Jython fails much
  earlier on such code). Expressions may now nest 2,000 levels instead of
  100, which is more than Jython accepts on a gateway thread.
- JSON nested deeper than 512 levels is no longer reported as invalid
  (`VIEW001` "nesting too deep"): Gson reads any depth. iglint parses,
  walks and frees JSON of any depth without recursion and analyses files
  nested up to 8,192 levels; `VIEW001` reports deeper ones, which
  Perspective cannot open (its view loader and property trees overflow
  the stack at 1,200 to 3,300 levels). `VIEW006` measures deep `custom`
  data in linear memory.
- `iglint ign-lint` no longer rejects views nested more than 128 levels
  deep as invalid JSON; it lints them like `iglint check` (ign-lint
  itself crashes from about 500 levels).
- `SCR002` reports scripts Jython cannot load because they are too large:
  first two lines of 100,000 characters or more (`Mark invalid`; exact),
  bodies with more than 10,922 statement lines (`Module or method too
  large`) and modules with more than 32,767 distinct string literals
  (`Class too large`). Thresholds in `docs/internal/ignition-format.md`.
- The text output no longer slows down quadratically with many findings
  on one very long line (a script in a view is one JSON line): 100,000
  findings took 74 s, now 0.3 s.
- `iglint ign-lint` prints ign-lint node paths for every finding,
  including BIND005 findings in query binding parameters and other binding
  expressions.
- `\N{...}` escapes in unicode literals resolve every name IA's Jython
  2.7.3 accepts (its Unicode 9.0 table, with its lookup quirks); unknown
  names are reported as SCR001, as Jython rejects them.
- `iglint ign-lint` groups pylint messages by category
  (`Pylint - Convention (C):`) like ign-lint.
- pylint messages that a native rule also reports (same place and name)
  are shown once.
- SARIF artifact URIs are percent-encoded (RFC 3986, per path segment) in
  results, related locations and fixes; files outside the working
  directory get absolute `file:` URIs instead of a path relative to
  `%SRCROOT%`.
- Azure Pipelines messages no longer show `%3B`/`%5D`: only `%`, `\r` and
  `\n` are escaped in the message, `;` and `]` only in properties.
- Fixes that delete statements (`SCR102`, `SCR103`) no longer empty a block
  when several of them remove every statement of it: the last one becomes
  `pass`.
- Fixing never makes a file worse: when the fixes of a pass make a script
  or expression stop parsing or compiling, empty a Perspective script or
  break the JSON, they are applied one at a time and the offending ones are
  dropped and reported (`warning: ...: fix not applied`).
- `SCR102` no longer reports top-level imports of library modules that other
  resources use as `pkg.mod.name`, and its fix is unsafe when such uses
  cannot be ruled out.
- `SCR101` no longer reports names created by `exec code in globals()` (and
  the other qualified `exec` forms that write the module or the function's
  locals), by `setattr(sys.modules[__name__], ...)`,
  `sys.modules[__name__].x = ...` or by writes to the builtins.
- `SCR108` compares keys with Python 2.7 equality: `True` and `1`, `0j` and
  `0`, equal tuples, `1e16` and `10000000000000000` are duplicates.
- `except ValueError, TypeError:` is reported once, by `SCR115`; `SCR103`
  and `SCR117` no longer report the same name.
- `iglint ign-lint`: the timestamps of the `--timing-output` report and of
  the aggregated `--results-output` summary are in local time, like
  ign-lint's.
- `All checks passed!` (and `iglint ign-lint`'s "No style inconsistencies
  found!") is no longer printed when the run had errors.
- Files that are not valid UTF-8 are linted instead of failing the run: the
  new rule `VIEW007 invalid-utf8` reports the invalid bytes (Ignition
  replaces them with U+FFFD), and such files get no fixes.

## [0.1.0]

First public release.

### Added

- Linter for Ignition 8.3 projects on disk: Perspective views, Jython 2.7
  scripts (project library, gateway and session events, WebDev, tag events,
  scripts embedded in views), expressions, style classes, page configuration,
  session properties, named queries, resource manifests and gateway tag
  resources.
- More than 100 rules in ten categories (`VIEW`, `NAME`, `BIND`, `SCR`,
  `REF`, `PERF`, `SEC`, `A11Y`, `STYLE`, `TAG`), with cross-resource checks
  over the whole project and its parent projects. Reference in
  [docs/rules](docs/rules/README.md), generated from the rule metadata.
- Jython 2.7 parser, semantic model and expression parser tested against the
  parsers shipped in the Ignition 8.3.3 image; `system.*`, expression
  function and Perspective component facts generated from a real gateway.
- Safe and unsafe auto-fixes (`--fix`, `--unsafe-fixes`, `--diff`) that only
  change the bytes they fix.
- Configuration in `iglint.toml`, `.iglint.toml` or `[tool.iglint]` in
  `pyproject.toml`: rule selection, per-rule severity, options and excludes,
  `[[overrides]]`, `strict`.
- Script suppression comments (`# iglint: ignore[...]`,
  `# iglint: ignore-file[...]`) and a baseline file
  (`iglint baseline`, `--prune-baseline`) that is portable across platforms.
- Output formats: text, concise, grouped, JSON, JSON lines, SARIF, GitHub
  Actions, Azure DevOps, GitLab Code Quality, JUnit and pylint.
- ign-lint 0.6.4 compatibility: `iglint ign-lint` (or the binary called as
  `ign-lint`) with ign-lint's arguments and output, `rule_config.json` and
  whitelist support, an equivalent for every ign-lint rule, and
  `iglint migrate` to convert the configuration.
- Result cache and parallel checking.
- Language server (`iglint server`) and a VS Code extension.
- Distribution as prebuilt binaries, a PyPI wheel (`pip install iglint`),
  pre-commit hooks and VS Code packages.

[Unreleased]: https://github.com/Mustry-Solutions/iglint/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/Mustry-Solutions/iglint/releases/tag/v0.1.0
