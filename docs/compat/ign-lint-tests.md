# ign-lint test suite and parity fixtures

Companion to [ign-lint.md](ign-lint.md). It records:

1. how the parity fixtures in `crates/iglint_linter/tests/fixtures/ign-lint/`
   were produced (exact commands), their file formats and how to compare
   against them;
2. a catalogue of the upstream ign-lint v0.6.4 test suite (`tests/`), so the
   behaviours they cover can be ported.

## 1. Parity fixtures

### 1.1 Layout

```text
crates/iglint_linter/tests/fixtures/ign-lint/
  LICENSE                     upstream MIT notice, with attribution
  variants.json               named rule configurations (see 1.4)
  generate_expected.py        regenerates every expected*.json
  cli-default.stdout.txt      real CLI stdout, default config, all views
  cli-default.results.txt     real CLI --results-output, same run
  cases/<Name>/view.json      copied unchanged from upstream tests/cases (18 views)
  cases/<Name>/resource.json  copied unchanged
  cases/<Name>/expected.json            findings, default config
  cases/<Name>/expected-variants.json   findings and fixes per variant
  probes/<Name>/view.json     original iglint views pinning quirks and bugs (6 views)
  probes/<Name>/expected*.json
```

The upstream `thumbnail.png` files and the golden files under upstream
`tests/debug/cases/` (flattened map, model, statistics per case) were not
copied; the goldens can be regenerated with `ign-lint --debug-output`.

Probe views (original, not covered by the upstream licence):

| probe | pins |
|---|---|
| NamingAbbreviations | abbreviation false positives/negatives, self-collision suggestion text, suggestion splitting (bug list 6 to 8) |
| PollingQuirks | `now()` detection on every binding kind, substring and mixed-call quirks (bug 10) |
| ScriptLines | wrapper headers, `Line n+1` mapping, transform `quality`, onChange, message handler, custom method, indentation checks, bogus propConfig definitions (bugs 3, 12, 13, 23) |
| ScriptPy2Syntax | one Python 2 statement hides all other pylint findings (bug 11) |
| UnusedQuirks | prefix crediting, cross-component `this.custom`, same-named components, index-stripped paths, params never fixed, ambiguous component fix suppression (bugs 4, 21, 22) |
| ReferenceQuirks | relative, absolute and script references, duplicate getSibling finding, view-level source, `../` plus `./` double match, cross-script inconsistent-quotes (bugs 14, 18 to 20) |

### 1.2 Environment

```sh
uv venv /tmp/ignlint-venv-064 -p 3.13
uv pip install -p /tmp/ignlint-venv-064/bin/python ign-lint==0.6.4
# resolved: ign-lint 0.6.4, pylint 4.1.3, astroid 4.3.4, CPython 3.13.16
```

The installed package was checked to be byte-identical to tag `v0.6.4`
(`dace01d`). PylintScriptRule findings depend on the pylint version; rerun the
generator with the versions above.

### 1.3 Commands

All runs use a scratch working directory with no `.config/.ignition-pylintrc`
and no `tests` directory above it, so PylintScriptRule uses the rcfile bundled
with ign-lint and writes its debug copies to `./.ignition-lint/debug`:

```sh
mkdir -p /tmp/ignlint-run && cd /tmp/ignlint-run
FIX=<repo>/crates/iglint_linter/tests/fixtures/ign-lint

# expected.json and expected-variants.json for every view under cases/ and probes/
/tmp/ignlint-venv-064/bin/python -I "$FIX/generate_expected.py" "$FIX"
```

`generate_expected.py` runs ign-lint in-process through the same calls the
CLI makes for one file in `--fix-dry-run` mode (`create_rules_from_config`,
`read_json_file`, `flatten_json`, `LintEngine.get_model_statistics`,
`LintEngine.process` with a `PathTranslator`), once per view and variant, with
`source_file_path` set to the view path relative to the fixture directory.

Cross-check against the real CLI (default config, every view): for each view,

```sh
echo '{}' > empty.json
ign-lint --config empty.json --results-output out.txt "$FIX/cases/<Name>/view.json"
```

and the per (rule, severity) counts parsed from `out.txt` were compared with
`expected.json`: all 24 views matched, every run exited 1.

CLI output goldens, from a copy of the fixture directory (to keep relative
paths short):

```sh
cd <copy of fixture dir>
echo '{}' > empty.json
ign-lint --config empty.json --results-output cli-default.results.txt \
  cases/*/view.json probes/*/view.json > cli-default.stdout.txt
# exit 1; Files processed: 24, Total warnings: 135, Total errors: 165
```

followed by replacing temp file names: in stdout the debug path
`<cwd>/.ignition-lint/debug/HHMMSS_pidPID_xxxx.py` became
`<debug-dir>/<tmpfile>.py`, and in both files every `HHMMSS_pidPID_xxxx`
module name inside E0001 messages became `<tmpmodule>`. The shell glob order
(C locale sort) is the processing order.

### 1.4 Variants

`variants.json` maps a variant name to a complete `rule_config.json`. The
`default` variant is `{}`: all nine rules with default kwargs
(PropertyAccessRule and PropertyPersistenceRule are inert). Every other
variant enables exactly one rule (the generator sets `"enabled": false` on all
others). Kwargs are taken verbatim from upstream tests where one exists:

| variant | rule | source |
|---|---|---|
| np-pascal-component-error, np-camel-component-error, np-snake-component-error, np-kebab-component | NamePatternRule | test_component_naming fixture tests |
| np-screaming-component, np-title-component, np-lower-component | NamePatternRule | iglint (conventions upstream never tests) |
| np-convention-only | NamePatternRule | test_cli_integration |
| np-target-string | NamePatternRule | test_discovery (string `target_node_types`) |
| np-property-camel | NamePatternRule | test_component_naming |
| np-component-custom-method-camel-min3 | NamePatternRule | test_component_naming |
| np-standard-component-property | NamePatternRule | test_component_naming (`skip_names` id, x, y, z) |
| np-severity-split | NamePatternRule | test_component_naming (per node type severity) |
| np-four-types | NamePatternRule | test_component_naming::test_all_node_types_with_different_severities |
| np-pascal-min5, np-pascal-max10, np-pascal-no-numbers, np-pascal-forbidden, np-pascal-abbrev-no-autodetect, np-pascal-skip-names, np-custom-pattern-prefix | NamePatternRule | test_component_naming edge cases |
| np-custom-method-camel-forbidden, np-message-handler-snake-forbidden, np-property-camel-forbidden | NamePatternRule | test_component_naming specific node types |
| np-mixedcase-pattern | NamePatternRule | test_component_naming MixedCase (custom pattern with suggestion convention) |
| poll-1000, poll-5000, poll-30000 | PollingIntervalRule | test_polling_interval, configs |
| pylint-e-warning | PylintScriptRule | test_pylint_category_mapping |
| bcr-case-insensitive, bcr-sibling-only-warning | BadComponentReferenceRule | test_bad_component_reference |
| crv-no-scripts, crv-no-expressions, crv-no-property-bindings | ComponentReferenceValidationRule | test_component_reference_validation |
| ecd-tight | ExcessiveContextDataRule | iglint (small thresholds so small views trigger every detector) |
| access-private, access-public, access-protected-exempt | PropertyAccessRule | test_property_access |
| persistence-false, persistence-true, persistence-false-no-exempt | PropertyPersistenceRule | test_property_persistence |

Every variant is run on every view (not only the views upstream pairs it
with), which is cheap and widens coverage. The only exception is listed under
`_exclude` in `variants.json`: `ecd-tight` is not run on
`cases/BadContextData` (it would produce about 7000 findings).

### 1.5 File formats

`expected.json`: the default variant's findings, a JSON array of

```json
{
  "rule": "NamePatternRule",
  "severity": "warning",
  "node_path": "root.root.children[0].iconCamelCase",
  "message": "root.root.children[0].iconCamelCase: Name 'iconCamelCase' doesn't follow PascalCase for component"
}
```

* `message` is the exact text ign-lint prints after `• `.
* `node_path` is the text before the first `": "` of the message (`null` if
  there is none). For PylintScriptRule pylint findings it is the script node
  path, and the entry also has `"code"` (for example `"E0602"`) and `"line"`
  (ign-lint's reported line, which is the body line + 1).
* PylintScriptRule's data-quality findings (indentation, mixed tabs and
  spaces) are plain entries without `code`/`line`.
* Order: rules in ign-lint's registry order, errors before warnings within a
  rule, then node order; pylint entries in pylint's output order. **Compare as
  multisets**: ign-lint's rule order depends on filesystem order and its
  stdout groups pylint findings by category.

`expected-variants.json`: `{variant: {"findings": [...], "fixes": [...]}}`
with findings as above and fixes as produced in fix mode (`--fix-dry-run`
semantics; nothing applied):

```json
{
  "rule": "UnusedCustomPropertiesRule",
  "is_safe": true,
  "safety_notes": null,
  "description": "Remove unused custom property 'unusedThing'",
  "violation_message": "custom.unusedThing: custom property 'unusedThing' is defined but never referenced",
  "operations": [
    {"op": "delete_key", "json_path": ["custom", "unusedThing"],
     "description": "Remove custom.unusedThing value entry", "old_value": 3}
  ]
}
```

`op` is `set_value` (with `old_value`, `new_value`), `string_replace` (with
`old_substring`, `new_substring`) or `delete_key` (with `old_value`).
`old_value` is always the value currently at `json_path` in the input view;
when its canonical JSON (`json.dumps(value, sort_keys=True, ensure_ascii=False)`)
is longer than 2000 characters it is stored as
`{"$sha256": <hex digest of that text>, "$json_length": <length>}`.
`json_path` addresses the parsed JSON (keys and array indices). Fixes are
listed in generation order, which is the order ign-lint applies them in (and
resolves conflicts in).

### 1.6 Default findings per view

Counts of `expected.json` per rule (E = error, W = warning) and number of
default fixes and of non-default variants with findings:

| view | NP | Poll | Pyl | BCR | CRV | Unused | ECD | default fixes | variants with findings |
|---|---|---|---|---|---|---|---|---|---|
| cases/AllNodeTypes | 0 | 1E | 0 | 0 | 0 | 3E | 0 | 0 | 6 |
| cases/AllScriptTypes | 0 | 0 | 6W | 0 | 0 | 0 | 0 | 0 | 16 |
| cases/BadComponentReferences | 4W | 0 | 4E 18W | 27E | 8E | 0 | 0 | 0 | 32 |
| cases/BadContextData | 3W | 0 | 1E 4W | 3E | 1E | 0 | 3E | 0 | 30 |
| cases/ExpressionBindings | 0 | 5E | 4W | 0 | 0 | 4E | 0 | 1 | 22 |
| cases/ExtensionFunctions | 1W | 0 | 0 | 0 | 0 | 1E | 0 | 1 | 23 |
| cases/LineDashboard | 11W | 0 | 1E 24W | 0 | 0 | 6E | 0 | 6 | 31 |
| cases/MixedCase | 11W | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 21 |
| cases/PascalCase | 0 | 0 | 0 | 0 | 0 | 4E | 0 | 3 | 16 |
| cases/Pipes | 0 | 0 | 1W | 5E | 4E | 1E | 0 | 0 | 16 |
| cases/PreferredStyle | 0 | 0 | 0 | 0 | 0 | 4E | 0 | 3 | 11 |
| cases/PylintViolations | 0 | 0 | 3E 16W | 0 | 0 | 0 | 0 | 0 | 17 |
| cases/Title Case | 1W | 0 | 0 | 0 | 0 | 1E | 0 | 0 | 22 |
| cases/UPPER_CASE | 1W | 0 | 0 | 0 | 0 | 4E | 0 | 3 | 23 |
| cases/camelCase | 1W | 0 | 0 | 0 | 0 | 6E | 0 | 4 | 23 |
| cases/inconsistentCase | 3W | 0 | 0 | 0 | 0 | 15E | 0 | 14 | 25 |
| cases/kebab-case | 1W | 0 | 0 | 0 | 0 | 4E | 0 | 3 | 24 |
| cases/snake_case | 1W | 0 | 0 | 0 | 0 | 4E | 0 | 3 | 24 |
| probes/NamingAbbreviations | 11W | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 21 |
| probes/PollingQuirks | 0 | 12E | 0 | 0 | 0 | 0 | 0 | 0 | 14 |
| probes/ReferenceQuirks | 0 | 0 | 1W | 4E | 7E | 0 | 0 | 0 | 19 |
| probes/ScriptLines | 0 | 0 | 8E 12W | 0 | 0 | 2E | 0 | 0 | 14 |
| probes/ScriptPy2Syntax | 0 | 0 | 1E | 0 | 0 | 0 | 0 | 0 | 11 |
| probes/UnusedQuirks | 0 | 0 | 0 | 0 | 0 | 8E | 0 | 4 | 10 |

## 2. Upstream test suite catalogue (v0.6.4)

Produced by reading every file under upstream `tests/` at v0.6.4. Paths are
relative to upstream `tests/`. Kwargs are quoted verbatim.

### 2.1 Harness overview

#### Running tests (`tests/test_runner.py`)
- It is a plain `unittest` runner. There is no pytest or conftest. It inserts `../src` into `sys.path`, and tests import helpers as `fixtures.*`, so they have to run with `tests/` on `sys.path` (that is, from inside `tests/`).
- Flags:
  - `--run-unit`: `unittest.TestLoader().discover("tests/unit", pattern="test_*.py" or --unit-pattern)`. Subpackages `properties/`, `scripts/`, `structure/` are found because they have `__init__.py`.
  - `--run-integration`: the same over `tests/integration` (or `--integration-pattern`).
  - `--run-all`: both.
  - `--test NAME`: case-insensitive substring match on `test_*.py` stems. It only looks at the top level of `unit/`, then `integration/`, so subfolder tests cannot be reached this way. It runs the first match via `loadTestsFromName`.
  - `--list`: lists top-level unit and integration test files, plus `cases/*/**/view.json`.
  - `--setup`: creates `unit/`, `integration/`, `fixtures/`, `configs/`, `results/`, `cases/` and missing `__init__.py` files.
  - `-v` (counted, default 1, capped at 2) and `-q` (verbosity 0).
  - Exit code is 1 if any suite fails.
- `tests/README.md` documents `--run-config` and `--tags`. Neither exists in `test_runner.py`; argparse would reject them. The README also points at `tests/configs/`, but the real config directory is `tests/integration/configs/`.

#### Base classes and helpers (`tests/fixtures/`)

**`base_test.BaseRuleTest(unittest.TestCase)`**
- `setUp` sets `test_cases_dir = tests/cases` and `configs_dir = tests/configs`.
- `create_lint_engine(rule_configs)`: for each `{RuleName: {"kwargs": {...}}}` it calls `RULES_MAP[name].create_from_config(kwargs)` and returns `LintEngine(rules)`. An unknown rule name or a construction exception causes `self.fail`. The `enabled` key is ignored.
- `run_lint_on_file(path, cfg)`: skips the test if the file is missing, then runs `flatten_file(path)` and `engine.process(flattened)` with no fix context. The result is stored in `self.last_results` (a `LintResults` with `.errors` and `.warnings` dicts of rule name to list of message strings, plus `.fixes` and `.custom_formatted_errors`).
- `run_lint_on_mock_view(json_str, cfg)`: writes the string to a temp file, calls `run_lint_on_file`, then deletes the file.
- Accessors: `get_error_count(rule=None)`, `get_warning_count`, `get_errors_for_rule`, `get_warnings_for_rule`, `has_errors`, `has_warnings`.
- `assert_no_issues(rule)`: errors plus warnings must equal 0.
- `assert_rule_passes(file, cfg, rule)`: 0 errors. Warnings are allowed and only printed.
- `assert_rule_fails(..., expected_error_count=None)`: more than 0 errors, and an exact count if one is given.
- `assert_error_contains(..., pattern)`: at least one error contains the substring.
- `assert_violations(..., expected_warnings, expected_errors, warning_patterns, error_patterns)`: exact counts, and every pattern must be a substring of at least one message. Patterns are checked independently, so they can match different messages.
- `assert_rule_warnings(..., expected_warning_count, warning_patterns)` and `assert_rule_errors(..., expected_error_count, error_patterns)`: exact count plus substring patterns.
- `assert_rule_passes_completely`: same as `assert_no_issues`.
- `assert_rule_summary(..., expected_warnings, expected_errors)`: exact counts for both.

