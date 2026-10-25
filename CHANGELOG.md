# Changelog

All notable changes to iglint are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and versions follow
[Semantic Versioning](https://semver.org/). Rule codes and names are stable
from 0.1.0 on.

## [Unreleased]

### Changed

- `bench/README.md` re-measured (2026-10-24, small, medium, large and the
  private project, with and without pylint, all CPUs and 2 CPUs) with
  every 10x claim qualified: without pylint 10x or more from about 300
  views on, 8x to 11x on the 50-view corpus depending on the run; a
  breakdown of the small corpus shows no fixed start-up cost to cut
  (2.3 ms for an empty directory; most of the time is indexing the rest
  of the repository).
- `bench/README.md` and the text summary of `bench/parity.py` say plainly
  that pylint-derived findings (`PylintScriptRule`, `SCR900`) are outside
  the parity comparison, and how they differ by design (the combined-file
  `C0103` and `W1405` messages dropped, no `E0602` for injected names,
  `W0702` reported once as `SCR106`).
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

- `[fix] vetted-system-functions` and `[fix] vetted-expression-functions`
  (default empty): `system.*` paths and expression functions of
  third-party modules that the project declares unable to evaluate code,
  read a frame or read workspace files, so that they do not count as
  unknown code. Checking that is the user's responsibility; `SCR201`'s
  `allow` and `BIND002`'s `extra-functions` only silence the findings.

- A drop-in `ign-lint` pre-commit hook (review round 14): the same id,
  `files` pattern (`view\.json$`), types, default `args`
  (`--config=.ignition-lint-precommit.json`) and file passing as
  ign-lint's own hook, running `iglint ign-lint`. Documented in the README
  and `docs/compat/ign-lint.md`.
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

- A path through another library module's import was not followed
  (review round 28, blocker; since the first project index): for
  `m0.util.json.dumps(1)` where `m0` does `from lib import util`, the
  index counted `m0.util` only, so `SCR102` removed `import json` from
  `lib.util` as safe and the caller then failed with `AttributeError`;
  `REF012` reported functions only reached that way and `REF002` missed
  `m0.util.nothere()`. The same for `import lib.util as util`, `import
  lib.util`, `import m1` and chains of any length, `from m0 import
  util`, `from m0.util import json`, `import m0.util.json` and
  `runScript("m0.util.json.dumps", 0, 1)`. Measured with Ignition 8.3.3's
  ScriptManager (38 chains, `chains.cases` of the library-routes
  oracle). There is now one resolver of dotted library paths
  (`LibraryTree::resolve_path`), which follows library imports with a
  bound of 64 and records every name a path reads; usage counts, values,
  dynamic references, exposed names, star imports, the shared-object
  reach, stores, rebinding, vetting, `REF002`, `REF003`, `REF012`,
  `SCR102`, `VIEW015`'s call following and the language server's
  dependencies all use it (some followed imports on their own, with a
  bound of 8, others not at all). Default deny where a path cannot be
  followed with certainty: a name bound several times through imports
  makes each import's target used as a value, a missing name in a module
  with star imports continues in each star-imported module, and a cycle
  or the bound makes what it reaches used as a value. A library module
  used as a value now also makes the library modules it imports used as
  values (`get().x.json` reached a module `get` returns), and a name that
  may hold an import from outside the library through several bindings
  reaches a shared object (`del m0.j.dumps`). `REF003` now checks calls
  through imports (`m0.util.fmt()`), and `REF002` reports
  `m0.util.nothere` with its segment in the path as written.
  Documented in `docs/internal/fix-safety.md`, "Paths through library
  imports"; oracle replay, property test (400 generated libraries) and
  generated end-to-end chains.
- A `runScript` target written as a dotted name was never analysed as
  code (review round 27, blocker; since round 8). Measured with
  Ignition 8.3.3's own `ScriptFunction` and Perspective's
  `PerspectiveRunScriptFunction`: `runScript` compiles `"__RESULT = " +
  script`, runs it with the library, `system` and `sys` (Perspective's
  also `self`, `view`, `page` and `session`) as locals, and calls the
  result with the other arguments when it is callable, also with none.
  So `runScript("m.f.func_globals.has_key", 0, "re")` read `m`'s
  namespace while `SCR102` removed `import re` from `m` as safe (the
  tag's value changed from `true` to `false`), and `runScript("eval", 0,
  ...)`, `"execfile"`, `"open"`, `"system.file.readFileAsString"`,
  `"system.util.execute"`, `"system.tag.configure"` or an unknown
  `system.*` function kept every safe fix safe. Now a dotted name is the
  call `name(*args)` with unknown arguments, analysed like a script
  (vetting, frame routes, closed world, file reads, references), its
  object a library value (a module, package or import that is not
  callable becomes the expression's value); any other literal is the
  code `__RESULT = <literal>`. In Perspective expressions, code that
  names `self`, `view`, `page` or `session`, or a computed script, is
  unknown code and makes `VIEW015` count every property of the view as
  read. The project index judges every segment of every dotted library
  reference, whatever produced it (scripts, imports, expressions),
  default deny: after a function any attribute but its name and
  documentation, after a class, variable or import any special
  attribute or member of Jython's function, method, class, instance and
  generator objects (measured), makes the module a value use, like
  `vars(m)`. The library-routes oracle measures those members and 56
  `runScript` expressions; the replayed table has 273 routes. Only
  `runScript` evaluates code among the expression functions of the
  8.3.3 image (every `Function` class of common, gateway and the
  modules scanned).
- SCR120 exempted `max(g)` and `min(g)` of a generator expression made
  in a loop when `*args` could add a positional argument (review round
  27, Codex: `out.append(max((v * t for v in [1]), *[None]))` keeps the
  generator, Jython prints `[[2], [2], [2]]`). The single-iterable
  exemption now counts the items of a literal `*args` and is not granted
  for any other; `**kwargs` adds no positional argument (verified on
  Jython). The other builtin exemptions do not depend on the number of
  arguments.
- Closures that read a function's local took time quadratic in their
  number when the local is bound many times (review round 27, Opus nit:
  32,000 lambdas reading one rebound local, 3.4 s): each read copied and
  hashed every binding of the name. The list is built once per function
  and name and shared; 32,000 closures now take 0.44 s with every rule.
  32,000 `try` blocks whose handler keeps an earlier binding of a name
  reaching remain quadratic with a small constant (1.4 s; the set of
  reaching bindings grows with every block), a known limit far above
  Jython's method size limit. Both shapes are in the release stress test
  `library_modules_take_linear_time_in_their_names`.
- SCR102 removed an import of a library module as safe while another
  module reached it through the module object (review round 26, blocker):
  `m.__findattr_ex__('re')`, `m.dict['re']`, `m.getDict()['re']`,
  `m.__reduce_ex__(2)[2]['re']` and the other special attributes of
  Ignition's library module and package objects (measured on 8.3.3:
  `ProjectScriptModule`, `ScriptPackage`). Default deny: a special
  attribute anywhere in a chain from another module (`m.__dict__`,
  `m.__class__`, `pkg.m.__getattribute__`, `m.f.__call__`, ...) uses the
  object before it as a value, like `vars(m)`; the literal accessors
  `__getattribute__`, `__getattr__`, `__findattr__` and `__findattr_ex__`
  with a literal name count like `getattr(m, 'n')`, `__setattr__` and
  `__delattr__` like `setattr` and `delattr`; the Java members of the
  module and package objects (`dict`, `getDict`, `setCode`,
  `reloadModule`, `name`, `path`, `pkg.clear`, `pkg.setDict`, ...) make
  the module a value unless it binds the name itself (a name it binds
  wins, measured). REF012 no longer reports a function of such a module
  (`m.__getattribute__(n)()` was a false positive), and REF002 no longer
  reports the members as missing. Every measured member and 80 routes
  (aliases, containers, package chains, `operator`, `inspect`, `copy`,
  `pickle`, `object.__getattribute__`, `type(m).__getattribute__`) are
  replayed from the oracle (`oracle/run-oracle.sh --library-routes`,
  `rules::refs::library_routes`, 137 routes).
- The namespace of a library module holds its `__builtins__`, so
  `m.dict['__builtins__']['locals']()` reads the caller's frame
  (measured): SCR102 removed the caller's own import as safe. `dict`,
  `getDict`, `setCode`, `__reduce__` and `__reduce_ex__` are frame
  routes on any value (fixes unsafe). On a value that may be a module (a
  name bound by an import or bound nowhere, and attributes of one),
  `vars(m)`, `m.__dict__`, `m.dict`, `m.getDict()`, `m.__reduce_ex__` and
  `getattr(m, '__dict__')` observe names, so the findings they would make
  false (SCR102, SCR103, REF002, REF012 in the script that uses them) are
  not reported (review round 26, M2: `vars(json)['__builtins__']`).
- SCR123 reported, and its safe fix deleted, suppressions in scripts
  that do not parse, where no rule that reads the syntax tree ran (review
  round 26, M1; fix-all on save in the middle of an edit). Such scripts
  are not judged, and they count as referencing anything for the unused
  rules elsewhere (REF011 to REF014, and SCR102's safe fix of a library
  module's import).
- SCR120 exempted every argument of `sorted`, `min`, `max`, `map`,
  `filter` and `reduce` (review round 26, Codex: `max(lambda: t, lambda:
  t)` returns the lambda). Only the arguments a builtin calls (`cmp` and
  `key` of `sorted`, `key` of `min` and `max`, the function of `map`,
  `filter` and `reduce`) or consumes are exempt. A generator expression
  kept in a loop (`out.append(v * i for v in y)`) is now reported, and a
  generator function whose every call is consumed in place
  (`total += sum(h())`) is not.
- A module where `n` functions bind one global that module code reads
  `n` times took cubic time (review round 26, M4: 4,000 of each, 7.1 s
  with `--select SCR001` and 21 s with every rule; now 0.01 s and 0.04 s).
  The lists of bindings that many references find are built once and
  shared. Long lists of reaching bindings are shared too: a name bound
  in 10,000 branches and read 10,000 times took 1.2 GB and 3.0 s, now
  87 MB and 0.5 s (their merges stay quadratic with a small constant).
  SCR104, SCR108, SCR113 and SCR900 count lines with line indexes.
- `bench/parity.py` resolves a component's own `propConfig`: a component
  custom property with a change script and ign-lint's definitions from a
  component's `propConfig` keys are the documented differences (0
  unexplained on the fixtures).

- A library module whose namespace another module reads through
  `__dict__` (`m.__dict__[name]`) counts as used as a value, like
  `vars(m)` and `getattr(m, name)` (review round 25, N2): REF012 no
  longer reports its functions and SCR102 its imports. Fixes that depend
  on the module's names only become more conservative. No change on the
  private corpus.
- SCR120 reports generators made in a loop that read its variable:
  calling a generator function (`def h(): yield t`, then
  `out.append(h())`) runs none of its body, and a generator expression
  returned by a function called at once (`(lambda: (t for _ in [0]))()`)
  runs when it is consumed, so each gives the last value (Jython:
  `[[2], [2], [2]]`; review round 25, Codex). It no longer reports a
  lambda passed directly to the builtin `sorted`, `min`, `max`, `map`,
  `filter` or `reduce`, which call it only while they run
  (`[sorted(rows, key=lambda r: r[c]) for c in cols]`; review round 25,
  N1), nor a generator expression such a function passes to a builtin
  that consumes it (`sum`, `any`, `''.join`, ...). No change on the
  private corpus.
- Project library modules with many top-level names were analysed in
  time about cubic in their number (review round 25, M3): the project
  index decided for every library value from scratch whether its module
  may hold a shared object and looked symbols up by a linear search; the
  flow analysis copied every local at each comprehension, branch, loop
  and `try` and merged binding sets one id at a time (it now shares the
  state in a persistent tree and merges sorted sets); the vetting walker
  listed every binding of a name at each use; `SCR003`, `SCR102` and
  `SCR103` searched the module once per `global` declaration, import or
  finding; and every module-level read went through every module
  binding of its name. `Binding::shadowed` of the semantic model is now
  recorded only for the kinds the analyses read (definitions, imports,
  augmented assignments). Release builds on one machine: 2,000
  statements `bI = a` 4.7 s to 0.01 s, 2,000 pairs `cI = 1; bI = cI`
  12.5 s to 0.01 s, 8,000 `b = [c for c in a]` 71 s to 0.05 s, 8,000
  such comprehensions with distinct names more than 150 s to 0.05 s,
  10,000 `bI = a` more than 150 s to 0.03 s, and the 16,000-branch `elif`
  chain of the stress tests more than 150 s to 0.7 s. The release stress
  test `library_modules_take_linear_time_in_their_names` keeps twelve
  shapes of 10,000 statements under 1.5 s (all between 0.03 s and 0.4 s
  here) and linear where the result is (four times the statements, at
  most six times the time); `language_server_survives_deep_documents`
  passes again. Results on the private corpus are byte-identical.
- `importlib.import_module('__builtin__')` hands out the builtins like
  `__import__('__builtin__')`, and `.locals()` on it reads the caller's
  frame (review round 25, M2), but it only made fixes unsafe, so SCR102
  reported an import read that way as unused. A direct call is now
  judged like `__import__` (a literal plain module keeps the findings),
  and every other use of it, `imp`'s loaders, `sys.builtins`,
  `sys.getBuiltins()`, `sys.getDefaultBuiltins()`,
  `sys.modules_reloading`, `sys.__dict__` and the `sys` attribute of
  other modules (`re.sys`) are routes that read names, like
  `sys.modules`. Measured on 8.3.3's Jython.
- SCR124's safe fix changed what a script does after a string Jython
  drops (review round 25, B1): Jython's lexer drops a single-quoted
  string that a line break ends, with the line break, and a backslash in
  it escapes the whitespace after it, so `v4 = 'x  \ ` then `y' # c` is
  `v4 = y`; without the whitespace the backslash continues the string and
  `v4` is `'x  y'` (measured on 8.3.3's Jython). Such whitespace is now
  part of the string: SCR124 does not report it and SCR900's fix is
  unsafe. Every layout-only fix (SCR123, SCR124 and SCR900's
  trailing-whitespace fix) is safe only when every token of every script
  stays as it is, proven by lexing; otherwise it is unsafe with the
  reason (whitespace after a stray backslash, `print 1, \ `, or a comment
  removed after one). The safety net also compares the reads that a fix
  removes and the literals of each script. A differential test
  (`layout_oracle`, `oracle/run-oracle.sh --run`) ran 8,000 generated
  programs through these fixes and Jython: no behaviour change (714 of
  2,000 with the old code). SCR124's fix safety is now "depends". On the private
  corpus no finding or fix changed.
- A safe fix in a module that fails to compile on its size either way
  was reported safe by `check` and refused by `--fix` as making the file
  worse (review round 25, M1): the health check took SCR002's message
  with another byte count for a new error. Both now compare size errors
  without their numbers. The rule tests check that every fix they see
  reported safe is applied by `--fix`.
- A reflection or introspection route that is only a first hop no
  longer silences findings: any of the 145 measured reflective Java
  member names (`.type`, `.base`, `.fields`, `.module`, `getType()`,
  `getColumnType(i)`, `__class__`, ...), a computed `getattr`,
  `sys.exc_info()`, `import traceback` or a line number anywhere in a
  module used to make every scope read its locals, so SCR102, SCR103,
  REF002, REF003 and REF012 (and SCR104 and SCR118) reported nothing in
  that module. Routes are now of two kinds (`dynamic::Route`, table in
  `docs/internal/fix-safety.md`, "Frame routes: findings and fixes"):
  those that read or write a frame's or a module's names by themselves
  (`sys._getframe`, `.f_locals`, `.tb_frame`, `e = eval`,
  `import __builtin__`, `get = getattr`, ...) still silence findings;
  every route of either kind still makes the safe fixes that depend on
  what code reads unsafe (SCR102's reason names the route), keeps the
  module's code unknown for the closed world, keeps library symbols
  variables for every fix-side consumer, and the safety net drops a safe
  fix that adds or removes one. On the private corpus: SCR102 5 to 10,
  SCR103 39 to 55, REF002 75 to 76, REF012 21 to 24, every other rule and
  every existing fix unchanged; all 25 new findings checked by hand and
  true, and each of their fixes unsafe.
- A Java class or `type(x)` in a comparison other than `is` / `is not`,
  in `in`, as the operand of `%` on anything but a string literal, or as
  the first argument of `issubclass` is unknown code: these hand the
  class to special methods user code may define (`E() == AbstractMap`
  gave `E.__eq__` the class, which could `del o.SimpleEntry` for every
  later script; measured on 8.3.3 for `==`, `!=`, `%`, `in` and
  `__subclasscheck__`). No change on the private corpus.
- `docs/internal/fix-safety.md` ("The binding rule") said that a module
  object in a read-only place (a comparison, `print`, `str`, `%`) is
  allowed; the code allows read-only places only for Java classes and
  `type` and fails closed for module objects. The text now says so.
- The safe-fix net was quadratic when a script's compile status is not
  known: it rejected the safe fixes together and then checked each alone.
  An edit that leaves every token where it was (trailing whitespace, an
  edit in a comment) is proven neutral by lexing both versions and
  passes whatever the status; a script with a certain error (a lower
  size bound above a limit included) has a known status (it fails), and
  the net compares which limit fails rather than the numbers; failing
  fixes are found by halving groups (O(k log n) evaluations, at most
  `2n`) in `check`, `--fix` and the language server; and a safe fix that
  may change a token of a script whose status is not known is dropped
  without evaluating the net. Measured in release builds on one machine
  (1,400 statements padded with comment lines, every line ending in
  whitespace; `--select SCR102,SCR124`): 1,400 lines 1.7 s to 0.03 s,
  12,000 lines 18 s to 0.12 s (`--fix` 0.6 s), 16,000 lines 26 s to
  0.16 s (`--fix` 1.0 s), and 2,000 functions whose every safe fix fails
  from more than 150 s to 0.4 s. A release-only stress test
  (`unknown_compile_status_is_not_quadratic`) keeps these shapes under
  20 s.
- SCR120 reports lambdas in comprehensions that read a comprehension
  variable (`[lambda: k for k in range(3)]`, also generator expressions
  and set and dict comprehensions: Jython gives `[2, 2, 2]` for each) and
  closures in a loop that read an `except` target (`except ValueError,
  error: callbacks.append(lambda: error.args[0])` gives `[1, 1]`). It no
  longer reports a closure that runs in the iteration that creates it: a
  lambda called where it is written (`(lambda: u)()`) and a `def` or a
  name bound to a lambda whose every use is a call later in the same
  block (Jython: `[0, 1, 2]`).
- An import in a class body is unknown code at the import: `class K:
  import math` stores the module as the attribute `K.math`, which the
  vetting walker did not follow, so `del K.math.pi`,
  `K.sys.modules['math'] = None`, `K.os.system(...)`, `K.System.loadLibrary`
  and `K.os.open` / `K.os.read` of a file a fix edits (also through
  `K()`, `self`, `cls`, a parameter, an alias, a subclass or a nested
  class) kept SCR102's and SCR124's fixes safe (Jython printed `missing`
  before SCR102's fix and `7` after). The rule is general: an object the
  vetting tracks may be held only by a name whose every use the walker
  analyses; every other holder receives it as a value, checked where the
  value is computed. A star import in a class body counts too. The
  project index no longer takes a function, class or variable of a
  library module for plain when that module's code may store such an
  object where it is reachable (`del l.K.math.pi` from another module is
  reported at the change). `x.__class__`, `x.__objclass__`,
  `getattr(x, '__class__')` and the builtin `type` used as a value are
  handled like `type(x)`: unknown unless only read
  (`ArrayList().__class__.size = f` changes the Java class). Findings and
  fixes on the private corpus are unchanged.
- SCR120 reports closures that read a module global a module-level loop
  rebinds (`for value in xs: callbacks.append(lambda: value)` gives every
  callback the last value), and a global a function's loop rebinds under
  `global`; loops in methods were attributed to the module and missed. A
  loop in a class body is not reported (its names are class attributes,
  which closures do not read by name).
- SCR102 no longer removes an import safely when code anywhere in the
  workspace changes the shared object the import's evidence describes:
  after `import math; del math.pi`, removing an unused `from math import
  pi` turned an `ImportError` into success. Measured facts (a module's
  attributes, a Java class's members, the `system` API, the builtins)
  hold only while no code changes these objects, which every script of
  the gateway shares. So a store, deletion or augmented assignment into a
  module, a Java class, `system`/`fpmi`, `sys` or a function, class or
  constant of a vetted module or the builtins (`del math.pi`,
  `m.pi = 3` through `import math as m`, `del os.path.join`,
  `System.out = x`, `ArrayList.size = f`, `sys.stdout = f`, `len.x = 1`),
  `setattr`/`delattr` on one, `__setattr__`/`__delattr__`/`__dict__`
  named on one, a Java class or member class kept or passed as a value
  (`c = System`, `e = Map.Entry`), `type(x)` other than read (compared,
  printed, an attribute read) and a change through a library module's
  import or a library module handed out as a value (`del lib.math.pi`,
  `k = lib.math`, `f(lib)` when `lib` imports `math`) are unknown code,
  with the site named, for every fix that needs the closed world.
  `__builtin__` and `reload` were already not vetted. The member classes
  of the vetted Java classes are measured in the image
  (`JAVA_MEMBER_CLASSES`).
- BIND005 reports property references with nothing after the scope or
  the property type (`{view}`, `{this}`, `{session}`, `{view.custom}`,
  `{this.props}`, `{../Label.props}`, also as property binding paths):
  they match Perspective's reference grammar, so expressions read them as
  property references, but Ignition 8.3.3's gateway rejects them
  ("Invalid property identifier ..., no scope specified" or "no property
  specified") and the binding fails.
- A tag path whose property is empty or only dots (`{..}`, `T.`) names
  no property, as Ignition 8.3.3's `TagPathParser` reads it, so it is no
  longer taken for a read of tag configuration (which made `{..}` withhold
  every safe fix as a possible file read while `{this}` did not).
- NAME001 no longer offers a safe rename to a name that broken references
  already use: renaming `my_label` to `MyLabel` made
  `self.getSibling('MyLabel')`, a binding path `../MyLabel.props.text` or
  an expression `{/root/MyLabel.props.text}` start to resolve. The new
  name gets the guards of the old one (any occurrence as a word in a
  string of the view or a script's string literal, in any case, makes the
  rename unsafe and names the site; fixed values of Perspective's schema,
  such as component, binding and action types, do not count for either
  name), and the safety net compares the
  view's reference-resolution map (expression and binding references,
  script lookups, tag and view paths, components identified by their
  place in the view) before and after every safe fix to a `view.json`;
  any change drops the fix, which is reported unsafe with the reason
  (review round 21).
- SCR104 reports a redefinition when the only read of the first
  definition is in a top-level class body that runs after both
  definitions (`class A: f = f`), which reads only the second one.
- NAME001 offers no suggestion or fix for a name with a character whose
  case mapping changes length (`ﬁle_name` became `FIleName`), and says
  why.
- SCR112 checks format specs with a Latin-1 fill character
  (`u'{0:é>10s}'.format(0)` raises `ValueError: Unknown format code 's'`
  on Jython), in unicode and byte format strings; byte strings are read
  as Latin-1. Verified on 614,400 generated cases with Latin-1 fills
  (`oracle/format-spec.sh nonascii`), all matching Jython.

- Code iglint does not know no longer counts as vetted (review round 18,
  Opus, B1). A `system.*` member that the measured 8.3 stubs do not list
  (a third-party module's `system.perspective.runJavaScriptAsync`, a
  package that does not exist), an expression function the gateway does
  not register (`objectScript`, a simulation module's functions) and a
  Perspective component or navigate action that can run JavaScript in the
  browser (a component type that is not in the 8.3 list, inline frames,
  amCharts and time series charts, maps, PDF and report viewers, the menu
  components, SVG drawings with elements that are not plain shapes,
  Markdown that renders HTML, links and navigate actions whose URL may be
  a `javascript:` URL) are unknown code for the closed world and possible
  file reads, with the site named. So are stores into the scripting API
  (`system.tag.readBlocking = f`), its legacy alias `fpmi`, and
  `system.perspective.navigate` with a URL that is not a literal that runs
  no script. Before, `SCR102`, `VIEW015`, `NAME001` and `SCR124` kept
  their safe fixes.
- A safe fix near Jython's size limits is no longer applied (review round
  18, Opus, B2): `SCR002`'s bounds are lower bounds, so the safety net
  could not see a fix turn a function Jython rejects ("Module or method
  too large") into one it compiles. Every code unit, the class's
  `<init>` and `call_function` and the constant pool now also have a
  sound upper bound, and a fix of a script whose bounds lie on both sides
  of a limit is unsafe, with the bounds in the reason. The upper bounds
  were calibrated on the oracle with 5,794 generated programs (3,000
  random function bodies near the method limit, and programs near the
  `<init>` and constant pool limits): no method or pool exceeds its
  bound, method bounds are at least a quarter above the method's size
  (15,659 bytes above any unit of more than 60,000 bytes) and pool bounds
  at least 5% above the count.
- `SCR002` reports modules with too many functions, classes, lambdas and
  generator expressions for Jython's class initializer (`<init>` builds a
  code object per body and is one Java method too): Jython fails from
  about 765 functions of eight parameters, `SCR002` reports these from
  840, with 0 false reports on 300 generated modules near the limit.
- The upper bounds of Jython's code size follow its code generator
  (review round 19, Opus, blocker): the round 18 bound charged a closure's
  free variables once per unit, while Jython builds the cells at every
  creation, so it undercounted functions full of closures by up to 4.6
  times, and `SCR102` removed an import from a function Jython rejects
  and compiles without it. Every Java method's bound now mirrors Jython
  2.7.3's `CodeCompiler` (the image's Jython emits the same code)
  instruction by instruction: cells per creation and free variable, also
  through the units in between, every copy of a `finally` body and of the
  units in it, the locals and stack a `yield` saves, wide locals and
  jumps, and the class's `<init>` and `call_function` per compiled unit.
  The derivation is a table in `docs/internal/fix-safety.md` ("Size
  limits"). Calibrated on 10,000 generated programs with a family of
  adversarial shapes per term: no method or constant pool above its
  bound, median margin 7%. The review's repro now gets "compile status is
  not known".
- Long names no longer escape the upper bounds of Jython's code size
  (review round 20, Codex, blocker): Jython loads a name of more than
  16,000 characters in chunks, and the bounds charged one constant per
  name, so a function of 1,683 reads of an attribute of 16,001 characters
  seemed to fit, and `SCR102` blanked an import Jython needs removed to
  compile it. Every name, attribute, keyword, imported name, class name
  and the names of every code object are now charged by their length (as
  Jython mangles them in a class), checked on the oracle with a family of
  long names in every position (smallest margin 7%). Independently, a
  script with an identifier of more than 1,000 characters has an unknown
  compile status ("identifier of N characters: Jython's code size cannot
  be bounded precisely"), so no fix of it is safe; the private project's
  longest identifier has 79 characters.
- `SCR002` reports a function or class whose name makes the name of its
  Java method longer than a class file allows (65,535 bytes,
  `UTF8 string too large`; a function of the module fails from 65,534
  characters), measured on the oracle. The documentation said that
  identifiers have no length limit: names in every other position compile
  at 1,000,000 characters.
- `SCR002` reports two more crashes of Jython's code generator (review
  round 20, Opus): a `yield` in the exception type of an `except` clause
  (`except (yield):`, `NegativeArraySizeException`) and a `yield` in an
  attribute or subscript target inside an unpacking target (`b, a[(yield)]
  = 1, 2`, also in `for`, `with` and `except` targets,
  `IllegalArgumentException`). Single targets, augmented assignments,
  `del` and unreachable copies compile and are not reported; the cross
  product of `oracle/compile_cross.py` (13,202 cases) has no difference
  from Jython.
- `oracle/jython_method_size_oracle.py` no longer aborts on a compile
  error message with non-ASCII text (review round 20, Opus), and records
  method names of more than 64 characters shortened.
- Size bounds saturate instead of overflowing (review round 19, Codex):
  nested `try`/`finally` chains with a `return` in each `finally` made the
  bound wrap in release builds (the `SCR124` fix of a 64-level chain was
  safe) and panic in debug builds. Such a script's compile status is now
  not known and no fix of it is safe.
- `SCR002`'s lower bound no longer exceeds Jython's size for simple
  slices, stores without a temporary and unpacking, `del` of a tuple, an
  `if`, `with` or `try` whose body ends, `exec` with both parts,
  comprehensions with several generators, `from` imports and the class
  initializer of small modules (found by the new calibration programs;
  `SCR002` never reported a script Jython compiles from these). The
  class initializer's bound counts each code object per instruction:
  `SCR002` reports modules of functions of eight parameters from 900
  (was 840; Jython fails from about 765).
- `BIND002` no longer reports `property` and `isAuthorized` in
  Perspective bindings, transforms and the other expressions of a view,
  which can call them (measured on an 8.3.3 gateway: Perspective's own
  function factory adds them, and replaces `runScript`), and still
  reports them in tag expressions and alarm and tag property bindings,
  where an 8.3.3 gateway rejects them. `BIND003` checks their argument
  counts. A call of `property` still keeps other rules' fixes from being
  safe (it reads properties by a path computed at run time) unless the
  project vets it. The generated lists come from `stubs-gen` (new:
  `expression_contexts.py`, `introspect/perspective_expression.py`).
- `check`, the language server's diagnostics and its quick fixes report a
  safe fix that the post-fix safety net would drop as unsafe, with the
  net's reason (review round 18, Opus, N1). Before, it was listed as safe
  and `--fix` said "fix not applied".

- No more panics (exit 101) on text with multi-byte characters where a
  byte index was assumed to be a character boundary (review round 18,
  Codex). A script path literal whose component starts with such a
  character (`os.path.exists(u"/é/report.txt")`) aborted `check` and
  the language server's initialization; so did a view property key ending
  in multi-byte characters, a string literal with a multi-byte character
  across its seventh byte (SEC004), and a configured NAME001 abbreviation
  whose uppercase form is shorter (`ı`, `ſ`), which could also split
  the name at the wrong place. Lexer error columns, pylint positions and
  source map offsets inside a character now snap to its start. Clippy's
  `string_slice` lint is denied in the whole workspace: every byte slice
  of a `str` either uses a checked API or says why its indices are
  boundaries. `TextRange::slice` clamps a range that splits a character
  or leaves the text instead of panicking (`TextRange::get` returns
  `None`), and `apply_edits` rejects such an edit as an error. A
  generated-workspace test puts random Unicode in every name and string
  position and runs every command and the language server over it.
- A linked Git work tree (`git worktree add`, `.git` is a file that points
  at `<git dir>/worktrees/<name>`) is a checkout of its own, wherever it
  lies (review round 17, Opus): a file of `.worktrees/x` or
  `.claude/worktrees/x` was linted against the main checkout's workspace,
  whose walk skips hidden directories, so the file-read gate and the
  closed world came from the main checkout's code (a feature branch's
  `open(p).read()` did not make SCR123 and SCR124 fixes unsafe), REF009
  said the walk did not reach `project.json` and `resource.json`, real
  REF002 and REF012 findings were missed and baseline paths started with
  `.worktrees/x`. The search for the workspace root stops at a linked work
  tree; the main checkout's runs (and the language server's folder) no
  longer enter one, hidden or not. Submodules and repositories made with
  `--separate-git-dir` still belong to the enclosing work tree.
- A workspace root candidate counts only when its walk reaches the linted
  file's project or collection (no hidden or otherwise skipped directory
  on the way), otherwise the nearest one that does is the root: a project
  below `node_modules/pkg/projects/` is linted in `node_modules/pkg`. A
  file whose project the walk did not reach anyway (a language server
  folder) gets no fixes, and REF009 says that iglint lints it on its own,
  without saying that the gateway does not load it. In the language
  server such a file gets a folder of its own.
- Rules that point at the first character of a file (REF011, REF013,
  STYLE002, REF014) no longer panic (exit 101, no output) when the file
  starts with a character longer than a byte, such as a UTF-8 byte order
  mark (review round 17, Opus). Every finding's range is widened to whole
  characters in one place, line and column conversions accept any offset,
  and a fix with an edit inside a character is dropped; debug builds assert
  that rules compute ranges on character boundaries.
- SCR307 no longer reports a handler binding that code the rule does not
  follow may replace (review round 17, Codex): a `global` declaration of
  the handler name in a class body or function (also when a later call
  assigns it), a module-level list comprehension whose variable is the
  name (Python 2 leaks it), `exec`, `import *` or writes through
  `globals()` in any scope, or a route to a frame. Bindings that stay in
  their scope (locals, class attributes, generator variables) still do
  not hide a finding. Verified on the Jython of Ignition 8.3.3.
- The workspace root no longer depends on what is linted (review round
  15): it is the outermost of the configuration directory, every Ignition
  layout root around the path (up to the Git work tree) and the Git work
  tree when a project or collection lies anywhere below it. Linting a tag
  file in `gateway/data/config/resources/` next to `projects/`, or
  `src/projects/A` next to `gateway/config/resources/`, now sees the
  projects too, so its fixes are as safe as in a run over the repository.
- Projects and collections named `target`, `node_modules` or
  `__pycache__` are discovered, linted and part of the closed world;
  before they were skipped (review round 15).
- Hidden resource directories and folders inside a project or collection
  (`script-python/.hidden/`, `views/.folder/V`) are linted and part of the
  closed world: an 8.3.3 gateway loads them. Before they were skipped.
- A workspace directory that cannot be read (permissions, a broken
  symbolic link, a symbolic link to a directory) makes the closed world
  unknown: safe fixes
  become unsafe with a reason that names the directory. REF009 says when
  a resource file lies in a project the workspace walk did not reach,
  instead of calling it outside every project.
- SCR307, SCR105, SCR112 and the library constants of the closed world
  treat `True`, `False` and `None` as constants only where they read
  Jython's builtins: all three are names a module can rebind (`True =
  lambda: 7`, `import x as None`; verified on Ignition 8.3.3), so `True =
  lambda: 7` followed by `handleTimerEvent = True` is no longer an SCR307
  error (review round 15, Codex).
- The "nothing to lint" warning shows the working directory as `.`
  instead of an empty path.
- Symbolic links to directories are no longer followed, like Ignition
  8.3.3, which does not follow them (review round 16, verified on a
  throwaway gateway: a linked project never starts, a linked `core`
  collection keeps the gateway from starting, a linked library folder,
  `script-python` or `ignition` directory gives `ImportError`, a linked
  tag folder has no tags). A link inside a project or collection, a
  project or collection directory that is a link, and a link to a
  directory that holds projects count as a directory that was not read:
  every safe fix is unsafe, with the link named. The files behind such a
  link are linted on their own, reported by REF009 ("behind ..., a
  symbolic link to a directory") and get no fixes, so `--fix` no longer
  edits files where a link points, also outside the repository.
- A safe fix of a file that is a symbolic link, the target of a symbolic
  link of the workspace, or a file with hard links is unsafe, with the
  other paths named (review round 16): the gateway reads files through
  links, so the fix changed every resource that reaches the file
  (`SCR102` removed an import that another project used through its
  link). Every file is known by its identity (device and inode).
  Symbolic links to manifests (`project.json`, `resource.json`,
  `config-mode.json`) and data files are read, as the gateway reads them.
- `--fix` keeps symbolic links and hard links: a fix of a link is written
  to the file it points to, and a file with hard links is rewritten in
  place. Before, the link was replaced by a regular file (review round 16).
- The workspace root of a file in a nested Git work tree (a submodule, or
  a repository made inside another one) is the outermost work tree that
  holds a project or collection, not the nearest `.git`, so linting one
  file of the nested repository sees the gateway configuration of the
  enclosing one (review round 16).
- Without a configuration file, baseline paths and fingerprints are
  relative to the workspace root instead of the working directory, so an
  explicit baseline matches, and is pruned, the same from every
  subdirectory (review round 16, Codex). Entries written by earlier
  builds from another directory still match, and are rewritten relative
  to the workspace root by `--update-baseline`; the default
  `.iglint-baseline.json` is looked up at the workspace root first.
  `--show-settings` prints the directory (`Baseline paths relative to:`).
- SCR307 no longer reports `handleTimerEvent = a == b` (or `<`, `in`,
  `is`) and backquotes as values that cannot be called: a rich comparison
  may return a function (`__eq__` returning a lambda is called by Jython).
  Only literals of non-callable types and the builtin constants count
  (review round 16, Codex).
- `bench/parity.py` no longer lets ign-lint write its debug files into
  the corpus or the source tree when the configuration has no debug
  settings (ign-lint picks `tests/debug` or `.ignition-lint/debug` by
  itself): `PylintScriptRule` always gets `debug` off and a `debug_dir`
  in `--out` (review round 16, Codex). Pylint findings are set aside
  before matching instead of being counted as identical when their text
  matched; the summary gives their number per side under `not compared`
  (the private project's parity row is now 1,463 identical and 915
  expected, 0 unexplained).
- `SCR112` missed formatting that certainly fails on Unicode (review round
  14, Codex): a unicode format spec with a character beyond U+00FF
  (`u'{0:\U0001f980^4}'.format(1)`: `ValueError`), a non-ASCII unicode
  literal made a byte string by a byte string format or `!s`
  (`UnicodeEncodeError`), a non-ASCII byte string inserted with `%s` into
  a unicode result (`u'%s' % '\xff'`, also after a unicode value:
  `UnicodeDecodeError`), and `%c` of a number from 256 on into a byte
  string (`OverflowError`). Checked against Jython on 16,947 generated
  cases (`oracle/format-unicode.sh`): no finding where Jython formats,
  every such failure with literal values reported.
- With lone CR line ends, findings were shown a line further down than
  Jython reports them (review round 14): Jython numbers lines by `\n`
  only, and a lone CR is a character of the line. Every output format
  (text, concise, JSON, SARIF and the others) now shows Jython's line and
  the column of the same character, checked against new parser oracle
  cases (`crates/iglint_python/tests/oracle/line_endings.*`). The
  language server keeps the protocol's lines (a lone CR ends a line
  there), so the editor highlights the same text.
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
