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

## Not yet compared

* `PylintScriptRule`: iglint's equivalents are the native `SCR` rules and
  `SCR900`, which are built in the script layer. Its findings are left out of
  the comparison until those rules are registered.
* `ComponentReferenceValidationRule`, script part (`self.getSibling('X')` and
  navigation chains): iglint's equivalent is `SCR305`. The expression and
  property binding part is `BIND005` and is compared.

## Whole run

* An invalid JSON file is reported (`VIEW001`) and the other files are still
  linted; ign-lint aborts (bug 25).
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
  referenced (4 entries).
* Unknown kwargs are reported as configuration notices instead of being
  ignored silently (bug 9).

## PollingIntervalRule (`BIND004`)

* Expressions are parsed: `snow(5)` is another function and `now()` inside
  a string literal is text (bug 10; 2 missing). A non-numeric argument is
  checked even when a numeric `now(n)` is in the same expression.
* `now()` without an argument polls every 1000 ms; it is reported only when
  that is below `minimum-interval` (6 missing under a 1000 ms minimum).
* `now(0)` and negative rates mean no polling (1 missing).
* A rate that is not a literal cannot be checked; it is reported only with
  `strict` (1 missing).
* Indirect tag binding `references` are property paths or literals, not
  expressions (1 missing).

## ComponentReferenceValidationRule (`BIND005`)

* References are resolved from the binding's own component; view-level
  bindings resolve from the view, not from a fake `root` component (bug 20).
* `BIND005` also checks that referenced `custom` properties and view
  `params` exist, which ign-lint does not (1 extra).

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

## ExcessiveContextDataRule (`VIEW006`)

* The breadth of the top-level `custom` object is measured, arrays nested in
  arrays are measured, and arrays of empty objects count with their real
  length (bug 24; 1 extra).

## PropertyAccessRule and PropertyPersistenceRule (`VIEW012`, `VIEW013`)

No differences in findings. Fixes are minimal text edits instead of a
re-serialised file.