**`base_test.BaseIntegrationTest`**
- `run_multiple_rules_detailed(file, cfg)` builds rules the same way, except unknown rule names are silently skipped. It runs `flatten_file` then `process` and returns `LintResults`.
- `run_multiple_rules` returns `{**warnings, **errors}`. Because this is a `dict.update`, a rule that has both warnings and errors only keeps its errors in the combined dict.
- `assert_total_errors(dict, n)`.

**`test_helpers`**
- `create_mock_view(components, custom_properties)` produces an unusual shape: `{"meta":{"name":"root"},"type":"ia.container.coord","version":0,"props":{..., "children": {<name>: {"meta":{"name":<name>},"type":..,"props":..}}}}`. There is no `root` key, and `children` is a dict keyed by component name, not a list.
- `create_temp_view_file(str) -> Path`: a `.json` temp file that is never deleted.
- `assert_rule_errors(errors_dict, rule, expected_count, error_patterns)` and `assert_no_errors(errors_dict, rule=None)`: plain `assert`-based checks.
- `get_test_config(rule, **kw)` returns `{rule: {"enabled": True, "kwargs": kw}}`.
- `load_test_view(cases_dir, name)` returns `cases/<name>/view.json` or raises `FileNotFoundError`.
- `create_mock_script(script_type, source, component_name="TestComponent")`. Every variant uses `{"custom":{},"params":{},"propConfig":{},"props":{},"root":{...}}` with `root.meta.name="root"` and `type ia.container.coord`:
  - `message_handler`: `root.scripts.messageHandlers[0] = {messageType:"test_message", script, pageScope:false, sessionScope:false, viewScope:true}`, with one button child.
  - `custom_method`: child button `scripts.customMethods[0] = {name:"testMethod", params:[], script}`.
  - `transform`: top-level `propConfig["root.children[0].props.text"].binding = {type:"property", config:{path:"view.params.inputValue"}, transforms:[{type:"script", code}]}`. The odd dotted key sits in the view-level propConfig. There is one label child.
  - `event_handler`: child button `events.component.onActionPerformed = {config:{script}, scope:"G", type:"script"}`.

**`mock_data.py`**: component, expression-binding and script generators. No test uses it.

#### Config-driven integration framework (`fixtures/config_framework.py`, run by `integration/test_config_framework.py`)
- `ConfigurableTestFramework()` uses `config_dir = tests/integration/configs` and `test_cases_dir = tests/cases`. It loads every `configs/**/*.json` with `rglob`.
- Config file format:
  - `{"test_suite_name","description","test_cases":[...]}`.
  - Each case: `{"name","description","view_file" (relative to tests/cases, e.g. "camelCase/view.json"),"rule_config":{Rule:{"enabled":bool,"kwargs":{}}},"expectations":[...],"tags":[],"skip":bool,"skip_reason":""}`.
  - Each expectation: `{"rule_name","error_count"(default 0),"warning_count"(default 0),"error_patterns":[],"warning_patterns":[],"should_pass"(default error_count==0)}`.
- Execution:
  - Skipped cases are reported as skipped. A missing view file is an "error".
  - Rule keys that start with `_`, are not dicts, have `enabled: false`, or are unknown are skipped. Rule construction errors are printed and skipped.
  - Then `flatten_file`, `LintEngine.process`.
  - An expectation is met when the warning count and error count match exactly, `(errors==0)==should_pass`, and every pattern is a substring of at least one message of that severity.
  - `run_all_tests(tags)` filters on any matching tag.
- **The only unittest entry point, `integration/test_config_framework.py::ConfigurableTestRunner.test_run_configured_tests`, asserts only that the result counts are ints and `total >= 0`.** Failing config expectations are printed but never fail the suite.
- The `ConfigurableTestRunner` class inside `fixtures/config_framework.py` is dead code with bugs: it references an undefined `results` variable and missing `expected_count` keys. It is never discovered.

#### Golden-file tests (`unit/test_golden_files.py`, data in `tests/debug/cases/<Case>/`)
- Every `tests/cases/<X>/view.json` must have a `tests/debug/cases/<X>/` directory, otherwise `setUpClass` raises `RuntimeError("Missing golden files ...")`. All 18 cases have one, containing `flattened.json`, `model.json`, `stats.json` and a `README.md`.
- The engine is built from all `RULES_MAP` rules with `create_from_config({})`.
- `flattened.json` holds the output of `flatten_json(read_json_file(view))`, a map from path to leaf value. It is compared with full `assertEqual` on dicts, so key order does not matter but every key and value must match. Things this pins down:
  - Component objects (anything with `meta.name`) get their name inserted as a path segment, which gives `root.root.children[0].IconPascalCase.meta.name`.
  - Top-level keys are kept as-is (`custom.X`, `params.X`, `propConfig.custom.X.persistent`).
  - Empty `{}` and `[]` are kept as leaves (for example `"props": {}`).
- `model.json` holds `engine.serialize_view_model()` after setting `engine.flattened_json` and `engine.view_model = engine.get_view_model()`. The shape is `{section: {"count": n, "nodes": [{"path","name","node_type",...}]}}`, with sections `bindings, components, custom_methods, event_handlers, expression_bindings, expression_struct_bindings, message_handlers, properties, property_bindings, property_change_scripts, query_bindings, script_transforms, scripts, tag_bindings`. The comparison is loose. It checks:
  - the set of section keys
  - each section's `count`
  - the length of each section's `nodes`
  - the sorted list of node `path`s

  Node fields such as `name`, `type`, `value`, `persistent` and `properties_count` are not compared.
- `stats.json` holds `engine.get_model_statistics(flattened)`. Only `total_nodes`, `node_type_counts`, `components_by_type` and `model_keys` (an ordered list) are compared. `rule_coverage` is ignored.
- `test_00_golden_files_can_be_generated` (PascalCase only) checks that the flattened output is non-empty, `stats.total_nodes > 0` and `'components' in model`.
- Examples of path conventions from the goldens:
  - Component paths: `root.root`, `root.root.children[0].IconPascalCase`.
  - Property paths: `custom.CustomViewParam`.
  - Binding paths: `propConfig.custom.expr`.
  - Script transforms: `propConfig.custom.transformScript.binding.transforms[0]`.
  - Expression transforms count as expression bindings: `propConfig.custom.transformExpression.binding.transforms[0]`.

#### CLI subprocess tests vs in-process tests
- **Subprocess (real CLI):**
  - `integration/test_cli_allow_fix.py` and `test_cli_fix_mode.py`: `python src/ignition_lint/__main__.py ...` with cwd set to the repo root.
  - `test_cli_integration.py`: `__main__.py` if it exists, otherwise `poetry run ign-lint` or `python -m ignition_lint`. cwd is None.
  - `test_whitelist_integration.py`: `python -m ignition_lint.cli` with cwd set to a temp dir and `PYTHONPATH=src`.
- **In-process:** everything else.
  - All `unit/**`.
  - `integration/test_multi_rule.py`, `test_pipe_references.py`, `test_config_framework.py`.
  - `unit/test_allow_fix.py`, `test_batch_cleanup.py` and `test_whitelist.py` call CLI module functions (`create_rules_from_config`, `aggregate_batch_results`, `cleanup_old_batch_files`, `load_whitelist`, `collect_files`, `generate_whitelist`) directly.

#### Default severities implied by the tests
| Rule | Default severity |
|---|---|
| NamePatternRule | warning |
| UnusedCustomPropertiesRule | error |
| BadComponentReferenceRule | error |
| ComponentReferenceValidationRule | error |
| PollingIntervalRule | error (never warnings) |
| ExcessiveContextDataRule | error |
| PropertyAccessRule | error |
| PropertyPersistenceRule | error |
| PylintScriptRule | per-category mapping |

`LintResults.errors` and `.warnings` omit rules that have no entries (tests assert `results.errors == {}`).

---

### 2.2 Per-file catalogue

#### test_runner.py
Purpose: the CLI runner described in 2.1. It has no tests of its own.

#### fixtures/*.py
Purpose: helpers described in 2.1. They contain no test methods that ever run.

#### integration/test_config_framework.py
Purpose: runs every JSON config case. It only asserts that the framework runs.
- `ConfigurableTestRunner.test_run_configured_tests`: `results['total'] >= 0` and the passed, failed and errors counts are ints. It prints failures but never fails on unmet expectations.

#### integration/configs/naming/name_pattern_tests.json (NamePatternRule)
| case | view | kwargs | expectation |
|---|---|---|---|
| pascal_case_positive | PascalCase | `{"convention":"PascalCase","allow_numbers":true,"min_length":1}` | 0 errors, pass |
| pascal_case_negative | camelCase | `{"convention":"PascalCase","allow_numbers":true,"min_length":1,"severity":"error"}` | 1 error, pattern `"doesn't follow"` |
| camel_case_positive | camelCase | `{"convention":"camelCase","allow_numbers":true,"min_length":1}` | 0 errors |
| snake_case_positive | snake_case | `{"convention":"snake_case","allow_numbers":true,"min_length":1}` | 0 errors |
| inconsistent_case_negative | inconsistentCase | `{"convention":"PascalCase","allow_numbers":true,"min_length":1,"severity":"error"}` | 3 errors (the components badEmbeddedPotato_0, "bad potato", anotherBadPotato) |
| pascal_case_negative_warnings | camelCase | same with `"severity":"warning"` | 1 warning, 0 errors, pattern `"doesn't follow"` |
| pascal_case_negative_errors | camelCase | same with `"severity":"error"` | 0 warnings, 1 error, `"doesn't follow"` |

Note that the default `target_node_types` (no key given) is evidently components only, since expectations of 0 or 1 would be wrong if properties were checked.

#### integration/configs/performance/polling_interval_tests.json
- expression_bindings_check: ExpressionBindings, `{"minimum_interval":10000}`, expects 5 errors. The view contains `now()` 4 times, `now(5000)` and `now(11000)`, so `now()` with no argument counts as below the minimum.

#### integration/configs/scripts/pylint_script_tests.json
- basic_script_check: PascalCase, PylintScriptRule `{}`, expects 0 errors.

#### integration/configs/cross-rule/warnings_vs_errors.json
- name_pattern_warning_demo: camelCase, `{"target_node_types":["component"],"convention":"PascalCase","min_length":1}`, expects 1 warning, 0 errors, warning patterns `"doesn't follow"` and `"PascalCase"`.
- polling_interval_error_demo: ExpressionBindings, `{"minimum_interval":5000}`, expects 0 warnings and 4 errors.
- perfect_view_no_issues: PascalCase, same NamePattern kwargs as the first case, expects 0 and 0.

#### integration/test_cli_allow_fix.py (subprocess)
Purpose: the per-rule `allow_fix` config key end to end. Inline view `UNUSED_PROP_VIEW`: `custom.unusedProp="value"`, `propConfig["custom.unusedProp"]={"persistent":true}`, `root` is `ia.container.flex` with one child `badButton` (`ia.input.button`, `props {}`). Config: `UnusedCustomPropertiesRule {"severity":"error"}`, optionally with `allow_fix`, and optionally `NamePatternRule {"convention":"PascalCase","target_node_types":["component"],"severity":"warning"}`.
- `test_allow_fix_false_reports_but_does_not_modify`: `allow_fix:false` plus `--fix`: stdout contains `unusedProp` and the file is byte-equal as JSON.
- `test_allow_fix_absent_defaults_to_fixing`: no key plus `--fix`: `custom.unusedProp` and `propConfig["custom.unusedProp"]` are removed.
- `test_dry_run_shows_no_fixes_for_disallowed_rule`: `allow_fix:false` plus `--fix-dry-run`: stdout does not contain `"Remove unused"` and the file is unchanged.
- `test_fix_rules_cli_overrides_allow_fix_false`: `--fix --fix-rules UnusedCustomPropertiesRule` overrides `allow_fix:false`, so the prop is deleted.
- `test_mixed_rules_only_allowed_rule_fixes`: Unused has `allow_fix:false` and NamePattern is allowed. `--fix` renames the child to `BadButton` and `unusedProp` stays.
- `test_unknown_fix_rules_name_warns_user`: `--fix-rules UnusedCustomPropertysRule` (typo): stdout contains `"does not match any loaded rule"` and the file is unchanged.
- `test_invalid_allow_fix_value_alerts_user`: `allow_fix:"false"` (a string), no `--fix`: stdout contains `allow_fix` and `"Error creating rule UnusedCustomPropertiesRule"`.

