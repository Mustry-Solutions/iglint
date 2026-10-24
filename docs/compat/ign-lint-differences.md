# Intended differences from ign-lint 0.6.4

iglint reproduces every finding of ign-lint's view rules, with ign-lint's
node paths and wording in `iglint ign-lint` mode, except where ign-lint is
wrong. This page lists those places. Each one is pinned:

* the parity test `crates/iglint/tests/ign_lint_parity.rs` compares iglint
  with the recorded ign-lint output of every fixture view and every
  configuration variant (`crates/iglint_linter/tests/fixtures/ign-lint/`);
* every accepted mismatch is an entry in
  `crates/iglint_linter/tests/fixtures/ign-lint/differences/*.json` with its
  reason, and an entry that no longer matches fails the test;
* iglint's own behaviour in those cases is asserted by the rule's unit tests.

Bug numbers refer to the bug list in [ign-lint.md](ign-lint.md#13-bug-list).
Counts are entries in the difference files (one per view, variant and
finding).

## Not compared

* `PylintScriptRule`: see [its section](#pylintscriptrule-scr900-and-the-native-scr-rules).
  Its findings depend on the installed pylint, and iglint lints each script
  as Ignition compiles it instead of ign-lint's combined file, so lines,
  wrappers and the set of messages differ by design. The parity test leaves
  its findings out on both sides and does not run `SCR900`.

## Whole run

* An invalid JSON file is reported (`VIEW001`) and the other files are still
  linted; ign-lint aborts (bug 25).
* Deeply nested views are linted. ign-lint flattens a view recursively and
  aborts the whole run with a `RecursionError` traceback (exit 1) from
  about 500 levels of JSON nesting (250 nested components); from about
  10,000 levels already Python's `json` module fails the same way.
  `iglint ign-lint` lints views of any depth up to iglint's limit of 8,192
  levels (`VIEW001`), and treats deeper files like invalid JSON, as
  ign-lint treats every file it cannot read: the run stops with exit
  code 1. Before, `iglint ign-lint` rejected views nested more than 128
  levels as invalid JSON.
* Fixes edit the original text in place; ign-lint re-serialises the whole
  file and drops the final newline (bug 31).
* View-level event scripts and component extension functions are scripts
  like any other; ign-lint never models them (bug 1). Because of that,
  `VIEW018` reports two traversal calls in extension functions that ign-lint
  does not see (2 entries).

## NamePatternRule (`NAME001`)

* **Abbreviations are whole words** (bug 6). ign-lint replaces abbreviation
  substrings before matching, so `idleTimer`, `restartButton`, `clientPanel`
  or `urlParams` fail camelCase while names containing `ID`, `AI`, `IP`, ...
  pass SCREAMING_SNAKE_CASE and lower case. iglint accepts the first group
  and reports the second (78 extra, 8 missing).
* **Suggestions** split words consistently, keep digits and keep
  abbreviation runs instead of splitting capitals per letter and dropping
  digits (bug 8). The
  findings are the same; only the suggestion text differs, which counts as
  one missing and one extra message (71 missing, 72 extra).
* No "already in use by a sibling" message when the suggestion equals the
  component's own name (bug 7).
* Keys of `props` that the component's schema declares (for example SVG
  `elements` attributes) are Ignition's names, not the user's, and are not
  checked (6 missing). Keys whose value is an object or array are checked
  too, not only leaf keys (7 extra).
* **Fixes:** a rename is offered only when the suggested name passes every
  check of the rule; ign-lint offers renames to names it would report again
  (36 entries). References to the component are found by resolving them
  (`{../Name.props.x}`, `/root/A/B`, property binding paths, indirect tag
  references, literal `getChild`/`getSibling` arguments in scripts), not by
  matching the name anywhere in the view; when nothing actually refers to the
  component the rename is safe even if another component of the same name is
  referenced. The rename is unsafe (ign-lint: safe) when a script of the
  view hands a component to code outside the view, for example to a library
  function, which may look the component up by name (2 entries, one per
  view, for every variant).
  The rename is also unsafe (ign-lint: safe) when the suggested name
  already occurs as a word, in any case, in a string of the view that is
  not a fixed value of Perspective's schema, where a broken reference or
  lookup could start to find the renamed component. Fixed values (known
  component types such as `ia.input.button`, measured binding and
  transform types such as `query` and `expr-struct` and the keys leading
  to their measured fields such as `struct`, known action types and
  scopes, `meta` keys other than `name`) do not count, so the fixtures
  need no entry for this.
* Outside a project (a lone view, as in the fixtures), the rename is also
  unsafe when a script of the view calls a name that may be a project
  library function (`lib.util.f()`, an undefined name): iglint cannot
  check that it is not a frame builtin such as `eval` under another name,
  which could look the component up (6 entries, one per view).
* Unknown kwargs are reported as configuration notices instead of being
  ignored silently (bug 9).
* Keys of Ignition's dataset encoding (`$`, `$columns`) are Ignition's
  names, not the user's, and are not checked.
* A component custom property that exists only in the component's
  `propConfig` (a bound property without a stored value) is a property
  like any other and is checked. ign-lint only sees component properties
  that have a stored value (it adds `propConfig`-only properties at view
  level only).

## PollingIntervalRule (`BIND004`)

* Expressions are parsed: `snow(5)` is another function and `now()` inside
  a string literal is text (bug 10; 2 missing). A non-numeric argument is
  checked even when a numeric `now(n)` is in the same expression.
* `now()` without an argument polls every 1000 ms; it is reported only when
  that is below `minimum-interval` (7 missing under a 1000 ms minimum).
* `now(0)` and negative rates mean no polling (1 missing).
* A rate that is not a literal cannot be checked; it is reported only with
  `strict` (1 missing).

## ComponentReferenceValidationRule (`BIND005`, `SCR305`)

The script part is `SCR305` (`validate_scripts` maps to its
`validate-scripts` option):

* A missing sibling is reported once; ign-lint reports it twice, as
  "Component 'X' not found as sibling in: ..." and "Script references
  non-existent sibling component 'X'" (1 missing per view).
* Navigation above the root container (`self.parent.parent...` from a
  child of the root) is not reported: what it returns at run time is not
  modelled (1 missing).
* Lookups are resolved on the parsed script through aliases
  (`card = self.getSibling('Card')`), and extension functions are checked
  (bug 1; 1 extra).

The expression and property binding part:

* References are resolved from the binding's own component; view-level
  bindings resolve from the view, not from a fake `root` component (bug 20).
* `BIND005` also checks that referenced `custom` properties and view
  `params` exist, which ign-lint does not (1 extra).
* `BIND005` checks the references in every expression of a binding:
  query parameters, tag history and HTTP binding expressions and map
  transform expressions too, not only expression bindings, structure
  leaves and expression transforms. In `iglint ign-lint` these are
  reported at ign-lint style paths (`{binding}.{parameter}` for a query
  parameter).

## BadComponentReferenceRule (`VIEW018`)

* Paths and scripts are parsed: `../` is not also counted as `./`, and
  comments and string literals never match (bug 18; 17 entries each way for
  the changed message, 2 missing for Python code pasted into an expression
  binding, which is `BIND001`'s finding).

## UnusedCustomPropertiesRule (`VIEW015`)

* References are matched exactly, not as substrings: `{view.custom.foobar}`
  does not use `foo` (bug 21), and `{this.custom.beta}` uses the binding
  component's `beta` only (bug 21). Same-named components are separate
  definitions (bug 22). Component `propConfig` metadata is not a definition
  (bug 23).
* A property with a change script is used: the script runs on every change
  (2 missing).
* `output` parameters are written by the view and read by its caller; they
  are `VIEW019`'s concern (2 missing).
* Assignments in scripts (`self.view.custom.x = ...`,
  `self.getChild('A').custom.x = ...`) write the property; they do not
  read it. ign-lint counts any text that names the property as a use.
* A script that uses a property container in a way that cannot be
  followed (`self.view.params.get(...)`, a subscript, `self` or the view
  passed to a function, an alias) counts as reading every property of it,
  so the rule stays silent instead of guessing. ign-lint reports the
  properties that the text never names.
* Outside a project (a lone view, as in the fixtures), a script that
  calls a name that may be a project library function (`lib.util.f()`,
  an undefined name) makes the fix unsafe: iglint cannot check that the
  name is not a frame builtin such as `eval` under another name, which
  could read any property. ign-lint offers a safe fix.
* A view whose scripts call a project library function do not report any
  of its `custom` or `params` as unused when some library module evaluates
  computed strings or reaches a frame: such a module rebinds its names, so
  the called function is no longer a plain function and iglint treats the
  call as reading every property it could reach (the deliberate precision
  cost of the frame and introspection analysis). ign-lint reports the
  properties its text never names. The parity harness classes these as
  expected (`VIEW015: a script calls a library function ...`).

## ExcessiveContextDataRule (`VIEW006`)

* The breadth of the top-level `custom` object is measured, arrays nested in
  arrays are measured, and arrays of empty objects count with their real
  length (bug 24; 1 extra).

## PylintScriptRule (`SCR900` and the native `SCR` rules)

`PylintScriptRule` maps to `SCR900`, which runs the real pylint, and to every
default rule of the `SCR` category that no other ign-lint rule names
(iglint's native script checks; rules added later join automatically).

* **Configuration.** `pylintrc` and `category_mapping` configure `SCR900`
  (`pylintrc` is relative to the project root, which is the current
  directory in `iglint ign-lint`); `severity` is `SCR900`'s severity for
  categories the mapping leaves out. `debug`, `batch_mode` and `debug_dir`
  are reported as ignored: iglint always lints everything in one pylint
  run and writes no debug files (bug 16). The native rules take no kwargs
  and keep their own defaults, except that `category_mapping` sets their
  severity like that of their pylint counterparts: error-level rules take
  the severity mapped for `E`, warning-level rules the one for `W`. A
  mapping value other than `error`, `warning` or `info` is a configuration
  error instead of an error counted as a warning (bug 17).
* **Disabling.** `"PylintScriptRule": {"enabled": false}` disables
  `SCR900` and the native rules: ign-lint then did not check scripts at
  all, and `iglint check` with a legacy configuration keeps that.
  `iglint migrate` writes `enabled = false` for each of them so they can be
  turned back on one by one.
* **Scope.** ign-lint runs pylint on the scripts of `view.json` files only.
  `iglint migrate` and `iglint check` with a `rule_config.json` keep that:
  `SCR900` (and every other rule ign-lint adds to iglint's defaults) is
  selected for `**/view.json` only. The native rules also lint library
  modules, gateway events and WebDev scripts.
* **One pylint process per run** over all scripts of all linted files, with
  one worker per CPU by default (`jobs`); ign-lint runs pylint once per view.
* **One module per script.** Each script is written to its own file, as
  Ignition compiles it (correct wrappers such as `def transform(self, value,
  quality, timestamp):`, bug 13). Python 2 syntax in one script no longer
  hides the findings of the other scripts of the view (bug 11), and
  module-wide checks such as `inconsistent-quotes` do not mix scripts (bug
  14). The bundled configuration is ign-lint's `.ignition-pylintrc` with the
  four messages ign-lint's combined-file header disabled added to `disable`.
* **Locations.** Findings point at the exact range in the file, also inside
  JSON-escaped scripts; `iglint ign-lint` prints `Line {n}` with the real
  body line instead of the line after it (bug 12). Messages about the
  generated header (naming, docstrings, unused or too many arguments) are
  not reported because Ignition writes the header; other header messages
  are reported at the script's property. Custom method headers are the
  user's and keep all messages.
* **Messages that are wrong for Ignition are dropped:** `undefined-variable`
  and `import-error` for `system`, `fpmi`, `sys`, the project's library
  packages and Jython builtins such as `xrange` (bug list, 5.3), and
  `syntax-error` for scripts Jython parses (Python 2 syntax). Real syntax
  errors are `SCR001`'s; pylint's is shown only when `SCR001` is off.
* **Data-quality findings** (script not indented, mixed tabs and spaces)
  are not reproduced as such: an unindented body is a syntax error
  (`SCR001`), and indentation style is pylint's `bad-indentation`.
* **Fixes.** `SCR900` has ign-lint's only pylint fix (6.4): when pylint
  reports `trailing-whitespace` (C0303, disabled by the bundled
  configuration), every line of the script is `rstrip()`ed, as one fix per
  script with ign-lint's description. It edits only the removed bytes and
  keeps the JSON escapes; it is unsafe (ign-lint: safe) when whitespace
  inside a multi-line string would go or the script does not parse. The
  native rules' fixes apply with `--fix` like those of any other
  equivalent, and `allow_fix: false` makes all of them unfixable.
* **Duplicates.** A pylint message that a native rule also reports (same
  range and name; `undefined-variable` and `SCR101`, `unused-variable` and
  `SCR103`, ...) is shown once, as the native finding.
* **Output.** pylint messages are grouped by category (`Pylint - Error
  (E):` ...) like ign-lint's; within a category they are in file order
  instead of pylint's checker order, and the native rules' findings are
  the rule's plain `•` lines before the groups.
* **pylint missing or failing** prints one warning for the run (`iglint
  check`: `warning: SCR900: ...`; `iglint ign-lint`: a `⚠️` line) with how
  to install or configure it; every other rule still runs and the exit
  status is not affected by it.

## PropertyAccessRule and PropertyPersistenceRule (`VIEW012`, `VIEW013`)

No differences in findings. Fixes are minimal text edits instead of a
re-serialised file. They are unsafe where ign-lint applies them by
default: changing an access mode changes who can read and write the
property at run time, and removing the stored value of a bound property
changes what scripts and expressions see before the binding delivers (the
`VIEW013` fix stays safe when there is no stored value).
