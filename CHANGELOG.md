# Changelog

All notable changes to iglint are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and versions follow
[Semantic Versioning](https://semver.org/). Rule codes and names are stable
from 0.1.0 on.

## [Unreleased]

### Changed

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

- Frame and introspection detection is now a closed argument instead of a
  list of patterns (`docs/internal/fix-safety.md`, "Threat model"). Jython
  code can observe a caller's locals, a module's namespace, a function's
  identity or a source line number in only three ways, and all are
  detected, failing closed: an introspection attribute named in source or
  in any string literal (`getattr(tb, 'tb_frame')`,
  `attrgetter('tb_frame.f_back.f_locals')`, `co_firstlineno`,
  `.tb_lineno`); a reflective accessor with a computed name
  (`getattr(o, n)`, `setattr`, `attrgetter(n)`, `__getattribute__(n)`);
  and the interpreter and reflection APIs (`compile`, `sys.exc_info`, the
  `traceback` module, any import from `org.python.*`, `java.lang.reflect`,
  `java.lang.Class`, `java.lang.invoke`, `javax.script`, `.getClass()`).
  A library function or module that takes any route is no longer treated
  as plain, so VIEW015, NAME001 and SCR102 no longer offer a safe fix
  (nor, for VIEW015, a finding) for data a frame-reading library function
  reaches, and SCR123 no longer deletes a line in a script that reads a
  line number (`tb_lineno`, `traceback.format_exc`).
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