#### integration/test_cli_fix_mode.py (subprocess)
Purpose: `--fix`, `--fix-unsafe` and `--fix-dry-run` semantics (issues #93 and #94), plus Unused fixes. `make_view(children)` gives `{"root":{children,meta root,props {},type ia.container.flex}}`.
- `test_fix_unsafe_alone_applies_safe_fixes`: `badButton` button, `PASCAL_CASE_CONFIG` (NamePattern PascalCase, components, warning), `--fix-unsafe` alone renames it to `BadButton`.
- `test_fix_unsafe_alone_applies_unsafe_fixes`: adds `MyLabel` with `props.text="{../badButton.props.text}"`. `--fix-unsafe` renames the button and the label text contains `BadButton`.
- `test_fix_unsafe_help_text_drops_use_with_fix`: `--help` contains `--fix-unsafe`, and the whitespace-collapsed help does not contain `"use with --fix"`.
- `test_fix_reports_post_fix_clean_exit`: `--fix` on `badButton`: renamed, exit code 0, stdout contains `"No style inconsistencies found"`. This shows the rules are re-evaluated after fixing.
- `test_fix_applies_exactly_once`: exactly one stdout line contains `"| Skipped:"` (the `Applied: N fix(es) | Skipped: ...` summary).
- `test_fix_removes_unused_custom_property`: `MyLabel` label, `custom.unusedProp`, `propConfig["custom.unusedProp"]={"access":"PRIVATE","persistent":true}`, UNUSED config (`severity error`), `--fix`: both entries removed, exit 0.
- `test_fix_keeps_unused_parameter`: `params.unusedParam=""`, `--fix`: the param stays and the exit code is not 0.
- `test_fix_unsafe_also_keeps_unused_parameter`: the same with `--fix-unsafe`: the param stays and the exit code is not 0.
- `test_fix_dry_run_previews_property_removal`: `custom.unusedProp` and `--fix-dry-run`: the file is unchanged and stdout contains `"DELETE"`.
- `test_fix_dry_run_does_not_reevaluate_or_modify`: `badButton` and `--fix-dry-run`: the name is unchanged and the exit code is not 0.

#### integration/test_cli_integration.py (subprocess)
- `TestCLIIntegration.test_cli_help`: `--help` returns 0 and stdout (lowercased) contains `"usage:"`.
- `test_cli_with_config_file`: config `{"NamePatternRule":{"enabled":true,"kwargs":{"convention":"PascalCase"}}}` with `--config <f> --files cases/PascalCase/view.json --verbose`: exit code is 0 or 1.
- `test_cli_stats_only`: `--files PascalCase/view.json --stats-only`: exit 0 and lowercased stdout contains one of `statistics`, `nodes`, `processed`.
- `test_cli_version_or_basic_execution`: `--help` returns an int exit code.
- `test_cli_files_processes_all_paths_passed_after_flag`: for N in 1, 2, 3, 5 it writes N inline views `{"root":{"type":"ia.container.flex","props":{},"children":[]}}` (no meta) at `view_i/view.json`, then runs `--stats-only --files p1 ... pN`. stdout must contain `"Files processed: N"` and every `view_i`. stdout plus stderr must contain `"Deprecation"` when N >= 2 and must not when N == 1.
- `TestCLIDiscovery.test_cli_methods_available`: at least one way to run the CLI exists.

#### integration/test_multi_rule.py (in-process, BaseIntegrationTest)
- `TestMultipleRules.test_name_pattern_and_polling_rules`: PascalCase, NamePattern `{"target_node_types":["component"],"convention":"PascalCase"}` plus Polling `{"minimum_interval":10000}`: the combined NamePatternRule list is `[]`.
- `test_all_available_rules_together`: PascalCase, camelCase and ExpressionBindings with NamePattern (as above), Polling 5000 and Pylint `{}`. Smoke test: results are a dict of lists.
- `test_rule_interactions`: PascalCase. NamePattern messages are the same when run alone and when run with Polling 10000.
- `test_error_aggregation`: camelCase, NamePattern (default severity warning) plus Polling 1000: more than 0 NamePatternRule entries in the combined dict, total more than 0.
- `test_mixed_node_type_naming_standards`: PascalCase and camelCase. NamePattern `{"target_node_types":["component","property"],"convention":"PascalCase","node_type_specific_rules":{"component":{"convention":"PascalCase","min_length":1,"allow_numbers":true},"property":{"convention":"camelCase","min_length":1,"allow_numbers":true,"skip_names":["id","x","y","z"]}}}` plus Polling 5000. Smoke test.
- `test_comprehensive_rule_coverage`: PascalCase. NamePattern `{"target_node_types":["component","property","custom_method","event_handler"],"convention":"PascalCase","node_type_specific_rules":{"component":{"convention":"PascalCase","min_length":1},"property":{"convention":"camelCase","min_length":1},"custom_method":{"convention":"camelCase","min_length":3},"event_handler":{"convention":"camelCase","min_length":2,"skip_names":["onClick","onLoad","onFocus","onBlur"]}}}` plus Polling 10000 and Pylint. Smoke test.
- `TestRuleConflictPrevention.test_consistent_component_naming_across_runs`: PascalCase, `{"target_node_types":["component"],"convention":"PascalCase","min_length":1}` run 3 times gives identical results.
- `test_property_naming_independence`: PascalCase with `{"target_node_types":["component"],"convention":"PascalCase"}`, `{"target_node_types":["property"],"convention":"camelCase"}` and `{"target_node_types":["component","property"],"convention":"PascalCase","node_type_specific_rules":{"component":{"convention":"PascalCase"},"property":{"convention":"camelCase"}}}`. Smoke test (lists only).

#### integration/test_pipe_references.py (in-process, cases/Pipes)
Config: `ComponentReferenceValidationRule {}` plus `BadComponentReferenceRule {}`.
- `test_all_broken_button1_references_detected`: exactly 3 validation errors contain `Button1`. At least one contains (lowercased) `"property binding"`, one contains `"expression"`, and one contains `"not found as child"` (the `getChild('Button1')` transform script).
- `test_broken_button2_expression_reference_detected`: exactly 1 error contains `Button2`, and it contains `"expression"`.
- `test_every_broken_reference_is_reported`: exactly 4 validation errors in total.
- `test_valid_button_reference_not_flagged_as_broken`: no validation error contains `'Button'` (with the single quotes). This implies component names appear single-quoted in messages.
- `test_property_binding_traversal_flagged_as_brittle`: exactly 2 BadComponentReferenceRule errors contain `"Property Binding"` (title case), for `./Button` and `./Button1`.

#### integration/test_whitelist_integration.py (subprocess, `-m ignition_lint.cli`)
Setup: in a temp dir, `view1/view.json` and `view2/view.json` are "valid" and `view3/view.json` is "invalid". **Inline shape:** `{"custom":{},"params":{},"props":{"children":[{"meta":{"name":"ValidComponent" | "invalid_component"},"type":"ia.display.label","props":{"text":"Test"}}]},"root":{"meta":{"name":"root"},"type":"ia.container.coord"}}`. The children sit under the view's top-level `props`, not under `root`. Config: `{"NamePatternRule":{"enabled":true,"kwargs":{"convention":"PascalCase"}}}`.
- `test_cli_with_whitelist_filters_files`: whitelist `view3/view.json` with `--files "**/view.json"`: exit 0 and stdout does not contain `invalid_component`.
- `test_cli_without_whitelist_processes_all`: exit code not 0 and stdout contains `invalid_component`. So NamePatternRule flags the component under top-level props, and a warning-only result still gives a non-zero exit.
- `test_verbose_reports_ignored_files`: whitelist `view1` with `--verbose`: lowercased stdout contains `"skipped"`.
- `test_generate_whitelist_creates_file`: `--generate-whitelist view1/view.json view2/view.json --whitelist-output test_whitelist.txt`: exit 0, the file exists and contains both paths.
- `test_generate_and_use_whitelist_workflow`: generate from view3, then lint with `--whitelist generated_whitelist.txt`: exit 0.
- `test_no_whitelist_flag_overrides`: `--whitelist` together with `--no-whitelist`: exit code not 0 and `invalid_component` in stdout.
- `test_dry_run_does_not_create_file`: `--generate-whitelist ... --dry-run`: exit 0 and no file.
- `test_append_mode_preserves_existing`: an existing `view1` entry plus `--append` of view2: the file contains both.

#### unit/test_allow_fix.py (in-process)
Inline view: `custom.unusedProp`, `propConfig["custom.unusedProp"]={"persistent":true}`, `root {children [], meta root}`. Processed with `json_data` and a `PathTranslator`, which is fix mode.
- `TestAllowFixGating.test_allow_fix_defaults_to_true`: `create_from_config({}).allow_fix is True`.
- `test_fixes_generated_when_allowed`: 1 error and 1 fix.
- `test_no_fixes_but_violations_still_reported_when_disallowed`: with `rule.allow_fix=False`: 1 error and `fixes == []`.
- `test_set_fix_context_refused_when_disallowed`: `set_fix_context({}, object())` leaves `has_fix_context` False.
- `test_reenabling_allow_fix_restores_fixes`: setting False then True gives 1 fix.
- `TestAllowFixConfigParsing.test_allow_fix_false_applied_to_instance`: `create_rules_from_config({Rule:{"enabled":true,"allow_fix":false}})` returns `(rules, statuses)`. The rule has `allow_fix False`, and the status is `state=="loaded"`, `detail=="allow_fix=false"`.
- `test_allow_fix_defaults_true_when_absent`: `create_rules_from_config({})` still loads UnusedCustomPropertiesRule (rules missing from the config load by default) with `allow_fix True` and `detail None`.
- `test_invalid_allow_fix_values_error_rule_and_alert_user`: each of `"no"`, `"false"`, `"true"`, `0`, `1`, `None`, `[]`, `{"on":true}` gives a None rule, `state=="error"`, `"allow_fix"` in the detail, and stdout containing `allow_fix` and the rule name.
- `test_allow_fix_on_non_fixable_rule_loads_and_alerts_user`: PollingIntervalRule with `allow_fix:false` loads, and stdout contains `"allow_fix has no effect"` and `PollingIntervalRule`.
- `test_no_notice_for_non_fixable_rule_without_the_key`: stdout does not contain `allow_fix`.
- `test_other_rules_unaffected_by_one_rules_bad_allow_fix`: Unused with `allow_fix:"oops"` still lets NamePatternRule load with `allow_fix True`.

#### unit/test_batch_cleanup.py (in-process, CLI functions)
- `TestCleanupOldBatchFiles.test_stale_aggregated_summary_is_deleted`: `results_AGGREGATED_SUMMARY.txt` with mtime 60 s ago is deleted by `cleanup_old_batch_files(results.txt)`.
- `test_fresh_aggregated_summary_is_preserved`: an mtime of now (under 5 s) is kept.
- `test_stale_base_results_file_is_deleted`: a stale `results.txt` is deleted.
- `test_stale_batch_files_from_other_pid_are_deleted`: stale `results_pid{otherpid}_batch1.txt` is deleted.
- `test_current_pid_batch_files_are_preserved`: the current pid's batch file is kept even when stale.
- `test_missing_directory_is_noop`: no exception.
- `TestAggregateBatchResults.test_non_batch_path_does_not_read_stale_summary`: `aggregate_batch_results(results.txt)` returns None even when a summary exists.
- `test_batch_path_aggregates_existing_files`: `results.txt` (files 3, warnings 4, errors 1, issues 2, clean 1) plus `results_pid{pid}_batch1.txt` (2, 1, 3, 1, 1) gives totals `files 5, warnings 5, errors 4, issues 3, clean 2`, and the summary file is written. The line format is `"Files processed:      N"`, `"Total warnings:"`, `"Total errors:"`, `"Files with issues:"`, `"Clean files:"`.

#### unit/test_binding_rules.py
- `test_binding_rule_base_functionality`: ExpressionBindings, Polling `{"minimum_interval":5000}`. Smoke test.
- `test_multiple_binding_types`: ExpressionBindings and PascalCase, Polling 1000. Smoke test.

#### unit/test_discovery.py
- `test_discover_test_cases`: every `cases/*` directory with a `**/view.json` has view files named `view.json`.
- `test_config_creation_and_usage`: `get_test_config("NamePatternRule", convention="PascalCase") == {"NamePatternRule":{"enabled":True,"kwargs":{"convention":"PascalCase"}}}`.
- `test_mock_view_creation`: `create_mock_view` output has `meta.name=="root"` and `props`.
- `test_temp_view_file_creation`: the temp file round-trips.
- `test_helper_assertion_functions`: helper assertions behave, including that a count mismatch raises AssertionError.
- `TestFrameworkIntegration.test_rule_engine_creation`: engines build for NamePattern PascalCase and Polling 5000.
- `test_multiple_rules_together`: PascalCase, NamePattern `{"target_node_types":"component","convention":"PascalCase"}` (a **string**, not a list) plus Polling 10000. It runs and the error count is an int.

#### unit/test_example_mixed_severity.py (ExampleMixedSeverityRule from `rules/_examples`, registered on import)
All tests use inline `create_mock_view` (dict children under top-level props, short types like `button` or `label`), rule kwargs `{}`.
- `test_warnings_for_style_issues`: tempButton, x, MyComponent give 3 warnings and 0 errors.
- `test_errors_for_functional_issues`: UnsafeComponent, DebugProdPanel give 1 warning and 2 errors.
- `test_mixed_warnings_and_errors`: tempButton, AdminPanel, x, TestLiveComponent give 4 warnings and 2 errors.
- `test_clean_components_pass`: SubmitButton, UserNameLabel, DataPanel, MainContainer give no issues.
- `test_warning_patterns`: tempLogin gives 2 warnings, with patterns `"temporary naming pattern"` and `"consider renaming"`.
- `test_error_patterns`: DebugComponent gives 1 error, with patterns `"potentially unsafe"` and `"debug functionality"`.
- `test_exception_cases`: ok, id, go give no issues.

#### unit/test_flatten_json.py
- `test_flatten_empty_dict`: `{}` gives an empty OrderedDict.
- `test_flatten_simple_dict`: dotted keys in insertion order.
- `test_flatten_nested_dict`: `component.props.style.color` and so on.
- `test_flatten_with_lists`: `items[0]`, `nested.numbers[2]`.
- `test_flatten_list_with_objects`: `children[0].name`.
- `test_flatten_with_component_name`: a top-level `{"meta":{"name":"MyButton"},...}` gives `MyButton.meta.name`, `MyButton.type`, `MyButton.props.text`. The name is prefixed even at the root.
- `test_flatten_root_list`: `["item1","item2",{"nested":"value"}]` gives `[0]`, `[1]`, `[2].nested`.
- `test_flatten_complex_ignition_structure`: `Root.meta.name` and `Root.children[0].Button_0.props.text == "Submit"`, `Root.children[0].Button_0.props.events.onActionPerformed.script`.
- `test_flatten_empty_list`: `{"items":[],"nested":{"empty":[]}}` gives `items: []` and `nested.empty: []`.
- `test_flatten_empty_nested_dict`: `custom.emptyMeta: {}`.
- `test_flatten_mixed_types`: str, int, float, bool and None leaves, including inside lists.
- `TestFileOperations.test_read_json_file`, `test_write_json_file`, `test_flatten_file` (returns an OrderedDict).
- `test_format_json`: 2-space indent and no trailing whitespace on any line.
- `TestUnicodeHandling.test_preserve_unicode_escapes`: literal `\u003c` and `\u003e` become `UNICODE_LT` and `UNICODE_GT` placeholders.
- `test_restore_unicode_escapes`: the reverse.
- `test_unicode_roundtrip`: `\u003c, \u003e, \u0026, \u003d, \u0027` round-trip unchanged.

#### unit/test_golden_files.py
See 2.1.
- `test_00_golden_files_can_be_generated` (PascalCase).
- `test_01_flattened_json_matches_golden` (all 18 cases, subTests).
- `test_02_model_matches_golden`.
- `test_03_stats_match_golden`.

#### unit/test_polling_interval.py
- `TestPollingIntervalRule.test_expression_bindings_validation`: ExpressionBindings at 10000. Smoke test.
- `test_different_minimum_intervals`: 1000, 5000, 10000, 30000. Smoke test.
- `TestPollingIntervalValidation.test_no_polling_expressions`: PascalCase at 10000 gives errors `[]`.

#### unit/test_rule_registration.py
- `test_every_rule_can_be_instantiated_with_empty_config`: every `RULES_MAP` entry builds with `create_from_config({})`, is a `LintingRule`, and has a non-empty string `error_message`.
- `test_same_class_registered_twice_is_noop`: `RuleRegistry.register_rule` returns the same value twice and `list_rules()` has length 1.
- `test_different_class_same_name_raises`: `RuleValidationError` for a different class registered under `"SharedName"`.
- `test_metadata_not_computed_at_registration`: `register_rule` does not instantiate. `get_rule_metadata` instantiates once and caches.

#### unit/test_warnings_vs_errors.py
NamePattern kwargs `{"target_node_types":["component"],"convention":"PascalCase","min_length":1}` throughout.
- `test_name_pattern_rule_produces_warnings`: camelCase gives 1 warning and 0 errors.
- `test_name_pattern_rule_warning_content`: camelCase gives 1 warning with patterns `"doesn't follow"` and `"PascalCase"`.
- `test_polling_interval_rule_produces_errors`: ExpressionBindings at 10000 gives no warnings.
- `test_mixed_rules_warnings_and_errors`: camelCase gives 1 warning and 0 errors. Polling 5000 on camelCase gives no warnings.
- `test_rule_passes_completely_no_warnings_or_errors`: PascalCase gives 0 and 0.
- `test_backward_compatibility_still_works`: the same kwargs plus `"severity":"error"`. PascalCase gives 0 errors and camelCase gives exactly 1 error.

#### unit/test_whitelist.py (CLI functions)
- `TestWhitelistLoading.test_load_valid_whitelist`: 2 lines give 2 entries, each an absolute `Path`.
- `test_whitelist_file_not_found`: `set()`.
- `test_comments_ignored`: `#` lines are skipped.
- `test_blank_lines_ignored`: empty and whitespace-only lines are skipped.
- `test_whitespace_trimmed`: spaces and tabs around a line are trimmed.
- `test_paths_normalized_to_absolute`: entries are absolute.
- `test_io_error_returns_empty_set`: passing a directory gives `set()`.
- `TestFileFiltering`:
  - `collect_files(Namespace(filenames, files, verbose), whitelist)` returns `(files, _)`.
  - `test_whitelist_filters_precommit_files`: filenames view1, view2 with view1 whitelisted give only view2.
  - `test_whitelist_filters_glob_files`: `files="<tmp>/**/view.json"` with view1 and view2 whitelisted gives only view3.
  - `test_empty_whitelist_includes_all`: 2 files.
  - `test_path_matching_works_with_relative_input`: relative `view1/view.json` matches the absolute whitelist, giving 0 files.
  - `test_path_matching_works_with_absolute_input`: 0 files.
  - `test_verbose_reports_ignored_files`: 1 file.
- `TestGenerateWhitelist`:
  - `generate_whitelist(patterns, output_file, append, dry_run)`.
  - `test_generate_from_single_pattern`: `views/legacy/*.json` gives view1 and view2, not view3.
  - `test_generate_from_multiple_patterns`: all three.
  - `test_append_mode_adds_to_existing`: keeps `views/existing/file.json` and adds view1.
  - `test_append_mode_deduplicates`: `views/legacy/view1.json` appears once.
  - `test_dry_run_does_not_write_file`.
  - `test_paths_are_relative`: no absolute temp path, and contains `views/legacy/`.
  - `test_paths_are_sorted`: non-comment lines are sorted (comments allowed in the output).

#### unit/test_pylint_category_mapping.py (PylintScriptRule, imported as `src.ignition_lint...`)
- `test_default_category_mapping`: F and E map to error, W, C and R to warning.
- `test_custom_category_mapping_strict` and `test_custom_category_mapping_lenient`: the `category_mapping` kwarg is stored.
- `test_pylint_violation_dataclass`: `PylintViolation(category, code, message, path, line)`.
- `test_structured_violation_storage`: `rule.pylint_violations` is a list.
- `test_get_category_grouped_violations_empty`: `{}`.
- `test_get_category_grouped_violations_single_category`: `{'E': {'severity':'error','name':'Error','violations':[...2]}}`.
- `test_get_category_grouped_violations_multiple_categories`: key order `['F','E','W','C','R']` with severities from the mapping.
- `test_get_category_grouped_violations_custom_mapping`: W mapped to error gives `grouped['W']['severity']=='error'`.
- `test_get_category_grouped_violations_formatting`: a violation with path `test_file::root.Button.onActionPerformed` and line 5 gives a string containing `root.Button.onActionPerformed`, `Line 5`, the message and `E0602`.
- `test_category_order_in_grouped_output`: input order R, F, C, W, E comes out as F, E, W, C, R.
- `test_batch_mode_preserves_violations`: `batch_mode=True` with `process_nodes([])` does not reset.
- `test_non_batch_mode_resets_violations`: `batch_mode=False` with `process_nodes([])` empties the list.
- `test_backward_compatibility_errors_warnings_lists`: `add_violation(msg, severity="error"|"warning")` fills `rule.errors` and `rule.warnings`.
- `test_unknown_category_fallback`: `PylintScriptRule(severity="warning")` with `add_violation(msg, severity=None)` lands in warnings.
- `test_multiple_violations_same_category`: 3 E violations are grouped.
- `test_category_names_mapping`: Fatal, Error, Warning, Convention, Refactor.
- `TestPylintCategoryMappingIntegration.test_parse_pylint_output_extracts_category`: `_parse_pylint_output("test.py:5:10: E0602: Undefined variable 'x' (undefined-variable)\n...W0611...\n...C0114...", line_map, path_to_issues)` gives 3 violations with categories E, W, C and codes in that order.
- `test_print_category_grouped_output`: `format_violations_grouped()` returns a dict with `errors` and `warnings` strings. Errors contain `"Pylint - Error (E)"`, `E0602` and `root.Button.script`. Warnings contain `"Pylint - Warning (W)"`, `W0611` and `root.Label.transform`.
- `test_print_category_grouped_output_severity_icons`: the same headers, with no emojis.

#### unit/test_pylint_trailing_whitespace_fix.py
Inline view: a `root` flex container with child `TestButton` (`ia.input.button`) whose `events.component.onActionPerformed.config.script` is the script under test. It uses a temp pylintrc with `disable=all`, `enable=trailing-whitespace`, `indent-string='\t'`, `max-line-length=200`, passed as the `pylintrc=` kwarg.
- `TestStripTrailingWhitespace.test_strips_spaces_and_tabs`: `"\tx = 1   \n\ty = 2\t\n\tz = 3"` becomes `"\tx = 1\n\ty = 2\n\tz = 3"`.
- `test_preserves_clean_script`: unchanged.
- `test_handles_empty_string`: `""` stays `""`.
- `test_handles_blank_lines`: `"\tx = 1\n   \n\ty = 2"` becomes `"\tx = 1\n\n\ty = 2"`.
- `TestNoFixWithoutContext.test_no_fix_without_fix_context`: `process(flat, source_file_path='test.json')` gives 0 fixes.
- `TestGeneratesFixForTrailingWhitespace.test_generates_fix`: `"\tx = 1   \n\ty = 2\t"` gives a fix with `is_safe True`, `rule_name 'PylintScriptRule'` and one `SET_VALUE` operation with `old_value=script` and `new_value="\tx = 1\n\ty = 2"`.
- `TestOneFixPerScript.test_one_fix_per_script`: 3 lines with C0303 give exactly 1 PylintScriptRule fix.
- `TestFixApplicationEndToEnd.test_fix_then_relint`: `FixEngine(translator).apply_fixes(fixes, safe_only=True)` applies more than 0. After re-flattening and re-linting, `rule2.pylint_violations` has no C0303 and there are 0 fixes.
- `TestProcessNodesResetsFixes.test_resets_between_files`: the same engine and rule process file1 (with fixes) and then a clean file2, which gives 0 fixes.

#### unit/test_script_linting.py (PylintScriptRule `{}`)
- `test_basic_script_linting`: PascalCase. Smoke test.
- `test_multiple_view_files`: PascalCase, camelCase, ExpressionBindings. Smoke test.
- `test_script_with_commented_first_line`: `TransformScript(path="test.path", script=...)` with an unindented script whose first line is a comment and which has 4-space nested lines. `get_formatted_script()` contains `"def transform(self, value):"`. The first comment line starts with `"\t#"`, the `dataset = value` line starts with `"\tdataset"`, and the nested `if` line starts with `"\t    if"` or has exactly one tab.
- `test_script_already_indented_with_comment_first`: the same script already tab-indented. The comment line and the code line each contain exactly one `\t` (no double indent).
- `test_script_with_blank_lines_and_comments`: a leading blank line, comment, blank, comment, blank, then code. The comment starts with `"\t#"` and the code with `"\tdataset"`.
- `test_script_with_mixed_tabs_and_spaces`: inline view with a view-level `propConfig["custom.testProp"].binding` of type `tag` (`tagPath "[default]TestTag"`) and a script transform `"\tdataset = value\n\t    if True:\n\t        return dataset"`. The errors include one containing `"mixes tabs and spaces for indentation"`, and that message contains `"Use either tabs OR spaces consistently"`.
- `test_script_with_only_tabs`: the same shape with code `"\tdataset = value\n\tif True:\n\t\treturn dataset"`. No error contains the mixed message.

#### unit/test_component_naming.py (NamePatternRule)
- `TestNamePatternPascalCase` (`{"target_node_types":["component"],"convention":"PascalCase","allow_numbers":true,"min_length":1,"severity":"error"}`):
  - `test_pascal_case_passes_pascal_case_rule` (PascalCase passes)
  - `test_camel_case_fails_pascal_case_rule` (camelCase fails)
  - `test_snake_case_fails_pascal_case_rule` (snake_case fails)
  - `test_inconsistent_case_fails_pascal_case_rule` (inconsistentCase fails)
- `TestNamePatternCamelCase` (the same with `camelCase`):
  - `test_camel_case_passes_camel_case_rule`
  - `test_pascal_case_fails_camel_case_rule`
  - `test_snake_case_fails_camel_case_rule`
- `TestNamePatternSnakeCase` (`snake_case`):
  - `test_snake_case_passes_snake_case_rule`
  - `test_pascal_case_fails_snake_case_rule`
  - `test_camel_case_fails_snake_case_rule`
- `TestNamePatternKebabCase` (`{"target_node_types":["component"],"convention":"kebab-case","allow_numbers":true,"min_length":1}`):
  - `test_kebab_case_passes_kebab_case_rule` (kebab-case gives 0 errors)
- `TestNamePatternMultipleNodeTypes`:
  - `test_components_pascal_case_properties_camel_case`: PascalCase, `{"target_node_types":["component","property"],"convention":"PascalCase","node_type_specific_rules":{"component":{"convention":"PascalCase","min_length":1},"property":{"convention":"camelCase","min_length":1}}}`. Smoke test.
  - `test_components_and_custom_methods_same_convention`: camelCase, `{"target_node_types":["component","custom_method"],"convention":"camelCase","min_length":3}`. Smoke test.
  - `test_different_conventions_per_node_type`: PascalCase, `{"target_node_types":["component","custom_method","message_handler"],"convention":"camelCase","node_type_specific_rules":{"component":{"convention":"PascalCase","min_length":1},"custom_method":{"convention":"camelCase","min_length":3},"message_handler":{"convention":"snake_case","min_length":4}}}`. Smoke test.
- `TestNamePatternMixedCase` (MixedCase, `{"target_node_types":["component"],"node_type_specific_rules":{"component":{"pattern":"^([A-Z][a-zA-Z0-9]*|[A-Z][A-Z0-9_]*)$","pattern_description":"PascalCase or SCREAMING_SNAKE_CASE","suggestion_convention":"PascalCase","min_length":3,"severity":"error"}}}`):
  - `test_pascal_case_passes`: no error mentions PascalCaseButton.
  - `test_screaming_snake_case_passes`: no error mentions SCREAMING_SNAKE_CASE_LABEL.
  - `test_mixed_with_numbers_passes`: none for DAT_02_TT_123_A, MyComponentWithNumbers123, UPPER_WITH_NUMBERS_456.
  - `test_camel_case_fails`: camelCaseInvalid is flagged and its message contains `"doesn't follow PascalCase or SCREAMING_SNAKE_CASE"`.
  - `test_snake_case_fails`: snake_case_invalid is flagged.
  - `test_kebab_case_fails`: kebab-case-invalid is flagged.
  - `test_mixed_pascal_snake_fails`: Mixed_PascalCase_Invalid is flagged.
  - `test_suggestions_provided`: the camelCaseInvalid message contains `"suggestion:"` and `"CamelCaseInvalid"`.
  - `test_abbreviations_in_pascal_case`: no errors for APIClient, HTTPSConnection, MyAPIHandler, XMLHTTPRequest, IOModuleController.
  - `test_abbreviations_in_screaming_snake_case`: none for API_CLIENT_HTTP, HTTP_API_URL, TT_DAT_API_123, CPU_GPU_MONITOR.
- `TestNamePatternEdgeCases` (all on PascalCase, all smoke tests). Kwargs per test:
  - `test_min_length_validation`: `{"target_node_types":["component"],"convention":"PascalCase","min_length":5}`
  - `test_max_length_validation`: `{"target_node_types":["component"],"convention":"PascalCase","min_length":1,"max_length":10}`
  - `test_numbers_allowed_vs_disallowed`: `{"target_node_types":["component"],"convention":"PascalCase","allow_numbers":true}` and the same with `false`
  - `test_forbidden_names`: `{"target_node_types":["component"],"convention":"PascalCase","forbidden_names":["Button","Label","Panel"]}`
  - `test_custom_abbreviations`: `{"target_node_types":["component"],"convention":"PascalCase","allowed_abbreviations":["API","HTTP","XML"],"auto_detect_abbreviations":false}`
  - `test_skip_names`: `{"target_node_types":["component"],"convention":"PascalCase","skip_names":["root","main","container"]}`
  - `test_custom_pattern`: `{"target_node_types":["component"],"custom_pattern":"^(btn|lbl|pnl)[A-Z][a-zA-Z0-9]*$","min_length":4,"max_length":25}`
- `TestNamePatternSpecificNodeTypes` (PascalCase, smoke tests):
  - `test_custom_method_naming`: `{"target_node_types":["custom_method"],"convention":"camelCase","min_length":3,"forbidden_names":["method","function","temp"]}`
  - `test_message_handler_naming`: `{"target_node_types":["message_handler"],"convention":"snake_case","min_length":4,"forbidden_names":["handler","msg","temp"]}`
  - `test_property_naming_camel_case`: `{"target_node_types":["property"],"convention":"camelCase","min_length":2,"forbidden_names":["temp","tmp","test","data"]}`
- `TestStandardNamingConventions`:
  - `test_standard_component_property_conventions`: PascalCase and camelCase, the "standard" config with property `skip_names ["id","x","y","z"]`. Smoke test.
  - `test_mixed_node_types_with_appropriate_conventions`: PascalCase, `{"target_node_types":["component","property","custom_method","message_handler"],"convention":"PascalCase","node_type_specific_rules":{"component":{"convention":"PascalCase","min_length":1},"property":{"convention":"camelCase","min_length":1},"custom_method":{"convention":"camelCase","min_length":3},"message_handler":{"convention":"camelCase","min_length":2}}}`. Smoke test.
  - `test_pascal_case_components_camel_case_properties`: PascalCase, `{"target_node_types":["component","property"],"node_type_specific_rules":{"component":{"convention":"PascalCase","severity":"error"},"property":{"convention":"camelCase","severity":"warning"}}}`. Exactly 4 warnings with patterns `"doesn't follow camelCase"` and `"property"` (CustomViewParam, ViewParam, RootCustomParam, ComponentCustomParam), and 0 errors.
  - `test_camel_case_components_pascal_case_properties_fails`: the same kwargs on camelCase. 1 error with patterns `"doesn't follow PascalCase"` and `"component"` (iconCamelCase). 2 warnings with patterns `"snake_case_test_custom"` and `"snake_case_test_param"` (properties that exist only in propConfig).
  - `test_auto_derived_target_node_types`: PascalCase, `{"node_type_specific_rules":{"component":{"convention":"PascalCase","severity":"warning"},"property":{"convention":"camelCase","severity":"warning"}}}` with no `target_node_types`. 4 warnings (`"doesn't follow camelCase"`, `"property"`) and 0 errors.
- `TestNamePatternArrayProperties` (inline, `{"target_node_types":["property"],"convention":"camelCase","severity":"error"}`):
  - `test_array_property_validated_once`: view `custom.MyArrayProperty=["item1","item2","item3"]`. Exactly 1 error, containing `MyArrayProperty` and not `[0]`, `[1]` or `[2]`.
  - `test_array_property_with_valid_name`: `myArrayProperty` gives 0 errors.
- `TestNamePatternCSSProperties` (inline, same kwargs):
  - `test_css_properties_in_style_should_pass`: `root.props.style` keys `touch-action`, `background-color`, `font-size`, `z-index` give 0 errors.
  - `test_css_properties_in_element_style_should_pass`: child FlexRepeater `props.elementStyle` keys `display`, `flex-direction`, `align-items` give 0 errors.
  - `test_regular_properties_still_validated`: `custom.BadPropertyName` and `custom["another-bad-name"]` fail (more than 0 errors).
- `TestNamePatternSVGProperties` (inline, same kwargs):
  - `test_svg_path_properties_should_pass`: child Line1 (`ia.shapes.svg`) with `props.elements[0].d` (an SVG path) and `type "path"`, plus `position` x, y, width, height, gives 0 errors.
  - `test_svg_paths_skipped_but_other_props_validated`: `custom.BadPropertyName` plus top-level `props.elements.d` gives exactly 1 error, containing `BadPropertyName` and not `props.elements.d`.
- `TestNamePatternPositionProperties` (inline, same kwargs):
  - `test_position_properties_should_pass`: Button and Label children with `position.x/y/width/height`, and root `position x, y`, give 0 errors.
  - `test_position_properties_with_custom_properties`: `custom.BadPropertyName` plus `root.position x, y` gives exactly 1 error, containing `BadPropertyName` and not `position.x` or `position.y`.
- `TestNamePatternPerNodeTypeSeverity`:
  - `test_component_error_severity`: inline child `badComponent`, `{"node_type_specific_rules":{"component":{"convention":"PascalCase","severity":"error"}}}`. 1 error containing `badComponent` and 0 warnings.
  - `test_property_warning_severity`: inline `custom.BadPropertyName`, `{"node_type_specific_rules":{"property":{"convention":"camelCase","severity":"warning"}}}`. 0 errors and 1 warning containing `BadPropertyName`.
  - `test_mixed_severity_per_node_type`: both, with component as error and property as warning. 1 error containing `badComponent` and `"component"`, and 1 warning containing `BadPropertyName` and `"property"`.
  - `test_all_node_types_with_different_severities`: PascalCase, `{"node_type_specific_rules":{"component":{"convention":"PascalCase","severity":"error"},"property":{"convention":"camelCase","severity":"warning"},"message_handler":{"convention":"kebab-case","severity":"warning"},"custom_method":{"convention":"snake_case","severity":"error"}}}`. Smoke test.

#### unit/test_fix_framework.py
Helpers: `make_view_json(children)` gives `root {children, meta root, props {}, type ia.container.flex}`. `make_component(name, type, children, props, propConfig)`. The NamePattern fix config is `{"convention":"PascalCase","target_node_types":["component"],"severity":"warning"}`.
- `TestFixOperations`:
  - `test_create_set_value_operation`: `FixOperation(operation, json_path, old_value, new_value, description)` stores its fields.
  - `test_create_string_replace_operation`: stores `old_substring` and `new_substring`.
  - `test_format_path`: the output contains `root`, `[0]`, `meta`, `name`.
  - `test_create_safe_fix`: `Fix(rule_name, violation_message, description, operations, is_safe=True)` has `safety_notes None`.
  - `test_create_unsafe_fix`: stores `safety_notes`.
- `TestPathTranslator` (BadButton and GoodLabel children):
  - `test_root_component_path`: `'root.root'` maps to `['root']`.
  - `test_child_component_path`: `'root.root.children[0].BadButton'` maps to `['root','children',0]`.
  - `test_component_name_path`: gives `[...,'meta','name']`.
  - `test_get_value`: returns `'BadButton'`.
  - `test_set_value`: modifies the data in place.
  - `test_get_all_component_paths`: includes `root.root`, `root.root.children[0].BadButton`, `root.root.children[1].GoodLabel`.
  - `test_string_replace_at`: returns True and replaces the substring.
  - `test_string_replace_at_no_match`: returns False.
  - `test_delete_value`: removes the dict key and keeps the others.
  - `test_delete_value_missing_key`: KeyError.
  - `test_delete_value_rejects_list_element`: KeyError for `['root','children',0]`.
  - `test_nested_children_path`: `root.root.children[0].Container1.children[0].NestedButton` maps to `['root','children',0,'children',0,'meta','name']`.
  - `test_property_value_path`: `...BadButton.props.text` maps to `['root','children',0,'props','text']`.
- `TestFixEngine` (badButton, GoodLabel):
  - `test_apply_safe_fix`: applied 1, skipped 0, renamed.
  - `test_skip_unsafe_fix_in_safe_mode`: applied 0, skipped 1, unchanged.
  - `test_apply_unsafe_fix_when_allowed`: `safe_only=False` applies it.
  - `test_apply_delete_key_fix`: DELETE_KEY on `['custom','unusedProp']` and `['propConfig','custom.unusedProp']` is applied as 1.
  - `test_delete_key_missing_target_skips_fix`: skipped 1 and `skip_reason` contains `'error'`.
  - `test_dry_run_does_not_modify`: `dry_run` reports applied 1 and the data is unchanged.
  - `test_rule_filter`: `rule_filter=['OtherRule']` gives skipped 1 with `'filtered'` in the reason.
  - `test_conflict_detection`: two fixes on the same path give 1 conflict with `fix_a` and `fix_b` set to those fixes.
  - `test_no_conflicts_different_paths`: 0 conflicts.
- `TestReferenceFinder` (`ComponentReferenceFinder(flattened, translator)`):
  - `test_find_expression_reference`: `props.text "{../MyButton.props.text}"` gives `ref_type 'expression'`.
  - `test_find_property_binding_reference`: `props.bindingRef "../Switch1.props.selected"` gives `'property_binding'`.
  - `test_find_script_reference`: an event script with `"\tself.getSibling('TargetButton').props.text"` gives `'script'`, `rewritable True`, `old_text "getSibling('TargetButton')"`.
  - `test_navigation_call_outside_script_key_is_safety_only`: the same text in `props.text` gives `'free_text'`, `rewritable False`, and `build_rename_operations` returns `[]`.
  - `test_no_references_for_unreferenced_component`: `[]`.
  - `test_has_self_name_binding_expression`: component `propConfig["meta.tooltip.text"]` expr `{this.meta.name}` gives True.
  - `test_has_self_name_binding_property`: `propConfig["props.text"]` property path `this.meta.name` gives True.
  - `test_no_self_name_binding`: False.
  - `test_build_rename_operations`: STRING_REPLACE with `old_substring '{../MyButton.props.text}'` and `new_substring '{../NewButton.props.text}'`. The whole reference is replaced, not just the bare name.
- `TestNamePatternRuleFixes`:
  - `test_generates_safe_fix_for_unreferenced_component`: 1 operation, SET_VALUE `badButton` to `BadButton`, safe.
  - `test_generates_unsafe_fix_for_referenced_component`: a label references it, so the fix is unsafe, `safety_notes` is set and there is more than 1 operation.
  - `test_no_fix_when_name_is_valid`: 0 fixes.
  - `test_no_fix_without_fix_context`: 0 fixes.
  - `test_generates_unsafe_fix_for_self_name_binding`: unsafe and `'this.meta.name'` in `safety_notes`.
  - `test_generates_unsafe_fix_for_self_name_and_references`: notes contain `'this.meta.name'` and `'reference'`.
  - `test_fix_skips_root_component`: no fix description mentions root.
- `TestEndToEndFixApplication`:
  - `test_full_safe_rename_pipeline`: applied 1, only badButton renamed.
  - `test_full_unsafe_rename_with_references`: the label text now contains `BadButton` and not `badButton`.
  - `test_write_and_read_back`: the fix survives `write_json_file` and `read_json_file`.
  - `test_safe_only_skips_unsafe_fixes`: applied 0 and skipped 1.
- `TestRenameUniquenessGuard`:
  - `test_no_fix_when_suggestion_collides_with_existing_sibling`: siblings `'My Button'` and `'MyButton'` give 0 fixes.
  - `test_violation_message_reports_collision`: the warning for `My Button` contains `"already in use by a sibling"`.
  - `test_convergent_suggestions_generate_only_one_fix`: `'my button'` and `'My Button'` give the rename target `MyButton` exactly once.
  - `test_fix_generated_when_no_collision`: `My Button` with `OtherButton` gives 1 fix with `new_value 'MyButton'`.
  - `test_collision_in_nested_container_scope`: `Panel/My Button` with top-level `MyButton` gives 1 rename fix.
- `TestReferenceGrammarCoverage` (`Target Component` button, plus `Consumer` with `propConfig["props.text"].binding`):
  - `test_detects_absolute_root_expression`: `{/root/Target Component.props.value}` gives 1 rewritable ref.
  - `test_detects_absolute_root_property_path`: property path `/root/Target Component.props.value` gives 1.
  - `test_detects_single_dot_expression`: `{./Target Component.props.value}` gives 1.
  - `test_detects_single_dot_property_path`: gives 1.
  - `test_absolute_rewrite_preserves_path_structure`: `{/root/Target Component/Detail.props.text}` with rename to `TargetComponent` gives 1 operation with `new_substring '{/root/TargetComponent/Detail.props.text}'`.
  - `test_view_keyword_reference_is_not_a_component_reference`: `{view.custom.paths}` gives no refs for `view` or `paths`.
  - `test_runscript_expression_blocks_rewrite`: `runScript("shared.nav.focus('Target Component')")` gives 1 ref with `rewritable False`.
- `TestRewriteCorruptionScenarios` (with `apply_fixes(safe_only=False)`):
  - `test_scenario_a_overlapping_names_both_renamed_and_rewritten`: `data label` and `data label 2` plus expression `{../data label.props.text} + {../data label 2.props.text}`. Applied 2, names become DataLabel and DataLabel2, and the expression becomes `{../DataLabel.props.text} + {../DataLabel2.props.text}`.
  - `test_scenario_b_literal_text_preserved`: `"'pump status: ' + {../pump status.props.text}"` becomes `"'pump status: ' + {../PumpStatus.props.text}"`.
  - `test_scenario_c_script_comments_and_literals_preserved`: the script gets `self.getSibling('DataLabel')` but keeps the comment `# update the data label widget` and `sendMessage('data label')`.

#### unit/test_bad_component_reference.py (BadComponentReferenceRule)
Most tests use `create_mock_script` with indented triple-quoted scripts that contain `def` lines.
- `test_detects_get_sibling`: BadComponentReferences, `{}`. More than 0 errors and one contains `".getSibling("`.
- `test_detects_get_parent`: transform with `self.getParent()` gives exactly 1 error containing `".getParent("`.
- `test_detects_get_child`: event_handler with `self.view.getChild("Container").getChild("Label")` gives 1 error with `".getChild("`.
- `test_detects_multiple_methods_reports_once`: custom_method with getSibling, getParent and getChild gives 1 error (one per script).
- `test_ignores_good_practices`: message_handler with `self.view.custom.*`, `self.props.text` and `system.tag.writeBlocking` gives 0.
- `test_case_sensitivity_option`: `{"case_sensitive":false}`, `component.GETSIBLING("test")` gives 1.
- `test_custom_forbidden_methods`: `{"forbidden_patterns":[".getSibling(",".customBadMethod("]}`, a script with `.customBadMethod(` and `.getParent(` gives 1 error containing `".customBadMethod("`.
- `test_empty_script_content`: an empty message handler gives 0.
- `test_script_types_coverage`: message_handler, custom_method, transform and event_handler, each with `self.getSibling("target")`, give 1 each.
- `test_severity_configuration_error`: `{"severity":"error"}` gives 1 error and 0 warnings.
- `test_severity_configuration_warning`: `{"severity":"warning"}` with getParent gives 0 errors and 1 warning.
- `TestBadComponentReferenceEdgeCases.test_method_in_comments_not_flagged`: a `# ... .getSibling() ...` comment **is** flagged (1 error). The name says otherwise; this is accepted current behaviour.
- `test_method_in_string_literals`: `"Don't use .getSibling() method"` in a string literal is flagged (1).
- `TestBadComponentReferenceBindingNodes` (inline `_view_with_root_propconfig`: the root `ia.container.coord` carries the propConfig and `props {}`):
  - `test_flags_parent_traversal_in_property_binding`: `props.text` property path `../OtherButton.props.text` gives 1 error containing `"Property Binding"`.
  - `test_flags_current_container_traversal_in_property_binding`: `./ChildButton.props.text` gives 1.
  - `test_ignores_view_scoped_property_binding`: `view.custom.enabled` and `view.params.color` give 0.
  - `test_flags_traversal_in_expression_struct_binding`: expr-struct `{"struct":{"color":"{./ChildButton.props.fill}"},"waitOnAll":true}` gives 1.
  - `test_ignores_view_scoped_expression_struct_binding`: `{view.params.color}` gives 0.
  - `test_flags_parent_traversal_in_expression_binding`: `{../OtherButton.props.text}` gives 1.
  - `test_ignores_view_scoped_expression_binding`: gives 0.
  - `test_flags_traversal_in_property_change_script`: root `propConfig["custom.total"].onChange.script = "value = self.getSibling('StatusLabel').props.text"` gives 1 error with `".getSibling("`.

#### unit/structure/test_component_reference_validation.py (ComponentReferenceValidationRule `{}` unless noted)
Inline views are `{"custom":{},"params":{},"propConfig":<root.propConfig or {}>,"props":{"defaultSize":{"height":600}},"root":<components>}`, with root `ia.container.coord`.
- `test_valid_sibling_reference_in_expression`: Button2 `props.enabled` expr `{../Button1.props.text}` gives 0.
- `test_invalid_sibling_reference_in_expression`: `{../NonExistentButton.props.text}` gives 1 error containing `NonExistentButton` and (lowercased) `"expression"`.
- `test_valid_nested_path_in_expression`: `{../Container1/NestedButton.props.text}` gives 0.
- `test_valid_property_binding_sibling`: path `../SourceButton.props.enabled` gives 0.
- `test_invalid_property_binding`: `../MissingButton.props.enabled` gives 1 error with `MissingButton` and `"property binding"`.
- `test_valid_get_sibling_script`: event script `label = self.getSibling('StatusLabel')\n...` gives 0.
- `test_invalid_get_sibling_script`: `self.getSibling('NonExistentLabel')` gives at least 1 error and one contains `NonExistentLabel`. Duplicates are tolerated.
- `test_valid_chained_navigation`: `self.getSibling('Container1').getChild('NestedButton')` gives 0.
- `test_invalid_chained_navigation`: `.getChild('MissingButton')` gives 1 error with `MissingButton` and `"child"`.
- `test_multiple_levels_up_three_dots`: DeepButton inside TopContainer/MiddleContainer, `{..../TopContainer.props.style}`, gives 0. Four dots means 3 levels up, landing on root.
- `test_navigate_above_root_error`: a root child with `{.../SomethingAboveRoot.props.enabled}` gives 1 error containing `"above root"`.
- `test_bad_component_references_test_case`: the BadComponentReferences case gives more than 0 errors, with some containing `NestedButton4` and some containing `UnknownButton`.
- `test_configuration_flags_expression_disabled`: `{"validate_expressions":false}` with a broken expression gives 0.
- `test_configuration_flags_property_binding_disabled`: `{"validate_property_bindings":false}` gives 0.
- `test_configuration_flags_script_disabled`: `{"validate_scripts":false}` gives 0.
- `test_parent_navigation_in_chained_calls`: ChildButton inside Container1 with script `self.parent.parent.getChild('Container2')` gives 0.
- `test_valid_expression_with_custom_property`: `{../Switch1.custom.energized}` gives 0.
- `test_valid_property_binding_with_custom_property`: `../Sensor1.custom.value` gives 0.
- `test_invalid_expression_with_custom_property`: `{../NonExistentSwitch.custom.energized}` gives 1 error with `NonExistentSwitch`.
- `test_nested_custom_property_reference`: `{../Container1/Switch1.custom.state}` gives 0.
- `test_valid_absolute_reference_in_expression`: root `ia.container.flex` with `Filters/Zone Dropdown`, Consumer expr `{/root/Filters/Zone Dropdown.props.value}`, gives 0.
- `test_broken_absolute_reference_in_expression`: `{/root/Filters/Missing Dropdown.props.value}` gives 1 error containing `Missing Dropdown`.
- `test_broken_absolute_property_binding_path`: Consumer expr `{view.custom.unrelated}` plus `props.style.classes` property path `/root/Filters/Missing Dropdown.props.value` gives 1 error with `Missing Dropdown`.
- `test_view_scoped_chain_resolving_through_root_passes`: view-level `propConfig["params.wing"].onChange.script` `'\tself.getChild("root").getChild("Filters").getChild("Wing Dropdown").props.value = [currentValue]'` gives 0. In a view-scoped script, `self` is the view.
- `test_view_scoped_chain_flags_the_actually_missing_link`: `self.getChild("root").getChild("Data").getChild("Filters")...` gives 1 error containing `'Data'` (quoted).
- `test_valid_absolute_property_binding_path`: `/root/Filters/Zone Dropdown.props.value` gives 0.

#### unit/structure/test_component_reference_validation_edge_cases.py
`_container_with_binding`: Container1 (flex) owns a binding at `props.style` (default) and has child InnerButton.
- `TestSingleDotReferences`:
  - `test_valid_current_container_reference_in_expression`: `{./InnerButton.props.text}` gives 0.
  - `test_invalid_current_container_reference_in_expression`: `{./MissingButton.props.text}` gives 1 error with `MissingButton`.
  - `test_valid_current_container_reference_in_property_binding`: path `./InnerButton.props.text` gives 0.
  - `test_invalid_current_container_reference_in_property_binding`: `./MissingButton.props.text` gives 1 error with `MissingButton` and `"property binding"`.
  - `test_valid_reference_in_expression_struct_binding`: `props.data` expr-struct `{"label":"{./InnerButton.props.text}"}` gives 0.
  - `test_invalid_reference_in_expression_struct_binding`: `{./MissingButton...}` gives 1 error with `MissingButton`.
- `TestReferenceRulesNodeAlignment.test_both_rules_cover_same_reference_nodes`: `BadComponentReferenceRule().target_node_types == COMPONENT_REFERENCE_NODES`, and `ComponentReferenceValidationRule().target_node_types - {NodeType.COMPONENT} == COMPONENT_REFERENCE_NODES`. According to the goldens' `rule_coverage`, that set is custom_method, event_handler, expression_binding, expression_struct_binding, message_handler, property_binding, property_change_script, transform.
- `TestComponentReferenceValidationEdgeCases` (Container1 with InnerButton):
  - `test_multiple_references_in_one_expression_flags_only_broken`: `{./InnerButton.props.text} + {./Missing.props.text}` gives 1 error with `Missing`.
  - `test_expression_struct_reports_only_broken_member`: struct `{"good":"{./InnerButton...}","bad":"{./Missing...}"}` gives 1.
  - `test_standalone_getchild_to_own_child_resolves`: Container1 event `self.getChild('InnerButton')` gives 0.
  - `test_standalone_getchild_to_missing_child_detected`: `self.getChild('Missing')` gives 1 error with `Missing` and `"child"`.
  - `test_nested_single_dot_path_resolves`: an extra SubContainer/Deep with `{./SubContainer/Deep.props.text}` gives 0.
  - `test_nested_single_dot_path_with_missing_leaf_detected`: `{./SubContainer/Missing...}` gives 1 error containing `"SubContainer/Missing"`.
  - `test_property_change_script_broken_reference_detected`: Container1 `propConfig["custom.total"].onChange.script "value = self.getChild('Missing')"` gives 1.
  - `test_property_change_script_valid_reference_passes`: `getChild('InnerButton')` gives 0.
  - `test_view_scoped_bindings_never_flagged`: `{view.params.color}` and path `view.custom.enabled` give 0.

#### unit/scripts/test_property_change_scripts.py
Inline view: view-level `propConfig["custom.total"]={persistent:false,onChange:{enabled:true,script:"\treturn previousValue.value + currentValue.value + undefinedTotal"}}`, and child Label with `propConfig["custom.flag"].onChange.script "\treturn currentValue.value and undefinedFlag"`.
- `test_builder_emits_script_nodes_for_onchange`: `ViewModelBuilder().build_model(flattened)['scripts']` contains a ScriptNode at `"propConfig.custom.total.onChange"` and one whose path ends with `"propConfig.custom.flag.onChange"`.
- `test_pylint_rule_lints_onchange_script_bodies`: PylintScriptRule `{}`. `results.custom_formatted_errors["PylintScriptRule"]` contains `E0602` and `onChange`. `previousValue` and `currentValue` must not count as undefined, so the onChange wrapper declares them.

#### unit/properties/test_excessive_context_data.py (ExcessiveContextDataRule)
Inline view: `custom` as given, `props.defaultSize`, and a root coord with one label `Placeholder`.
- `test_long_string_value_flagged_with_defaults`: `custom.embeddedContent` of 10001 characters gives 1 error containing `"custom.embeddedContent"` and `"10001"`.
- `test_string_at_threshold_passes`: exactly 10000 gives no issues (the limit is exclusive).
- `test_threshold_is_configurable`: 150 characters with `{"max_value_length":100}` gives 1. With `{"max_value_length":200}` it gives 0.
- `test_nested_long_string_flagged`: `custom.config.template` gives 1 error containing `"custom.config.template"`.
- `test_each_oversized_value_reported_separately`: 2 errors.
- `test_long_string_outside_custom_ignored`: a 10001-character component `props.text` gives 0.
- `test_non_string_values_ignored`: `10**100` and `True` give 0.
- `test_severity_warning_downgrade`: `{"severity":"warning"}` gives 0 errors and 1 warning.

#### unit/properties/test_propconfig_only_properties.py
Inline view: `custom.chillers {}`, `custom.showFlow {D:false, FWR_1:true}`, `params.baseTagPath ""`; propConfig `custom.chillers {persistent}`, `custom.chillers.WingOrCore` (property binding), `custom.deviceName` (binding, persistent false), `custom.showFlow {persistent}`, `custom.showFlow.A` (expr `true`), `params.baseTagPath`.
- `test_model_emits_property_nodes_for_propconfig_only_paths`: `model['properties']` paths include `custom.chillers.WingOrCore`, `custom.deviceName`, `custom.showFlow.A`, `custom.showFlow.D`, `custom.showFlow.FWR_1`.
- `test_name_pattern_rule_flags_propconfig_only_properties`: `{"node_type_specific_rules":{"property":{"convention":"camelCase","min_length":2,"severity":"warning"}}}`. Some warning contains `"custom.chillers.WingOrCore"` and some contains `"custom.showFlow.A"` (too short). So messages carry the full property path.

#### unit/properties/test_property_access.py (PropertyAccessRule; in-process `_lint` without and `_lint_with_fix_context` with a translator)
`_view` gives `{custom, params, propConfig, root{children, meta root}}`. `_component` is `ia.display.label`.
- `TestAccessDetection`:
  - `test_inert_by_default`: with no `expected_access`, errors and warnings are `{}`.
  - `test_undeclared_access_is_public_and_flagged`: `custom.a` with no propConfig and `{"expected_access":"PRIVATE"}` gives 1 error containing `custom.a` and `PUBLIC`.
  - `test_protected_flagged_when_private_expected`: gives 1.
  - `test_private_passes_when_private_expected`: gives `{}`.
  - `test_private_flagged_when_public_expected`: `{"expected_access":"PUBLIC"}` with declared PRIVATE and an undeclared prop gives 1 error, for `custom.declared` only.
  - `test_params_and_props_never_flagged`: Table `props.data` with a binding, and `params.pageId`, give `{}`.
  - `test_component_custom_property_flagged`: Chart `custom.theme` gives an error containing `"root.root.children[0].Chart.custom.theme"`.
  - `test_propconfig_only_custom_property_flagged`: `propConfig["custom.transient"]` only gives 1.
  - `test_exempt_props_bare_and_qualified`: `{"exempt_props":["custom.theme","root.root.children[0].Chart.custom.other"]}` gives `{}`.
  - `test_exempt_props_by_bare_name`: `["data"]` exempts view and component `data`. 1 error remains, containing `"Chart.custom.theme"`.
  - `test_exempt_props_wildcards`: `["kpi*","root.root.children[?].Chart.custom.other"]` gives `{}`. Brackets are literal and `?` is a wildcard.
  - `test_exempt_props_name_does_not_match_substring`: `["data"]` does not exempt `kpiData` or `dataSet`, so 2 errors.
  - `test_component_named_custom_not_misread`: a component named `custom` with `props.text` gives `{}`.
  - `test_lowercase_expected_access_accepted`: `"private"` gives 1.
  - `test_invalid_expected_access_raises`: `"HIDDEN"` raises ValueError.
  - `test_severity_warning_downgrade`: gives 1 warning and 0 errors.
- `TestAccessFixGeneration`:
  - `test_fix_sets_access_on_existing_entry`: `propConfig custom.a {persistent:false}` gives 1 safe fix with op paths `{('propConfig','custom.a','access')}`, SET_VALUE `'PRIVATE'`. Afterwards `persistent` is still False.
  - `test_fix_creates_entry_for_plain_custom_prop`: an existing propConfig with another key gives op path `('propConfig','custom.a')`, and the result is `{'access':'PRIVATE'}`.
  - `test_fix_creates_propconfig_when_owner_has_none`: Chart `custom a, b` with no propConfig gives 2 fixes, both applied, and the component propConfig becomes `{'custom.a':{'access':'PRIVATE'},'custom.b':{'access':'PRIVATE'}}`. Sequential fixes on a newly created dict compose.
  - `test_public_expected_removes_access_key`: `{access:PRIVATE, persistent:false}` with PUBLIC expected deletes `access` and leaves `{'persistent':False}`.
  - `test_public_expected_removes_access_only_entry`: `{access:PRIVATE}` has its whole entry deleted (`('propConfig','custom.a')`), leaving propConfig `{}`.
  - `test_no_fixes_without_fix_context`: 1 error and no fixes.
  - `test_fixed_view_lints_clean`: after applying, a re-lint gives `{}`.
- `TestAccessFixScoping` (kitchen-sink view: DataTable with `props.data` bound to `view.custom.stagingData`, `props.params.configuring`, `custom.stagingData`; a component named `custom`; Container/DeepLabel `custom.deepProp`; view `custom.viewProp` and `params.pageId`):
  - `test_every_operation_targets_custom_propconfig`: every operation path contains `propConfig`, and the next segment starts with `custom.`. A newly created propConfig dict may only have `custom.*` keys.
  - `test_end_to_end_fix_only_changes_custom_propconfig_entries`: `skipped == []`, more than 0 applied, and a JSON diff shows changes only in `custom.*` propConfig entries. `params`, `custom`, table props, `props.data`, `props.params.configuring`, the `custom`-named component and `params.pageId` are all unchanged.
  - `test_no_fix_for_already_compliant_entries`: no errors and no fixes.
  - `test_no_fix_for_exempt_props`: no errors and no fixes.
  - `test_public_expected_never_touches_params_or_props_declarations`: only `('propConfig','custom.a')` is touched, and the Table's `props.data {access:PRIVATE}` stays.

#### unit/properties/test_property_persistence.py (PropertyPersistenceRule)
- `TestPersistenceDetection`:
  - `test_inert_by_default`: gives `{}`.
  - `test_bound_persistent_property_flagged`: `{"expected_persistent":false}` with an expr-bound `custom.a` that has `persistent:true` gives 1 error containing `custom.a`.
  - `test_absent_persistent_flag_treated_as_persistent`: gives 1.
  - `test_bound_nonpersistent_property_passes`: gives `{}`.
  - `test_unbound_property_ignored`: `{persistent:true, access:PRIVATE}` with no binding gives `{}`.
  - `test_component_level_binding_flagged`: gives an error containing `"root.root.children[0].Label.custom.compProp"`.
  - `test_severity_warning_downgrade`: gives a warning.
- `TestBindingTypeExemptions`:
  - `test_tag_bindings_exempt_by_default`: tag bindings in `direct`, `indirect` and `expression` modes give `{}`.
  - `test_empty_exemptions_include_tag_bindings`: `{"exempt_binding_types":[]}` gives 1.
  - `test_tag_mode_specific_exemption`: `["tag.indirect"]` leaves only `custom.direct` flagged.
  - `test_other_binding_type_exemption`: `["expr"]` gives `{}`.
  - `test_unknown_exemption_token_raises`: `["indirect_tag"]` raises ValueError.
  - `test_invalid_expected_persistent_raises`: `"false"` (a string) raises ValueError.
- `TestScopes`:
  - `test_props_scope_never_checked`: component `props.text` bound and persistent gives `{}`.
  - `test_params_scope_never_checked`: `params.pageId` bound gives `{}`.
- `TestExpectedPersistentTrue`:
  - `test_nonpersistent_bound_property_flagged`: an error containing `"not persistent"`.
  - `test_persistent_without_default_value_flagged`: no `custom.a` value gives an error containing `"no stored default value"`.
  - `test_persistent_with_default_value_passes`: `custom.a=42` passes.
  - `test_no_fix_generated_for_expected_true`: 1 error and 0 fixes.
- `TestPersistenceFixGeneration`:
  - `test_fix_sets_persistent_false_and_removes_stale_value`: 1 safe fix with operations SET_VALUE `('propConfig','custom.a','persistent')` to `False` and DELETE_KEY `('custom','a')`. After applying, `custom` has no `a`.
  - `test_fix_without_stored_value_only_touches_propconfig`: only the persistent operation.
  - `test_component_level_fix_targets_component_propconfig`: paths `('root','children',0,'propConfig','custom.compProp','persistent')` and `('root','children',0,'custom','compProp')`.
  - `test_nested_children_deleted_parent_kept_as_empty_dict`: `custom.dates.startedDate` and `completedDate` bound give 2 applied fixes, and `custom == {'dates': {}}`.
  - `test_nested_child_deletion_keeps_unbound_siblings`: results in `{'dates':{'label':'Production Run'}}`.
  - `test_no_fixes_without_fix_context`.
  - `test_fixed_view_lints_clean`: a re-lint after applying gives no errors or warnings.

#### unit/properties/test_unused_custom_properties_fix.py (UnusedCustomPropertiesRule `{}` in fix mode)
- `test_safe_fix_for_unused_view_custom_property`: 1 safe fix, all DELETE_KEY, description contains `unusedProp`, paths `{('custom','unusedProp'),('propConfig','custom.unusedProp')}`.
- `test_fix_for_propconfig_only_property`: `custom.ghostProp` only in propConfig gives path `{('propConfig','custom.ghostProp')}`.
- `test_no_fix_for_unused_view_parameter`: 1 error and 0 fixes.
- `test_fix_for_component_custom_property_resolves_child_index`: the second child TestButton gives paths with index 1.
- `test_fix_deletes_nested_propconfig_children`: adds `('propConfig','custom.unusedObj.child')`.
- `test_prefix_named_sibling_property_is_untouched`: `prop` and `propLonger` give 2 separate fixes. The description's first single-quoted token is the prop name.
- `test_no_fix_when_property_has_onchange_script`: 1 error and 0 fixes.
- `test_empty_binding_dict_credits_property`: `binding: {}` gives no error and no fix.
- `test_build_delete_operations_guard_refuses_bound_entries`: `rule._build_delete_operations([], 'custom', 'weird')` with an entry that has a binding returns `([], False)`.
- `test_bound_properties_get_no_violations_and_no_fixes`: query and http bindings give nothing.
- `test_props_subtree_with_literal_custom_key_never_fixed`: Chart `props.seriesStyle.custom.strokeWidth` gives 0 fixes and is untouched. The violation count is not asserted.
- `test_embedded_view_pass_on_params_never_deleted`: `ia.display.view` with `props.params.unusedRef` and `propConfig["props.params.color"]`, plus view `params.reallyUnused`, gives 1 error, 0 fixes, everything kept.
- `test_empty_dict_component_custom_flagged_and_fixed`: `custom.emptyMeta {}` gives 1 error and a fix at path `('root','children',0,'custom','emptyMeta')`, leaving `custom == {}`.
- `test_dict_valued_component_custom_flagged_and_fixed`: `cfg {a, b:{c}}` gives a single delete of `('root','children',0,'custom','cfg')`.
- `test_dict_valued_component_custom_with_referenced_child_kept`: script `print(self.custom.cfg.a)` gives no error and no fix.
- `test_empty_dict_view_custom_without_propconfig_flagged_and_fixed`: path `('custom','emptyMeta')`.
- `test_empty_list_component_custom_flagged_and_fixed`: `emptyList []` gives 1 fix and `custom {}`.
- `test_no_fixes_without_fix_context`.
- `test_no_fixes_for_used_properties`: a binding placed **inline in `props.text.binding`** (type `"expression"`) referencing `{view.custom.usedProp}` gives nothing.
- `TestUnusedCustomPropertiesFixApplication.test_apply_fixes_and_relint_clean`: unusedProp, ghostProp, compUnused and unusedParam give 3 safe fixes, all applied, 0 skipped, 0 conflicts. Afterwards `list(propConfig) == ['params.unusedParam']`, and a re-lint gives 1 error containing `unusedParam`.
- `test_all_generated_fixes_are_safe`: 1 fix, and with `safe_only=False` the param survives.

#### unit/test_unused_custom_properties.py (UnusedCustomPropertiesRule `{}`, detection only, all inline)
Each test gives the expected error count, then the inline shape and patterns.
- `test_unused_view_custom_property`: 1 error. `custom.unusedViewProp`, root with no type. Patterns `unusedViewProp`, `"never referenced"`.
- `test_unused_component_custom_property`: 1 error. Child `custom.unusedComponentProp`.
- `test_used_custom_property_in_binding`: 0. Inline `props.text.binding` (type `"expression"`) `"{view.custom.usedProp} + {this.custom.usedComponentProp}"`.
- `test_used_custom_property_in_script`: 0. Non-standard shape: child with no type, `events.onClick.script` containing `self.view.custom.scriptProp`, root with no meta.
- `test_mixed_used_and_unused_properties`: 2 (`unusedProp`, `unusedComponentProp`).
- `test_view_parameters_usage`: 1 (`unusedViewParam`).
- `test_state_reset_between_files`: one engine runs two files. Each gives 1 error, the first naming `fileOneProp` and the second `fileTwoProp` only.
- `test_view_param_used_in_script_transform_with_self_params`: 0. Inline `props.text.binding` expr `None` with a script transform `return str(self.params.scriptValue)`.
- `test_view_param_used_in_script_transform_on_root`: 0. `root.props["custom.displayValue"].binding` (a dotted key inside props) with transform `self.params.rootParam`.
- `test_multiple_view_params_mixed_usage_in_scripts`: 1 (`unusedParam`). Transform uses `usedInTransform`, event handler uses `usedInEventHandler`.
- `test_custom_property_used_in_tag_binding`: 0. `tagPath "{view.custom.tagPrefix}/Value"`.
- `test_custom_property_used_in_message_handler`: 0. Child `scripts.messageHandlers` script `self.view.custom.messageValue`.
- `test_custom_property_used_in_custom_method`: 0. `self.view.custom.methodValue + str(self.custom.myProp)`.
- `test_custom_property_used_in_property_binding`: 0. Path `view.custom.sourceValue`.
- `test_custom_property_with_self_view_pattern_in_expression`: 0. `{self.view.custom.selfViewProp} + {self.view.params.selfViewParam}`.
- `test_output_param_with_binding`: 0. `params.outputData` output with property binding, `custom.dataSource` expr-bound.
- `test_output_param_without_binding`: 1 (`outputWithoutBinding`).
- `test_output_param_with_tag_binding`: 0. Indirect tag binding with `references.tagPath "{view.params.tagPath}"`. Both params count as used.
- `test_custom_property_with_binding_not_referenced`: 0. A bound custom counts as used, and its expression credits `inputA` and `inputB`.
- `test_unused_input_param`: 1 (`unusedInput`). Component `propConfig["props.text"]` expr `{view.params.usedInput}`.
- `test_mixed_param_scenarios`: 3 (`unusedInput`, `unusedCustom`, `outputWithoutBinding`).
- `test_object_custom_property_with_bound_children`: 0. `custom.network {}` with an onChange script reading `self.custom.network.nat1/nat2`, and children `nat1` and `nat2` tag-bound.
- `test_object_custom_property_with_referenced_child_only`: 0. `custom.settings.timeout` read via `self.view.custom.settings.timeout`.
- `test_object_component_custom_property_with_referenced_child`: 0. `{this.custom.config.color}`.
- `test_unused_object_custom_property_still_flagged`: 1 (`unusedNetwork`). The parent and child propConfig have only `persistent`.
- `test_object_child_binding_does_not_credit_sibling_prefix`: 1. `custom.net` is flagged while `custom.network` is credited through a bound child (`custom.network.nat1`). Pattern `"net"`.
- `test_view_object_custom_nested_child_in_event_script`: 0. A component event script `x = self.custom.network.nat1` credits the view's `custom.network` (a nested-child access through `self.custom` credits the view-level object).
- `test_plain_self_custom_does_not_credit_view_level_scalar`: 1 (`network`). A component script with bare `self.custom.network`.
- `test_object_child_referenced_in_unmodeled_onchange_script`: 0. View `propConfig["custom.network"].onChange` reads `self.custom.network.nat1`.
- `test_view_custom_referenced_via_self_custom_in_view_scope_onchange`: 0. View onChange on `custom.supervisor` reads `self.custom.devices` and `self.custom.supervisor`.
- `test_view_param_referenced_via_self_params_in_view_scope_script`: 0. View-level `custom.derived` binding transform reads `self.params.threshold`.
- `test_view_custom_referenced_via_self_custom_in_view_event_handler`: 0. Top-level `events.system.onStartup` reads `self.custom.startupFlag`.
- `test_truly_unused_view_custom_still_flagged_with_view_scope_fix`: 1 (`trulyUnused`).
- `test_whole_object_view_params_forwarding_credits_all`: 0. `payload=self.view.params`.
- `test_whole_object_view_custom_forwarding_credits_all`: 0. `payload=self.view.custom`.
- `test_subscript_view_params_access_credits_only_that_member`: 1 (`rowIndex`). `self.view.params['sectionUuid']`.
- `test_single_member_access_still_credits_only_that_member`: 1 (`sibling`).
- `test_bare_self_params_at_component_scope_not_credited`: 1 (`forwarded`). Component `payload=self.params`.
- `test_subscript_view_params_access_with_whitespace_credits_only_that_member`: 1 (`rowIndex`). `self.view.params [ 'sectionUuid' ]`.
- `test_dynamic_subscript_view_params_access_credits_all`: 0. `self.view.params[key]`.
- `test_all_binding_types_credit_their_owner`: 0. Propconfig-only customs bound by expr, query, expr-struct, http and tag-history.
- `test_component_custom_with_unmodeled_binding_type_credited`: 0. Component `custom.data` with a query binding.
- `test_whole_params_object_property_binding_credits_all_params`: 0. Embedded view `props.params.binding` path `view.params` (inline in props).
- `test_whole_params_object_expression_binding_credits_all_params`: 0. Expression `{view.params}`.
- `test_static_params_on_embedded_view_component_not_treated_as_definitions`: 0. `props.params.staticChildParam`.
- `test_embedded_view_pass_on_params_are_not_definitions`: 1 (`reallyUnused`).
- `test_navigation_chain_custom_access_credited`: 0. `self.getSibling('Btn').custom.data`.
- `test_variable_held_component_custom_access_credited`: 0. `s = self.getSibling('Btn')\nprint(s.custom.data)`.
- `test_component_scope_quoted_subscript_custom_credited`: 0. `self.custom['data']`.
- `test_component_scope_dynamic_subscript_credits_all_component_customs`: 0. `self.custom[key]`.
- `test_non_identifier_prop_name_subscript_credited`: 0. `custom["my prop"]` read via `self.view.custom['my prop']`.
- `test_view_custom_not_credited_by_component_scope_dot_reference`: 1 (`viewOnly`). Component `self.custom.viewOnly`.

---

### 2.3 Fixture usage

| Case | Used by (file::test) | Rule and kwargs variants (verbatim, compact) |
|---|---|---|
| AllNodeTypes | test_golden_files (01/02/03), test_discovery::test_discover_test_cases | all rules `{}` (golden only) |
| AllScriptTypes | golden, discovery | same |
| BadComponentReferences | test_bad_component_reference::test_detects_get_sibling; structure/test_component_reference_validation::test_bad_component_references_test_case; golden; discovery. Also mentioned (not loaded) in a docstring in test_unused_custom_properties | BadComponentReferenceRule `{}` (errors > 0, one with `.getSibling(`); ComponentReferenceValidationRule `{}` (messages with `NestedButton4` and `UnknownButton`) |
| BadContextData | golden, discovery | golden only |
| camelCase | test_component_naming (Pascal and Snake fail, Camel pass, `test_components_and_custom_methods_same_convention`, `test_standard_component_property_conventions`, `test_camel_case_components_pascal_case_properties_fails`); test_warnings_vs_errors (all except `test_rule_passes_completely...`); test_script_linting::test_multiple_view_files; integration/test_multi_rule (`all_available_rules_together`, `error_aggregation`, `mixed_node_type_naming_standards`); configs (`pascal_case_negative`, `camel_case_positive`, `pascal_case_negative_warnings`, `pascal_case_negative_errors`, `name_pattern_warning_demo`); golden | NamePatternRule: `{"target_node_types":["component"],"convention":"PascalCase","allow_numbers":true,"min_length":1,"severity":"error"}`, `{"target_node_types":["component"],"convention":"camelCase","allow_numbers":true,"min_length":1,"severity":"error"}`, `{"target_node_types":["component"],"convention":"snake_case","allow_numbers":true,"min_length":1,"severity":"error"}`, `{"target_node_types":["component","custom_method"],"convention":"camelCase","min_length":3}`, `{"target_node_types":["component","property"],"convention":"PascalCase","node_type_specific_rules":{"component":{"convention":"PascalCase","min_length":1,"allow_numbers":true},"property":{"convention":"camelCase","min_length":1,"allow_numbers":true,"skip_names":["id","x","y","z"]}}}`, `{"target_node_types":["component","property"],"node_type_specific_rules":{"component":{"convention":"PascalCase","severity":"error"},"property":{"convention":"camelCase","severity":"warning"}}}`, `{"target_node_types":["component"],"convention":"PascalCase","min_length":1}`, `{"target_node_types":["component"],"convention":"PascalCase","min_length":1,"severity":"error"}`, `{"target_node_types":["component"],"convention":"PascalCase"}`, `{"convention":"PascalCase","allow_numbers":true,"min_length":1,"severity":"error"}`, `{"convention":"camelCase","allow_numbers":true,"min_length":1}`, `{"convention":"PascalCase","allow_numbers":true,"min_length":1,"severity":"warning"}`. PollingIntervalRule `{"minimum_interval":5000}`, `{"minimum_interval":1000}`. PylintScriptRule `{}` |
| ExpressionBindings | test_polling_interval (both TestPollingIntervalRule tests); test_binding_rules (both); test_warnings_vs_errors::test_polling_interval_rule_produces_errors; test_script_linting::test_multiple_view_files; test_multi_rule::test_all_available_rules_together; configs `expression_bindings_check` (10000 gives 5 errors) and `polling_interval_error_demo` (5000 gives 4 errors); golden | PollingIntervalRule `{"minimum_interval":10000}`, `{"minimum_interval":1000}`, `{"minimum_interval":5000}`, `{"minimum_interval":30000}`. PylintScriptRule `{}`. NamePatternRule `{"target_node_types":["component"],"convention":"PascalCase"}` (multi_rule) |
| ExtensionFunctions | golden, discovery | golden only |
| inconsistentCase | test_component_naming::test_inconsistent_case_fails_pascal_case_rule; config `inconsistent_case_negative`; golden | `{"target_node_types":["component"],"convention":"PascalCase","allow_numbers":true,"min_length":1,"severity":"error"}`, `{"convention":"PascalCase","allow_numbers":true,"min_length":1,"severity":"error"}` (3 errors) |
| kebab-case | test_component_naming::test_kebab_case_passes_kebab_case_rule; golden | `{"target_node_types":["component"],"convention":"kebab-case","allow_numbers":true,"min_length":1}` |
| LineDashboard | golden, discovery | golden only |
| MixedCase | test_component_naming::TestNamePatternMixedCase (all 10 tests); golden | `{"target_node_types":["component"],"node_type_specific_rules":{"component":{"pattern":"^([A-Z][a-zA-Z0-9]*\|[A-Z][A-Z0-9_]*)$","pattern_description":"PascalCase or SCREAMING_SNAKE_CASE","suggestion_convention":"PascalCase","min_length":3,"severity":"error"}}}` (the `\|` is a table escape for a plain `\|`-free `|`; the regex contains a single `|`) |
| PascalCase | test_component_naming (every fixture test except camelCase-, snake_case-, kebab-case-, MixedCase- and inconsistentCase-only ones); test_warnings_vs_errors (`passes_completely`, `backward_compatibility`); test_polling_interval::test_no_polling_expressions; test_binding_rules::test_multiple_binding_types; test_script_linting (`basic`, `multiple`); test_discovery::test_multiple_rules_together; test_golden_files::test_00 plus 01/02/03; integration/test_multi_rule (every test); test_cli_integration (`with_config_file`, `stats_only`); configs (`pascal_case_positive`, `perfect_view_no_issues`, `basic_script_check`) | NamePatternRule: `{"target_node_types":["component"],"convention":"PascalCase","allow_numbers":true,"min_length":1,"severity":"error"}`, the same with `"camelCase"` and `"snake_case"`, `{"target_node_types":["component","property"],"convention":"PascalCase","node_type_specific_rules":{"component":{"convention":"PascalCase","min_length":1},"property":{"convention":"camelCase","min_length":1}}}`, `{"target_node_types":["component","custom_method","message_handler"],"convention":"camelCase","node_type_specific_rules":{"component":{"convention":"PascalCase","min_length":1},"custom_method":{"convention":"camelCase","min_length":3},"message_handler":{"convention":"snake_case","min_length":4}}}`, `{"target_node_types":["component"],"convention":"PascalCase","min_length":5}`, `{"target_node_types":["component"],"convention":"PascalCase","min_length":1,"max_length":10}`, `{"target_node_types":["component"],"convention":"PascalCase","allow_numbers":true}`, `{"target_node_types":["component"],"convention":"PascalCase","allow_numbers":false}`, `{"target_node_types":["component"],"convention":"PascalCase","forbidden_names":["Button","Label","Panel"]}`, `{"target_node_types":["component"],"convention":"PascalCase","allowed_abbreviations":["API","HTTP","XML"],"auto_detect_abbreviations":false}`, `{"target_node_types":["component"],"convention":"PascalCase","skip_names":["root","main","container"]}`, `{"target_node_types":["component"],"custom_pattern":"^(btn\|lbl\|pnl)[A-Z][a-zA-Z0-9]*$","min_length":4,"max_length":25}`, `{"target_node_types":["custom_method"],"convention":"camelCase","min_length":3,"forbidden_names":["method","function","temp"]}`, `{"target_node_types":["message_handler"],"convention":"snake_case","min_length":4,"forbidden_names":["handler","msg","temp"]}`, `{"target_node_types":["property"],"convention":"camelCase","min_length":2,"forbidden_names":["temp","tmp","test","data"]}`, the "standard" component/property config (as in camelCase), `{"target_node_types":["component","property","custom_method","message_handler"],"convention":"PascalCase","node_type_specific_rules":{"component":{"convention":"PascalCase","min_length":1},"property":{"convention":"camelCase","min_length":1},"custom_method":{"convention":"camelCase","min_length":3},"message_handler":{"convention":"camelCase","min_length":2}}}`, `{"target_node_types":["component","property"],"node_type_specific_rules":{"component":{"convention":"PascalCase","severity":"error"},"property":{"convention":"camelCase","severity":"warning"}}}` (4 warnings, 0 errors), `{"node_type_specific_rules":{"component":{"convention":"PascalCase","severity":"warning"},"property":{"convention":"camelCase","severity":"warning"}}}` (4 warnings), `{"node_type_specific_rules":{"component":{"convention":"PascalCase","severity":"error"},"property":{"convention":"camelCase","severity":"warning"},"message_handler":{"convention":"kebab-case","severity":"warning"},"custom_method":{"convention":"snake_case","severity":"error"}}}`, `{"target_node_types":["component"],"convention":"PascalCase","min_length":1}`, the same with `"severity":"error"`, `{"target_node_types":"component","convention":"PascalCase"}`, `{"target_node_types":["component"],"convention":"PascalCase"}`, `{"target_node_types":["property"],"convention":"camelCase"}`, `{"target_node_types":["component","property"],"convention":"PascalCase","node_type_specific_rules":{"component":{"convention":"PascalCase"},"property":{"convention":"camelCase"}}}`, the multi_rule comprehensive config (2.2), `{"convention":"PascalCase","allow_numbers":true,"min_length":1}`, `{"convention":"PascalCase"}` (CLI). PollingIntervalRule `{"minimum_interval":10000}`, `{"minimum_interval":1000}`, `{"minimum_interval":5000}`. PylintScriptRule `{}`. CLI `--stats-only` (no config) |
| Pipes | integration/test_pipe_references (all 5); golden | ComponentReferenceValidationRule `{}` plus BadComponentReferenceRule `{}` together |
| PreferredStyle | golden, discovery | golden only |
| PylintViolations | golden, discovery | golden only (no PylintScriptRule test uses it) |
| snake_case | test_component_naming (Pascal fail, Camel fail, Snake pass); config `snake_case_positive`; golden | `{"target_node_types":["component"],"convention":"PascalCase","allow_numbers":true,"min_length":1,"severity":"error"}`, the same with camelCase and snake_case, `{"convention":"snake_case","allow_numbers":true,"min_length":1}` |
| Title Case | golden, discovery | golden only (directory name contains a space) |
| UPPER_CASE | golden, discovery | golden only |

---

### 2.4 Behaviours covered only by inline views

These need hand-written fixtures.

**UnusedCustomPropertiesRule.** All detection and fixing.
- View and component customs and params.
- Every crediting path: expression, tag including indirect references, property, query, expr-struct, http, tag-history, `binding {}`, transforms, message handlers, custom methods, view event handlers, onChange scripts, inline `props.*.binding`.
- Whole-object forwarding (`self.view.params` and `.custom`, `{view.params}`, path `view.params`).
- Literal and dynamic subscripts, with and without whitespace and non-identifier names.
- Nested-child crediting of parent objects and the prefix-sibling guard.
- View-scope vs component-scope `self.custom` and `self.params` boundaries.
- Navigation-chain and variable-held component custom access.
- Embedded view pass-on params.
- Output params with and without binding.
- Empty `{}` and `[]` customs.
- State reset across files.
- The fix policy: params never fixed, onChange props never fixed, bound entries refused, list-index resolution, nested propConfig cleanup, only safe fixes.

**NamePatternRule.**
- Array properties validated once.
- CSS keys in `style` and `elementStyle` skipped.
- SVG `props.elements[*].d` and `props.elements.d` skipped.
- `position.*` skipped.
- Per-node-type severity on single components and properties.
- propConfig-only properties (also partly in the camelCase fixture).
- Rename fixes (safe vs unsafe, `this.meta.name`, reference updates).
- Sibling-collision message and suppression.
- Convergent renames.
- Container-scoped uniqueness.
- Corruption scenarios (overlapping names, literals, script comments).
- Components under the top-level `props.children` (whitelist integration).
- Dict-keyed children (create_mock_view).

**ComponentReferenceValidationRule.**
- Single-dot `./` in expression, property and expr-struct bindings.
- Nested `./A/B`.
- `{..../X}` multi-level.
- `{.../X}` above root.
- Absolute `{/root/...}` and `/root/...`.
- `self.parent.parent.getChild`.
- Standalone `getChild`.
- Chained `getSibling().getChild()`.
- onChange scripts, component level and view level, including `self.getChild("root")` chains.
- `validate_expressions`, `validate_property_bindings` and `validate_scripts` flags.
- `.custom.` paths.
- Multiple refs per expression, and per-member expr-struct reporting.
- The node-set alignment with BadComponentReferenceRule. The BadComponentReferences fixture only covers NestedButton4 and UnknownButton; Pipes covers some `./` forms.

**BadComponentReferenceRule.**
- Every script type: message handler, custom method, transform, event handler, onChange.
- `.getParent(` and `.getChild(`.
- Once-per-script reporting.
- `case_sensitive`, `forbidden_patterns`, `severity`.
- Matches inside comments and string literals.
- Property, expression and expr-struct binding traversal, and view-scoped exclusions. Pipes covers two property-binding flags only.

**PylintScriptRule.**
- Mixed tabs and spaces error.
- `TransformScript.get_formatted_script` indentation.
- onChange script linting (E0602).
- Trailing-whitespace fix (strip, one per script, safe SET_VALUE, re-lint clean, reset between files).
- Category mapping, grouping and formatting, `_parse_pylint_output`.
- batch_mode.

Note that the PylintViolations fixture is not used by any rule test.

**Other rules.**
- ExcessiveContextDataRule `max_value_length` behaviour. The BadContextData fixture is golden-only.
- PropertyAccessRule: everything.
- PropertyPersistenceRule: everything.
- ExampleMixedSeverityRule: everything.

**Framework and CLI.**
- Fix framework: FixOperation and Fix, PathTranslator, FixEngine (apply, dry_run, rule_filter, conflicts, DELETE_KEY failure), ComponentReferenceFinder grammar and `has_self_name_binding`.
- `allow_fix` config parsing and gating.
- CLI fix modes and exit codes.
- `--files` with multiple paths and the deprecation warning.
- Whitelist load, generate and filter.
- Batch result cleanup and aggregation.
- Rule registry contract.
- `flatten_json` unit semantics (root list, top-level meta.name prefix, empty containers, unicode placeholders).

---

### 2.5 Notable assertions that reveal quirks or bugs

1. **Config-driven expectations never fail CI.** `integration/test_config_framework.py` only asserts that the counts are ints. The `ConfigurableTestRunner` in `fixtures/config_framework.py` is broken dead code: it uses an undefined `results` and missing `expected_count` and `actual_count` keys. That module also does `sys.path.insert(.../'../src')` relative to `fixtures/`, which points at the wrong place.
2. `tests/README.md` documents `--run-config` and `--tags`, which `test_runner.py` does not have, and `tests/configs/`, which is really `tests/integration/configs/`. `--test` and `--list` only see top-level `unit/test_*.py`.
3. **Flatten path convention.** Any dict with `meta.name` inserts the name as a segment, so paths look like `root.root...`, `root.root.children[0].Name...` and even a top-level `MyButton.meta.name`. Golden `flattened.json` keeps `"props": {}` as a leaf. The goldens compare the model only by section counts and sorted node paths, and stats only by 4 keys (`rule_coverage` is ignored).
4. **Message formats asserted as substrings.**

   NamePatternRule:
   - `"doesn't follow"`, `"doesn't follow PascalCase"`, `"doesn't follow camelCase"`, `"doesn't follow PascalCase or SCREAMING_SNAKE_CASE"` (uses `pattern_description`).
   - `"suggestion:"`.
   - The node type word `"component"` or `"property"`.
   - The full property path, e.g. `"custom.chillers.WingOrCore"`.
   - `"already in use by a sibling"`.

   UnusedCustomPropertiesRule:
   - `"never referenced"`.
   - The fix description contains `"Remove unused"` and the prop name in single quotes.

   ComponentReferenceValidationRule:
   - `"expression"`, `"property binding"` (lowercased check), `"not found as child"`, `"child"`, `"above root"`.
   - Names in single quotes, e.g. `'Data'`, `'Button'`.
   - The nested path `"SubContainer/Missing"`.

   BadComponentReferenceRule:
   - The pattern text, e.g. `".getSibling("`.
   - `"Property Binding"` (title case).

   PylintScriptRule:
   - `"mixes tabs and spaces for indentation"` and `"Use either tabs OR spaces consistently"`.
   - Grouped output `"Pylint - Error (E)"` and `"Pylint - Warning (W)"`, `"Line 5"`, the code.

   ExcessiveContextDataRule: the path `custom.embeddedContent` (no `root.` prefix for view-level paths) and the actual length `10001`.

   PropertyAccessRule: the path and the current mode, e.g. `PUBLIC`. Component paths look like `root.root.children[0].Chart.custom.theme`.

   PropertyPersistenceRule: `"not persistent"`, `"no stored default value"`.

   CLI output:
   - `"No style inconsistencies found"`.
   - `"| Skipped:"` (exactly once per fix run).
   - `"DELETE"`.
   - `"does not match any loaded rule"`.
   - `"Error creating rule UnusedCustomPropertiesRule"`.
   - `"allow_fix has no effect"`.
   - `"Files processed: N"`.
   - `"Deprecation"` (only when N >= 2 paths follow `--files`).
   - `"skipped"` (verbose whitelist).
   - `"usage:"`.
   - The help text must not contain `"use with --fix"`.
5. **Array index stripping.** NamePatternRule messages for array properties must not contain `[0]`, `[1]` or `[2]`; the property is validated once by its base name.
6. **BadComponentReferenceRule is plain substring matching.** It flags patterns inside comments and string literals (`test_method_in_comments_not_flagged` asserts 1 error despite its name). It reports at most once per script even when several patterns occur.
7. `test_invalid_get_sibling_script` tolerates duplicate errors for one `getSibling('NonExistentLabel')` (comment: "simple pattern + chained pattern"). Elsewhere exact counts of 1 are asserted (chained `getChild`, standalone `getChild`, onChange).
8. `test_props_subtree_with_literal_custom_key_never_fixed` says in a comment that the definition regex is over-broad and may flag `props.seriesStyle.custom.strokeWidth` as an unused custom. It only asserts that no fix is produced. `test_embedded_view_pass_on_params_never_deleted` has a stale docstring ("param-unsafe fix") but asserts 0 fixes.
9. **ExpressionBindings polling.** `now()` with no argument is a violation. At 5000 there are 4 errors (four `now()`); at 10000 there are 5 (four `now()` plus `now(5000)`). `now(11000)` never fires. The limit appears to be "interval < minimum".
10. **ExcessiveContextData threshold is exclusive.** Length 10000 passes and 10001 fails. Only string values under `custom.*` are checked.
11. **`create_rules_from_config({})` loads every registered rule**, so absent rules are treated as enabled. Statuses use `state` `"loaded"`/`"error"` and `detail` `None` or `"allow_fix=false"`. Bad `allow_fix` values (including `"false"`, `0`, `1`, `None`, `[]`, dict) disable only that rule and print to stdout.
12. **Engine result dicts omit empty rules.** For example `results.errors == {}`. `BaseIntegrationTest.run_multiple_rules` merges warnings then errors, so a rule's warnings are hidden when it also has errors.
13. **Unusual inline shapes the linter must tolerate:**
    - `create_mock_view`: no `root` key, `children` as a dict keyed by name under top-level `props`.
    - Whitelist integration: components under the view's top-level `props.children` list with a childless `root`. NamePatternRule must still flag `invalid_component`, and the warning-level result must give a non-zero exit.
    - Bindings written inline under `props.text.binding` instead of `propConfig`, still credited.
    - Root `props["custom.displayValue"]` (a dotted key).
    - `events.onClick.script` with no domain level.
    - A root with no meta or type.
    - `create_mock_script("transform")` puts the binding at the view-level `propConfig["root.children[0].props.text"]`.
    - Scripts are indented triple-quoted Python containing `def` lines.
14. `target_node_types` given as the string `"component"` (test_discovery) must be accepted. When it is omitted it is derived from `node_type_specific_rules`, and with neither it defaults to components only.
15. **Exit codes.** `--fix` gives 0 after it clears the only violation, because there is a single re-evaluation pass and the fix is applied once. `--fix-dry-run` and remaining param violations give a non-zero exit. The `--config` run on PascalCase only asserts the exit code is 0 or 1.
16. `TransformScript.get_formatted_script` emits `def transform(self, value):` (not the 3-argument Ignition signature) and indents with exactly one tab without double-indenting scripts that are already tab-indented. A comment-first or blank-first script is handled by scanning for the first real code line.
17. The trailing-whitespace fix also strips the last line (`"\ty = 2\t"` becomes `"\ty = 2"`) and is a single SET_VALUE of the entire script.
18. `PropertyAccessRule` exempt patterns are anchored globs where `[...]` is literal (`children[?]`). `expected_access` is case-insensitive. PUBLIC is the implicit default: removing it deletes the `access` key, or the whole entry if `access` was its only key.
19. `PropertyPersistenceRule` exemption tokens: `"tag"` is the default, `"tag.indirect"` and similar exempt one mode, and other binding types like `"expr"` work. `"indirect_tag"` raises. A missing `persistent` key means persistent.
20. Smoke-only tests assert nothing about behaviour (only `assertIsInstance(..., list)`): most of `TestNamePatternEdgeCases`, `TestNamePatternSpecificNodeTypes`, `test_polling_interval`, `test_binding_rules`, `test_script_linting` basic and multiple, and most of `test_multi_rule`. There is a comment noting that `test_event_handler_naming` was intentionally removed because event handler names are framework-defined.
21. `test_golden_files` claims it "runs first (alphabetically)", which is not true. Test methods are prefixed `test_00` to `test_03` for ordering within the class.
22. The pylint test files import through `src.ignition_lint...` while the rest use `ignition_lint...`, so the package is imported twice under two names.
23. `create_temp_view_file` temp files are never cleaned up in most unused-property and example tests. This is harmless but worth knowing.
