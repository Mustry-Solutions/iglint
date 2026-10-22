# ign-lint compatibility specification

This document specifies the observable behaviour of **ign-lint**
(`ignition-lint`, PyPI `ign-lint`, https://github.com/bw-design-group/ignition-lint,
MIT) so that iglint can support every feature without reading its code.

* Reference version: **v0.6.4** (tag `v0.6.4`, commit `dace01d`, 2026-08-31).
  `main` is identical to `v0.6.4`. Unreleased work on `dev` is summarised in
  [section 12](#12-unreleased-changes-on-dev-after-v064).
* Verified by running ign-lint 0.6.4 from PyPI with Python 3.13.16,
  pylint 4.1.3 and astroid 4.3.4 (the package pins `pylint = "*"`, so pylint
  findings depend on the installed pylint version).
* Projects that still pin **v0.6.3** are covered too: see
  [0.6.3 and 0.6.4](#063-and-064) below.
* "Bug" marks behaviour we believe is wrong. iglint is correct, not
  bug-compatible, but parity tests must know where outputs differ. Every
  bug listed here was reproduced; the probe views in
  `crates/iglint_linter/tests/fixtures/ign-lint/probes/` pin most of them.

### 0.6.3 and 0.6.4

v0.6.4 (compared on the PyPI wheels of both releases) changed only this;
every other module is identical:

| | v0.6.3 | v0.6.4 | iglint |
|---|---|---|---|
| `allow_fix` in a rule entry | Not read: ignored, whatever its value. | Must be a JSON boolean (else `Error creating rule {name}: allow_fix must be true or false; got {v!r}` and the rule does not run); `false` keeps the rule's findings but `--fix` / `--fix-dry-run` skip it; an explicit `--fix-rules` overrides it; `Note: allow_fix has no effect on {name} ...` for rules without fixes. The rule list printed at start-up shows `, allow_fix=false`. | 0.6.4 (`unfixable` after `iglint migrate`). |
| `ExcessiveContextDataRule` | Four detectors (array size, siblings, depth, data points); kwargs may also be passed positionally. | Adds the value-length detector (`max_value_length`, default 10000 characters, [5.7](#57-excessivecontextdatarule)); kwargs are keyword-only. A 0.6.4 configuration that sets `max_value_length` fails to create the rule in 0.6.3 (`TypeError`). | 0.6.4: long strings in view `custom` are reported. To match a 0.6.3 CI, set `max-value-length` very high in `iglint.toml` (or `max_value_length` in `rule_config.json`, which 0.6.3 itself rejects). |
| `PropertyAccessRule`, `PropertyPersistenceRule` | Do not exist: `Unknown rule in config: {name}`. | New, inert until configured ([5.8](#58-propertyaccessrule), [5.9](#59-propertypersistencerule)). | 0.6.4. |
| `--fix-rules` | Names that match no rule are ignored silently. | Prints a warning for names that match no loaded rule and for rules without fix support. | 0.6.4. |
| Node order of `propConfig`-only properties | Hash order of a set: varies between runs, so `--debug-nodes` / `--debug-output` and the order of some findings are not reproducible. | Sorted. | Sorted. |

Every configuration valid for 0.6.3 is valid for 0.6.4 and is read the same
way, except a non-boolean `allow_fix` (ignored by 0.6.3, a creation error in
0.6.4 and iglint). `iglint ign-lint` and `iglint migrate` accept both;
`iglint ign-lint --version` prints `ign-lint 0.6.4 (iglint <version>
compatibility mode)`.

Contents

1. [Pipeline overview](#1-pipeline-overview)
2. [Flattening](#2-flattening)
3. [Object model](#3-object-model)
4. [Rule framework](#4-rule-framework)
5. [Rules](#5-rules)
6. [Auto-fix](#6-auto-fix)
7. [Configuration file](#7-configuration-file)
8. [CLI](#8-cli)
9. [Output formats](#9-output-formats)
10. [Integrations](#10-integrations)
11. [Other user-visible behaviour](#11-other-user-visible-behaviour)
12. [Unreleased changes on dev after v0.6.4](#12-unreleased-changes-on-dev-after-v064)
13. [Bug list](#13-bug-list)
14. [Private corpus run (aggregates)](#14-private-corpus-run-aggregates)
15. [Appendix: pylint messages enabled by the bundled rcfile](#15-appendix-pylint-messages-enabled-by-the-bundled-rcfile)

---

## 1. Pipeline overview

For every selected file, sequentially, in one process:

1. Read the file as UTF-8 and parse it with Python `json.loads` (key order
   preserved, duplicate keys: last wins, `NaN`/`Infinity` accepted).
   Invalid JSON or a missing file aborts the **whole run** with exit code 1
   (see [8.6](#86-exit-codes)).
2. Flatten the JSON into an ordered map `path -> leaf value`
   ([section 2](#2-flattening)).
3. Build the object model (typed nodes) from the flattened map
   ([section 3](#3-object-model)).
4. Run every loaded rule, in registry order, over the node list. Some rules
   ignore the nodes and scan the flattened map directly.
5. Print results, accumulate totals. Optionally apply fixes, rewrite the file
   and re-lint once.

After all files: finalize "batch" rules (only `PylintScriptRule` with
`batch_mode: true` does anything), write optional report files, print the
summary, exit.

Only Perspective `view.json` files are linted. `resource.json` is never read.
No project-level analysis exists: each view is linted in isolation.

## 2. Flattening

`flatten_json(data, path="")` produces an ordered dict from path strings to
leaf values, in document order (depth first, keys in file order).

### 2.1 Path syntax

* Object member: `parent.key` (no escaping; keys containing `.`, `[` or `]`
  are written verbatim, which makes such paths ambiguous).
* Array element: `parent[i]`.
* A top-level array yields `[0]`, `[1]`, ...
* **Component name injection.** Whenever a JSON object has `meta` (an object)
  with a truthy `name`, the name is appended as an extra path segment
  *before* that object's members are flattened. This applies to *any*
  object, not only components. Consequences:
  * the view root component (top-level key `root`, `meta.name` = `"root"`)
    has path `root.root`;
  * a child is `root.root.children[0].Button`, its props
    `root.root.children[0].Button.props.text`;
  * its own name leaf is `root.root.children[0].Button.meta.name`;
  * a custom property whose value is an object with `meta.name` (for example
    `{"meta": {"name": "Ghost"}, "x": 1}`) also gets the name injected:
    `...custom.Ghost.x`, and it is modelled as a component (bug).
  * If `meta` exists but is not an object, flattening raises (crash).

### 2.2 Leaves

| JSON value | Flattened entries |
|---|---|
| scalar (string, number, bool, null) | `path = value` |
| non-empty object | recurse |
| empty object `{}` member | `path = {}` |
| empty array `[]` member | `path = []` |
| non-empty array | each element `path[i]`, recursively |
| empty object or empty array *as an array element* | nothing (silently dropped) |
| Java date object | `path._JavaDate = <$ts>` |

A Java date object is an object with exactly the two keys `$` and `$ts`,
where `$` is a 3-element array `["ts", <number>, <number>]` and `$ts` is a
number. Datasets (`{"$": ["ds", ...], "$columns": [...], "rows": [...]}`) are
not special-cased and flatten like any other object (so `$`, `$columns`,
`name`, `rows` become property names).

Values are the decoded JSON strings: the `=`, `'`, `<`,
`>`, `&` escapes that Ignition writes are already decoded.

## 3. Object model

`ViewModelBuilder.build_model(flat)` builds the collections below, in this
order, each in flattened-key (document) order.

### 3.1 Node types

`NodeType` values (the strings are used in config and CLI):

| value | class | built from |
|---|---|---|
| `component` | Component | every key ending in `.meta.name` |
| `expression_binding` | ExpressionBinding | `.binding.type` = `expr` or `expression`; also every `type: "expression"` transform |
| `expression_struct_binding` | ExpressionStructBinding | `.binding.type` = `expr-struct` |
| `property_binding` | PropertyBinding | `.binding.type` = `property` |
| `tag_binding` | TagBinding | `.binding.type` = `tag` |
| `query_binding` | QueryBinding | `.binding.type` = `query` |
| `message_handler` | MessageHandlerScript | `.scripts.messageHandlers` ... `.messageType` |
| `custom_method` | CustomMethodScript | `.scripts.customMethods[i]` |
| `transform` | TransformScript | `type: "script"` transform of any binding |
| `event_handler` | EventHandlerScript | `.events.<domain>.<event>` with a script |
| `property_change_script` | PropertyChangeScript | `propConfig.<prop>.onChange.script` |
| `property` | Property | remaining leaves (see 3.6) |

Groups: `ALL_BINDINGS` = the five binding types; `ALL_SCRIPTS` = the five
script types; `COMPONENT_REFERENCE_NODES` = expression, expression-struct and
property bindings plus `ALL_SCRIPTS`.

Binding types `http`, `tag-history` and any other unknown type produce no
binding node, but their transforms are still collected.

### 3.2 Components

For each flattened key ending in `.meta.name`: `path` = the key without
`.meta.name`, `name` = the value, `type` = value of `<path>.type` or
`"unknown"`. The root component is `root.root` named `root`.

Parent of a component (used by the reference rules): match
`(.*?)\.children\[\d+\]\.[^.]+$` on the component path; group 1 is the parent
path; no match means "no parent" (top level). Component names containing `.`
break all path arithmetic.

### 3.3 Bindings

For each key containing `.binding.type` (first occurrence per binding path,
document order): `binding_path` = key with the last `.binding.type` removed.
The node path is the **binding path**, which is the property path, e.g.
`root.root.children[0].Label.propConfig.props.text` or
`propConfig.custom.total` (view level). Bindings written inline
(`props.text.binding`) are found too: `root.root.children[0].Label.props.text`.

| type | fields (missing values) |
|---|---|
| expr / expression | `expression` = `<bp>.binding.config.expression` (`"unknown"`) |
| expr-struct | `struct` = every string leaf under `<bp>.binding.config.struct.` keyed by the remaining path; `config` = other config leaves |
| property | `target_path` = `<bp>.binding.config.path` (`"unknown"`) |
| tag | `tag_path` = `config.tagPath` (`"unknown"`), `mode` = `config.mode` (`"direct"`), `references` = string leaves under `config.references.`, `config` = other config leaves (excluding keys ending in `mode` or `tagPath`) |
| query | `query_path` = `config.queryPath` (`"unknown"`), `parameters` = string leaves under `config.parameters.` |

Transforms of every binding (whatever its type): keys starting with
`<bp>.binding.transforms` and ending in `.type`:

* value `script` and `<t>.code` exists: TransformScript, path
  `<bp>.binding.transforms[i]`, script = code.
* value `expression` and `<t>.expression` exists: an extra ExpressionBinding
  with path `<bp>.binding.transforms[i]` and that expression, appended to the
  expression bindings right after its owner.
* Other transform types (`map`, `format`, ...) are ignored.

### 3.4 Scripts and their wrappers

Each script node has a `function_def` header used only by PylintScriptRule:

| node | path | header |
|---|---|---|
| message handler | `<comp>.scripts.messageHandlers[i]` | `def onMessageReceived(self, payload):` |
| custom method | `<comp>.scripts.customMethods[i]` | `def <name>(self, <params...>):` (`[N]` stripped from name) |
| script transform | `<bp>.binding.transforms[i]` | `def transform(self, value):` |
| event handler | `<comp>.events.<domain>.<event>` (or `...<event>[k]` for action lists) | `def <event>(self, event):` (`[N]` stripped) |
| property change | `[<comp>.]propConfig.<prop>.onChange` | `def valueChanged(self, previousValue, currentValue, origin, missedEvents):` |

Details:

* Message handler: any key containing `.scripts.messageHandlers` and ending
  in `.messageType`; script = `<base>.script` or `""`; scope flags from
  `pageScope`/`sessionScope`/`viewScope` (unused by rules).
* Custom method: keys matching `\.scripts\.customMethods\[(\d+)\]`; fields
  `name` (default `unknown_method`), `script`, `params[j]`.
* Event handler: any key containing `.events.` and either containing
  `.config.script` or ending in `.script`. The event path is the key before
  `.config.script` (or before `.script`). `\.events\.([^.]+)\.([^.]+)$` must
  match the event path, giving domain and event type. Scope =
  `<event path>.scope` or `"L"`. **View-level events (top-level `events.*`
  keys, for example `events.system.onStartup`) do not match because the key
  has no leading dot, so view event scripts are invisible to every rule (bug).**
  Non-script actions (navigation, popup) have no script and are ignored.
* Property change: keys ending in `.onChange.script` that start with
  `propConfig.` or contain `.propConfig.`; property path = text between the
  last `propConfig.` and `.onChange`.
* The transform header lacks Ignition's `quality` and `timestamp` parameters,
  so a transform that uses them gets `undefined-variable` (bug).

Script formatting (`get_formatted_script`):

1. If `script.strip()` is empty the script becomes `"\tpass"`.
2. Find the first line that is non-blank and does not start with `#` after
   stripping. If it starts with a tab or four spaces, the script is used as
   is: `header + "\n" + script`.
3. Otherwise every non-blank line gets one leading tab and blank lines become
   empty: `header + "\n" + indented`.

The line count never changes, which is what the line mapping relies on.

### 3.5 Expression handling

There is **no expression tokenizer or parser**. Every expression check is a
substring or regex scan over the raw expression text, so matches inside
string literals and in non-reference positions count. The patterns are given
with each rule.

### 3.6 Properties

For every flattened leaf, in order, skip it when:

* the key contains `.meta.`, `.binding.`, `.scripts.` or `.events.`, or ends
  with `.type` (so a custom property literally named `type` is never a
  property);
* the key starts with `propConfig.` (view-level propConfig).

Then:

* Keys starting with `custom.` or `params.` (view level): the property is
  created if "persistent" (`propConfig.<key>.persistent` is not `false`; the
  check uses the full leaf key, so nested leaves are always persistent).
* Other keys: only if the key starts with some component path (plain string
  prefix test, longest wins). For component custom leaves the persistence
  check looks up `propConfig.<full key>.persistent`, which never exists, so it
  always passes.

The Property node:

* `path` = the key with **every `[N]` removed** (`root.root.children[0].B.custom.x`
  becomes `root.root.children.B.custom.x`).
* `name` = last segment of the key with a trailing `[N]` removed.
* `value` = the first leaf value seen (array elements are de-duplicated by the
  index-free path, so only the first element survives).
* `persistent` / `private_access` = from `propConfig.<key>.persistent` /
  `.access == "PRIVATE"`, only meaningful at view level.

Consequences (bugs): component-level propConfig metadata leaves become
properties (`root.root.children.B.propConfig.custom.x.persistent`,
`...propConfig.custom.x.access`, `...propConfig.custom.x.onChange.enabled`,
`...onChange.script`); view-level `props.*` (for example `props.defaultSize`)
are not properties (no component prefix).

Finally, propConfig-only view properties: for every view-level propConfig
property path (the part of a `propConfig.` key before the first segment named
`binding`, `persistent`, `paramDirection` or `access`, segment 0 always kept)
that starts with `custom.` or `params.` and was not already created, a
Property with `value = None` is added (sorted by path).

### 3.7 Node order seen by rules

Rules get one flat list built from these collections in this order:
components, message handlers, custom methods, expression bindings (including
expression transforms), expression-struct bindings, property bindings, tag
bindings, query bindings, script transforms, event handlers, property change
scripts, properties. Findings of node-based rules follow this order.

### 3.8 Private properties

`LintingRule.applies_to` drops Property nodes whose name starts with `_` or
equals `_JavaDate`, unless the rule was built with
`include_private_properties=True`. No shipped rule exposes that argument as a
config kwarg, so private properties are always skipped by node-based rules.
Flattened-map rules (ExcessiveContextData, PropertyAccess,
PropertyPersistence, the reference scans of UnusedCustomProperties) do not
apply this filter.

## 4. Rule framework

* A rule is a class deriving from `LintingRule` with `target_node_types`
  (empty set means all nodes), a default `severity` (`"warning"` or
  `"error"`; any other value silently becomes `"error"` for most rules), an
  `error_message` description and `create_from_config(kwargs)`, which drops
  kwargs whose key starts with `_` and calls `__init__(**kwargs)`.
* Visiting: `node.accept(rule)` calls `visit_<node_type>` or `visit_generic`.
* A violation is a plain string `"<node path>: <text>"` stored in the rule's
  `errors` or `warnings` list (per call severity override allowed).
* Results per file are dicts `rule class name -> [messages]`; rules without
  findings are absent.
* Rule name = Python class name; config keys and `--fix-rules` use it,
  case-sensitive.
* Registration order (which is also the output order of rules) is the order
  in which `rglob("*.py")` walks the `rules/` package, i.e. filesystem order,
  which differs between machines. On ext4 here it was: ExcessiveContextData,
  UnusedCustomProperties, PropertyAccess, PropertyPersistence, PylintScript,
  BadComponentReference, ComponentReferenceValidation, PollingInterval,
  NamePattern. Treat rule order as unspecified.
* Custom rules: drop a module into the installed package's `rules/`
  directory (any subpackage not starting with `_`); every `LintingRule`
  subclass defined there is auto-registered. Rules can also be registered
  from Python with `register_rule(cls, name=None)`. There is no config key,
  CLI flag, entry point or path setting to load external rule files.

## 5. Rules

Nine rules are registered in v0.6.4. All run by default (absent from config
means enabled with default kwargs). `PropertyAccessRule` and
`PropertyPersistenceRule` are inert unless configured.

Summary:

| Rule | Default severity | Default effect | Fix |
|---|---|---|---|
| NamePatternRule | warning | component names must be PascalCase | rename component (safe or unsafe) |
| PollingIntervalRule | error | `now()` polling below 10000 ms | no |
| PylintScriptRule | error (E, F) / warning (W, C, R) | pylint over all scripts of a view | strip trailing whitespace (safe) |
| BadComponentReferenceRule | error | traversal patterns (`getSibling`, `../`, ...) | no |
| ComponentReferenceValidationRule | error | relative/absolute/script component references must resolve | no |
| UnusedCustomPropertiesRule | error | custom props and params never referenced | delete property (safe) |
| ExcessiveContextDataRule | error | oversized view `custom.*` data | no |
| PropertyAccessRule | error | inert (`expected_access` unset) | set/remove `access` (safe) |
| PropertyPersistenceRule | error | inert (`expected_persistent` unset) | set `persistent: false` (safe) |

Message formats below use `{x}` placeholders; everything else is literal.

### 5.1 NamePatternRule

Validates names of components (default), properties, message handlers
(their `messageType`) and custom methods (their `name`). Event handlers,
bindings and transforms are never checked. Supports fixes for components only.

**kwargs**

| kwarg | type | default | notes |
|---|---|---|---|
| `convention` | string | none | one of the conventions below; unknown value prints `Warning: Unknown convention '{c}', using PascalCase as default` |
| `target_node_types` | string or list of node type strings | derived | unknown strings print `Warning: Unknown node type '{t}'. Available types: [...]` and are dropped |
| `custom_pattern` | regex string | none | overrides `convention` for the global pattern |
| `suggestion_convention` | string | none | convention used for suggestions (and for abbreviation processing) |
| `node_type_specific_rules` | object: node type string -> overrides | `{}` | unknown keys print `Warning: Unknown node type '{t}' in node_type_specific_rules` |
| `severity` | `"warning"`/`"error"` | `"warning"` | any other value is a rule creation error |
| `allow_numbers` | bool | true | when false, `0-9` is deleted from the convention regex |
| `min_length` | int | 1 | |
| `max_length` | int or null | null | |
| `forbidden_names` | list | none | |
| `skip_names` | list | `["root"]` when not given | giving a list replaces the default, so `root` is then checked |
| `allowed_abbreviations` | list | `[]` | |
| `auto_detect_abbreviations` | bool | true | adds the built-in abbreviation list |
| any other key | | | **silently ignored** (accepted via `**kwargs`; a typo such as `convetion` is not reported) |

`target_node_types` resolution: explicit value if it converted to at least
one valid type; else the keys of `node_type_specific_rules`; else
`{component}`.

Per node type overrides (`node_type_specific_rules.<type>`): `convention`,
`pattern`, `pattern_description`, `suggestion_convention`, `min_length`,
`max_length`, `forbidden_names`, `skip_names`, `severity`, `allow_numbers`
(only used to build the pattern from that override's `convention`). A missing
override key falls back to the global value. If an override has `convention`
but no `pattern`, its pattern and description come from the convention table.

**Conventions** (`pattern`, description used in messages):

| convention | regex | description |
|---|---|---|
| PascalCase | `^[A-Z][a-zA-Z0-9]*$` | `PascalCase` |
| camelCase | `^[a-z][a-zA-Z0-9]*$` | `camelCase` |
| snake_case | `^[a-z][a-z0-9_]*$` | `snake_case` |
| kebab-case | `^[a-z][a-z0-9-]*$` | `kebab-case` |
| SCREAMING_SNAKE_CASE | `^[A-Z][A-Z0-9_]*$` | `SCREAMING_SNAKE_CASE` |
| Title Case | `^[A-Z][a-z]*(\s[A-Z][a-z]*)*$` | `Title Case` |
| lower case | `^[a-z][a-z\s]*$` | `lower case with spaces (e.g., my button, data table)` |

Global pattern: `custom_pattern` (description `custom pattern: {regex}`), else
the convention, else PascalCase (also when `convention` is unset). Matching
uses Python `re.match` (anchored at the start only; the built-in patterns end
in `$`, custom ones may not).

Built-in abbreviations (when `auto_detect_abbreviations`): API HTTP HTTPS XML
JSON SQL URL URI UUID CPU GPU RAM SSD HDD PDF CSV ZIP GIF PNG JPG JPEG SVG CSS
HTML JS TS PHP ASP JSP CGI FTP SSH TCP UDP IP DNS DHCP VPN SSL TLS JWT CRUD
REST SOAP AJAX DOM UI UX GUI CLI OS iOS macOS AWS GCP IBM AI ML NLP OCR QR
RFID NFC GPS LED LCD OLED CRT ID.

**Which nodes**: the rule's target types; Property nodes are skipped when the
(index-stripped) path contains `.style.`, `.elementStyle.`, `.textStyle.`,
`.instanceStyle.`, `.position.` or `.props.aspectRatio`, or contains
`.props.elements` and ends with `.d`. Private properties are skipped
([3.8](#38-private-properties)). Name extraction: component `name`, property
`name`, message handler `message_type`, custom method `name`. Empty names are
skipped.

**Validation order** (first failure wins, at most one message per node):

1. name in `skip_names` -> no finding;
2. name in `forbidden_names` -> `Name '{name}' is forbidden for {type}`;
3. `len < min_length` -> `Name '{name}' is too short (minimum {n} characters) for {type}`;
4. `max_length` truthy and `len > max_length` -> `Name '{name}' is too long (maximum {n} characters) for {type}`;
5. pattern check on the *abbreviation-processed* name ->
   `Name '{name}' doesn't follow {description} for {type}` plus a suggestion
   suffix.

`{type}` is the node type value (`component`, `property`, ...). The full
violation is `{node path}: {text}`. Severity: per-type `severity` override,
else the rule severity.

**Abbreviation processing** (before the pattern check, only when a
`suggestion_convention` or `convention` applies to the node type and the
abbreviation set is non-empty): for each abbreviation, longest first, if
`abbrev in name.upper()` (substring, case-insensitive):

* PascalCase/camelCase: replace each of `ABBR`, `abbr`, `Abbr` occurring in the
  name with `ABBR`;
* snake_case/kebab-case: replace `ABBR` with `abbr`;
* SCREAMING_SNAKE_CASE: uppercase the whole name;
* Title Case: words equal to the abbreviation (case-insensitive) become
  uppercase;
* lower case: lowercase the whole name.

Bugs that follow (all reproduced in `probes/NamingAbbreviations`):

* camelCase false positives for any name *starting* with a lowercase
  abbreviation substring: `idleTimer`, `idle` (ID), `restartButton` (REST),
  `guideLabel` (GUI), `clientPanel` (CLI), `aspectRatio` (ASP), `aircraft`
  (AI), `domain` (DOM), `ledger` (LED), `ramp` (RAM) all fail camelCase.
* SCREAMING_SNAKE_CASE false negatives: any name containing an abbreviation
  substring passes (`idleTimer`, `myAPIButton`, `HTTPServer`).
* Title Case: a word that is an abbreviation (`API Key`) always fails.
* snake/kebab: `my_API` is lowercased before matching and passes.

**Suggestion suffix**, only when a `suggestion_convention` or a known
`convention` applies to the node type:

* ` (suggestion: '{s}')`, or
* for components whose suggestion equals the name of any component in the
  same parent container *including itself*:
  ` (suggestion '{s}' already in use by a sibling; rename manually)`. Because
  the component itself is included, a suggestion identical to the current
  name (common after the abbreviation false positives) produces this text
  (bug).

Suggestion algorithm: split the name on `[-_\s]+`; if that yields a single
part, split camel humps with `[A-Z][a-z]*|[a-z]+|[A-Z]+(?=[A-Z][a-z]|$)`
(digits are dropped, and runs of capitals split per letter: `HTTPServer` gives
`H T T P Server`, `myButton1` gives `my Button`). Then join per target:
Pascal (abbreviations upper, others `capitalize()`), camel (first part
lower unless abbreviation), snake/kebab (lowercase, `_`/`-`), SCREAMING (snake
then upper), Title (space joined, capitalized), lower (space joined lower).
An unknown suggestion convention yields `No suggestion available`.

**Fix** (components only, when fix mode is on): see [6.3](#63-namepatternrule-rename).

### 5.2 PollingIntervalRule

Flags `now()` polling faster than a minimum.

kwargs: `minimum_interval` (int ms, default 10000), `severity` (default
`"error"`). Targets all five binding types.

Checked strings:

| node | strings | message |
|---|---|---|
| expression binding (incl. expression transforms) | `expression` | `{path}: '{expr}'` |
| expression-struct | each struct value | `{path}.{key}: '{expr}'` |
| query | each parameter value | `{path}.{param}: '{expr}'` |
| tag, mode `expression` | `tagPath` | `{path}: '{tagPath}'` |
| tag, mode `indirect` | each reference value | `{path}.references.{key}: '{expr}'` |
| property, tag direct | nothing | |

A string is checked only if it contains the substring `now`. It is invalid
when:

* `re.findall(r'now\s*\(\s*(\d*)\s*\)')` finds nothing but `now\s*\(` occurs
  (for example `now({view.custom.rate})`, `now(-1)`); or
* any match has an empty group (`now()`); or
* any match has `0 < interval < minimum_interval`.

`now(0)` is valid. Exactly `minimum_interval` is valid.

Bugs (probe `PollingQuirks`): substring matching flags `snow(5)` and `now()`
inside string literals; if any numeric `now(n)` is present, non-numeric
`now(x)` calls in the same string are ignored (`now(20000) + now({x})`
passes).

### 5.3 PylintScriptRule

Runs pylint (in-process, `pylint.lint.Run`) over every script of a view.

kwargs:

| kwarg | default | meaning |
|---|---|---|
| `severity` | `"error"` | severity of the data-quality findings and of unmapped categories |
| `pylintrc` | null | rcfile path; relative to the current directory |
| `debug` | false | prints diagnostics, always keeps the combined file |
| `batch_mode` | false | lint all files' scripts in one pylint run at the end |
| `debug_dir` | null | where combined files are copied |
| `category_mapping` | `{"F":"error","E":"error","W":"warning","C":"warning","R":"warning"}` | replaces (does not merge with) the default |

Targets all script node types.

**rcfile resolution**: if `pylintrc` is given and exists (absolute, or
relative to the cwd) use it, else print
`⚠️  Warning: Specified pylintrc not found: {p}` (plus `   Tried: ...` and
`   Current directory: ...` lines for relative paths) and fall back. Fallback:
the first `.config/.ignition-pylintrc` found walking up from the cwd, else the
bundled `ignition_lint/.config/.ignition-pylintrc`. (Only if none existed
would pylint run with `--disable=all --enable=unused-import,undefined-variable,syntax-error,invalid-name`.)

**Data-quality checks**, before pylint, per non-empty script, with the rule
`severity`:

* first code line (non-blank, not starting with `#`) does not start with a
  tab or 4 spaces: `{path}: Script lacks proper indentation in view.json (scripts should be indented with tabs or spaces for valid Python syntax)`;
* any non-blank line whose leading whitespace contains both a tab and a
  space (first such line only): `{path}: Script mixes tabs and spaces for indentation. Use either tabs OR spaces consistently.`

If every script is blank, pylint is not run.

**Combined file**. All scripts of one view (all files in batch mode) go into
one temporary `.py` file named `HHMMSS_pid{PID}_{random}.py` in the system
temp dir:

```text
#pylint: disable=unused-argument,missing-docstring,redefined-outer-name,function-redefined
# Stub for common globals, and to simulate the Ignition environment
# Note: function-redefined is disabled because multiple scripts may define the same function names
system = None  # Simulated Ignition system object
self = {} # Simulated self object for script context
event = {}  # Simulated event object

# File: {source path as given on the command line}
# Script 1: {node path}
{header}
{formatted body}

# Script 2: {node path}
...
```

Each script: the two comment lines, the formatted script
([3.4](#34-scripts-and-their-wrappers)), one blank line. Scripts are ordered
by node order; keys are `{file}::{node path}` (a second script with the same
node path in the same file would overwrite the first).

pylint arguments: `--rcfile {rc}` (or the inline fallback),
`--output-format=text --score=no --module-rgx=.* {tmpfile}`. stdout and stderr
are captured. The temp file is deleted afterwards; if any finding exists (or
`debug`), it is first copied into the debug directory and
`🐛 Pylint found issues. Debug file saved to: {path}` is printed. The debug
directory is `debug_dir` (relative to cwd), else `tests/debug` if the cwd or
an ancestor is named `tests` or an ancestor contains a `tests` directory, else
`./.ignition-lint/debug`. On the first pylint run of the process, every `*.py`
in that directory is deleted (hazard: with a misconfigured `debug_dir` or a
repository that has `tests/debug`, unrelated `.py` files are removed).

**Output parsing**: each pylint output line matching
`.*:(\d+):\d+: ([EWCRF]\d+): (.+)` gives combined line, code and message (the
message includes pylint's ` (symbol)` suffix). The owning script is the one
with the greatest start line `<=` the line (start line = the `def` header).
Lines before the first script are dropped. Relative line =
`combined line - header line + 1`, so **body line k is reported as `Line k+1`**
and `Line 1` is the generated `def` line. Findings keep pylint's output order
(grouped by checker, not sorted by line). Multi-line pylint messages lose
their continuation lines.

Message: `{path}: Line {n}: {pylint message} ({code})`, for example
`root.root.children[0].Btn.events.component.onActionPerformed: Line 3: Undefined variable 'undefinedName' (undefined-variable) (E0602)`.
In batch mode `{path}` is `{file}::{node path}`.

Severity: `category_mapping[category]`, else the rule severity. A mapping
value other than `error`/`warning` is counted with the rule severity but
displayed in the warnings block (bug).

Display: pylint findings are shown grouped per category ([9.1](#91-normal-stdout)).
The rule's `errors`/`warnings` lists get one empty-string placeholder per
pylint finding, purely for counting.

**Effective behaviour with the bundled rcfile** (`py-version = 2.7`,
`max-line-length=140`, `indent-string=\t`, `check-quote-consistency=yes`,
`additional-builtins=system,General,Components,DanPT,Views,Widgets,unicode,long,double,AnalysisTags`,
`good-names=i,j,k,ex,_,eq,msg,db`, `bad-names=foo,bar,baz,toto,tutu,tata,temp,tmp,data,var,obj,item`,
snake_case-only variable/argument regexes, function names snake or PascalCase
plus a fixed exempt list). 345 messages are enabled; the full list is in the
[appendix](#15-appendix-pylint-messages-enabled-by-the-bundled-rcfile).
Disabled by the rcfile: trailing-whitespace, trailing-newlines,
wrong-import-order, import-error, no-name-in-module, no-member,
c-extension-no-member, useless-object-inheritance, duplicate-code, all
`too-many-*`/`too-few-*`, inconsistent-return-statements, super-with-arguments,
attribute-defined-outside-init, arguments-differ, signature-differs,
abstract-method, import-self, fixme, global-statement, raise-missing-from,
f-string-without-interpolation, redundant-u-string-prefix,
using-f-string-in-unsupported-version, the I-category messages; the combined
file header also disables unused-argument, missing-docstring,
redefined-outer-name and function-redefined.

The messages that dominate in practice (private corpus, [14](#14-private-corpus-run-aggregates)):
C0103 invalid-name (camelCase variables and arguments, wrapper names such as
`valueChanged`, camelCase custom method names), W1405 inconsistent-quotes,
E0602 undefined-variable, W0311 bad-indentation (space-indented scripts,
since `indent-string` is a tab), C0301 line-too-long, C0415
import-outside-toplevel, C0121 singleton-comparison, R1705 no-else-return,
W1401 anomalous-backslash-in-string, C0104 disallowed-name, W0702
bare-except, C0200, W0612 unused-variable, C0207, W0622 redefined-builtin.

Semantic consequences (bugs, all reproduced in `probes/ScriptLines` and
`probes/ScriptPy2Syntax`):

* Jython 2.7 code is parsed by CPython 3 (astroid). Any Python 2 only syntax
  (`print "x"`, `except E, e:`, backticks, `<>`, `0777`, `10L`, `exec "..."`)
  produces a single E0001 `Parsing failed: '...' (syntax-error)` for the
  combined file and **suppresses every other pylint finding of the view**.
  The E0001 text embeds the temp module name and the combined-file line.
* The combined file is one module, so module-wide checks leak across
  scripts: `inconsistent-quotes` compares quote styles across all scripts of
  the view; names defined at module level (`system`, `self`, `event`) are
  visible everywhere.
* Wrapper signatures do not match Ignition (`transform` lacks `quality` and
  `timestamp`); wrapper names and parameters trigger C0103 on `Line 1`.
* Jython builtins (`xrange`, `basestring`, `unicode` is declared, ...) and
  project script packages are undefined variables (E0602).

**Batch mode**: scripts accumulate across files and pylint runs once in
`finalize()`; data-quality checks also run then. Output is broken: the
multi-file batch section of stdout prints one `⚠️ PylintScriptRule: ` line
per placeholder with an empty message and never shows the grouped pylint
output (it only appears in the results file); per-file counts are zero and
`Files with issues` becomes at least 1. Fixes are disabled in batch mode.

**Fix**: see [6.4](#64-pylintscriptrule-trailing-whitespace). With the bundled
rcfile C0303 is disabled, so the fix never triggers.

### 5.4 BadComponentReferenceRule

Flags component tree traversal in scripts and bindings.

kwargs: `forbidden_patterns` (list of strings, default below; an empty list
also means the default), `case_sensitive` (bool, default true; when false,
both content and patterns are lowercased), `severity` (default `"error"`).

Default patterns, checked in this order as plain substrings:
`.getSibling(`, `.getParent(`, `.getChild(`, `.getChildren(`, `self.parent.`,
`self.children.`, `self.parent)`, `self.children)`, `self.parent,`,
`self.children,`, `self.parent\n`, `self.children\n`, `self.parent\r`,
`self.children\r`, `../`, `./`.

Checked content, per node (targets `COMPONENT_REFERENCE_NODES`):

| node | content | content type word |
|---|---|---|
| any script node | script | `Script` |
| expression binding (incl. expression transforms) | expression | `Expression` |
| expression-struct | all values joined with `\n` (one finding per binding) | `Expression` |
| property binding | target path | `Property Binding` |

Tag and query bindings are not checked. At most one finding per node:

`{path}: {Type} contains '{first pattern}' which creates brittle view structure dependencies. Consider using view.custom properties or message handling for component communication instead.`

or, when n > 1 patterns matched,
`... contains '{first}' and {n-1} other object traversal pattern(s) which creates ...`.
`{first}` is the first matching pattern in list order.

Matches in comments and string literals count. Every `../` reference also
matches `./`, so relative references always read `'../' and 1 other`.
`self.parent` at end of script (no trailing character) is not matched.

### 5.5 ComponentReferenceValidationRule

Checks that component references resolve in the view's component tree.

kwargs: `severity` (default `"error"`), `validate_expressions`,
`validate_property_bindings`, `validate_scripts` (bools, default true).

Index: all component nodes; parent via the regex in [3.2](#32-components);
children in node order; lookups by name return the first match.

Source component of a node path: if the path contains `.propConfig.`, the
part before it; else the part before the first of `.props.`, `.events.`,
`.scripts.`; else the longest component path that is a string prefix; else
`root` (which is not a component path, so view-level sources resolve
nothing).

**Expression references** (expression and expression-struct bindings, each
struct value separately, same node path): every match of
`\{{1,2}(\.+/|/)([^}]+?)\}{1,2}`. Leader `/` means absolute; otherwise
`levels_up = number of dots - 1`. The reference path is cut at the first of
`.props.`, `.position.`, `.meta.`, `.custom.`; the remainder split on `/`
(empty segments dropped) gives component names.

**Property binding paths**: the whole target path must match
`^(\.+/|/)(.+)$` (other paths such as `view.custom.x` are ignored); same
decomposition.

Relative resolution: start at the source component, go up `levels_up`
parents; if a parent is missing:
`{path}: Relative {kind} '{full}' navigates above root component`. Then walk
down the segments as children (one segment: direct child). Failure:
`{path}: {Kind} references non-existent component '{a/b}' in: {full}`.

Absolute resolution: first segment must name a top-level component (no
parent; normally `root`), then walk down. Failure: same "non-existent"
message with all segments joined.

`{kind}` is `expression` or `property binding`; `{Kind}` is the title-cased
form (`Expression`, `Property Binding`); `{full}` is the **whole expression
string** or whole property path, not just the reference.

**Scripts** (all script node types):

1. Each match of `self\.getSibling\(['"]([^'"]+)['"]\)`: unless view-scoped,
   find a sibling with that name (excluding itself). Failure:
   `{path}: Script references non-existent sibling component '{name}'`.
2. Each match of `(self(?:\.parent)*(?:\.[a-zA-Z]+\([^)]*\))+)` (no word
   boundary before `self`): `parent_count` = occurrences of `.parent`; method
   calls = `\.([a-zA-Z]+)\(["']([^"']+)["']\)` in the chain with `.parent`
   removed; chains without such calls are skipped.
   * Not view-scoped: go up `parent_count` parents from the source component;
     if one is missing: `{path}: Navigation chain goes beyond root: {chain}`.
     Then for each call: `getSibling` -> sibling lookup, `getChild` -> child
     lookup, other methods ignored. First failure:
     `{path}: Component '{arg}' not found as sibling in: {chain}` or
     `... not found as child in: {chain}`; stop.
   * View-scoped (node path starts with `propConfig.` or `events.`): chains
     with `.parent` are skipped; the first call must be `getChild` and
     resolves a top-level component, else the chain is skipped.

Because steps 1 and 2 both match `self.getSibling('X')`, one missing sibling
produces two findings (bug, reproduced in `probes/ReferenceQuirks`).

### 5.6 UnusedCustomPropertiesRule

Flags custom properties and view parameters that are defined but never
referenced.

kwargs: `severity` (default `"error"`).

**Definitions**, from Property nodes (index-stripped paths, private names
skipped), keyed as follows:

| property path | key | location |
|---|---|---|
| `custom.{name}[...]` | `view.custom.{name}` | `custom.{name}` |
| `params.{name}[...]` | `view.params.{name}` | `params.{name}` |
| `{...}.custom.{name}[...]` (first `.custom.`) | `{last segment before .custom.}.custom.{name}` | `{prefix}.custom.{name}` |

`{name}` is the first segment after `custom.`/`params.`, so nested leaves
register their top-level property. Component definitions are keyed by the
component **name**, so same-named components share one definition, the last
location wins and only one finding is emitted (bug). Because component
propConfig leaves are Property nodes ([3.6](#36-properties)), a component
propConfig entry such as `custom.x` with `persistent`/`access`/`onChange`
creates a bogus definition keyed `propConfig.custom.x`, reported as
`{comp}.propConfig.custom.x: custom property 'x' is defined but never referenced`
(bug, reproduced in `probes/ScriptLines`).

**Usage evidence** (a set of used keys). Sources:

1. Owners of bindings: every flattened key matching
   `^(?:(?P<owner>.+)\.)?propConfig\.(?P<prop>(?:custom|params)\..+?)\.binding(?:[.\[]|$)`
   credits `view.{prop}` (view level) or `{last owner segment}.{prop}`. The
   expression/property/tag binding visitors also credit their owner.
2. Every string value in the flattened map, with `view_scope` = key starts
   with `propConfig.` or `events.`:
   * plain substring search for, per definition: `view.custom.{n}`,
     `self.view.custom.{n}`, `{n}` in braces (`{{n}}`) for view custom;
     `view.params.{n}`, `self.view.params.{n}`, `{{n}}` for params;
     `{comp}.custom.{n}`, `this.custom.{n}`, `self.custom.{n}` for component
     custom (the latter two credit `*.custom.{n}` for every component).
     Substring search means `view.custom.foo` is credited by
     `{view.custom.foobar}` (bug, `probes/UnusedQuirks`);
   * script regexes with `nested` = `[a-zA-Z_][a-zA-Z0-9_]*(?:\.[a-zA-Z_][a-zA-Z0-9_]*)*`:
     `self\.view\.custom\.{nested}` -> `view.custom.X`,
     `self\.view\.params\.{nested}` -> `view.params.X`,
     `self\.custom\.{nested}` -> `*.custom.X` (and `view.custom.X` when view-scoped),
     `self\.params\.{nested}` -> `*.params.X` (and `view.params.X` when view-scoped),
     `[\w\)\]]\.custom\.{nested}` -> `*.custom.X`,
     subscripts `self.view.params['x']`, `self.view.custom['x']`,
     `<expr>.custom['x']` (and view-scoped `self.custom['x']`);
     `[\w\)\]]\.custom\b` not followed by `.` or `['` -> `*.custom.*`;
     `self\.view\.params\b` / `self\.view\.custom\b` not followed by `.` or
     `['` -> `view.params.*` / `view.custom.*`;
   * expression regexes: `\{view\.custom\.([^}]+)\}`, `\{view\.params\.([^}]+)\}`,
     `\{this\.custom\.([^}]+)\}` (-> `*.custom.X`), `\{self\.view\.custom\.([^}]+)\}`,
     `\{self\.view\.params\.([^}]+)\}`; `{view.params}`/`{self.view.params}`
     or a value that is exactly `view.params` -> `view.params.*`; same for custom.

A definition is used when its key is in the set, or the whole-scope wildcard
applies (`view.params.*`, `view.custom.*`), or any used key starts with
`{key}.` (a nested child is used), or for component custom
`*.custom.*`, `*.custom.{n}` or `*.custom.{n}.*` is present, or for view
custom `*.custom.{n}.*` is present, or for params `*.params.{n}` or
`*.params.{n}.*` is present.

Output (after all nodes, in definition order):
`{location}: {kind} '{name}' is defined but never referenced` where `{kind}`
is `view parameter` if the key contains `.params.` else `custom property`.
Locations are index-stripped (`root.root.children.Card.custom.alpha`).
Output parameters (`paramDirection: output`) are treated like any other.

**Fix**: see [6.5](#65-unusedcustompropertiesrule-delete).

### 5.7 ExcessiveContextDataRule

Flags large data stored in **view-level** `custom.*` (component custom and
params are not checked). Scans flattened keys directly; private keys are not
skipped.

kwargs (all keyword-only): `max_array_size` (50), `max_sibling_properties`
(50), `max_nesting_depth` (5), `max_data_points` (1000), `max_value_length`
(10000), `severity` (`"error"`).

Detectors, run in this order, all with strict `>`:

1. Arrays: keys matching `^(custom\.[^\[]+)\[(\d+)\]` (first bracket only);
   size = max index + 1 per group. Message:
   `{group}: Custom property '{group without "custom."}' contains {size} items. Large datasets should be stored in databases, not view JSON. Maximum recommended: {max} items.`
   (`replace('custom.', '')` removes every occurrence.)
2. Breadth: index-stripped keys under `custom.` with at least three segments;
   for every prefix of length 2 or more, collect distinct next segments.
   The top-level `custom` object itself is never measured.
   `{parent}: Contains {n} sibling properties. Large flat structures should be stored in databases, not view JSON. Maximum recommended: {max} siblings.`
3. Depth: one finding at most, for the first deepest index-stripped key
   (depth = segments - 1):
   `{path}: Custom properties are nested {d} levels deep. Deeply nested structures indicate complex data that should be in databases. Maximum recommended: {max} levels.`
4. Data points: number of flattened keys under `custom.`:
   `custom: Contains {n} total data points across all custom properties. Large volumes of data should be stored in databases, not view JSON. Maximum recommended: {max} data points.`
5. Value length, per string leaf under `custom.` (path keeps indices):
   `{path}: Custom property '{index-stripped path without first "custom."}' contains a string value of {len} characters. Large values should be stored in external resources or databases, not view JSON. Maximum recommended: {max} characters.`

Lengths are Python string lengths (code points). Empty objects inside
arrays are invisible ([2.2](#22-leaves)).

### 5.8 PropertyAccessRule

Opt-in check of the `access` mode of custom properties.

kwargs: `expected_access` (`"PUBLIC"`, `"PROTECTED"`, `"PRIVATE"`,
case-insensitive; null = rule inert; other values are a creation error),
`exempt_props` (list of glob patterns, default `[]`), `severity`
(`"error"`).

Candidates are top-level custom properties at view level and on every
component: those with a stored value (keys `custom.{top}...` or
`{component path}.custom.{top}...`, where the component path is a real
component path from `.meta.name` keys, indices kept) plus those with a
propConfig entry whose key starts with `custom.`. Evaluated in order sorted by
(owner or `""`, name). Exempt if any pattern fully matches the bare name,
`custom.{name}` or the full path; patterns support `*` and `?` only
(brackets are literal). Actual access = the `access` of the propConfig entry
`custom.{name}` (exactly that key; nested entries are ignored), uppercased,
default `PUBLIC`. Message:

`{full path}: custom property access mode is {actual}, expected {expected}`

where the full path is `custom.{name}` or `{component path}.custom.{name}`
(indices kept). `params.*` and `props.*` are never checked.

### 5.9 PropertyPersistenceRule

Opt-in check of `persistent` on bound custom properties.

kwargs: `expected_persistent` (true, false or null = inert; other types are
a creation error), `exempt_binding_types` (list, default `["tag"]`; tokens
`expr`, `expr-struct`, `property`, `query`, `tag`, `tag.direct`,
`tag.indirect`, `tag.expression`; unknown tokens are a creation error; `[]`
checks everything), `severity` (`"error"`).

Entries: every propConfig key, view or component level, matching
`^(?:(?P<owner>.+)\.)?propConfig\.(?P<prop>(?:custom|params|props)\..+?)\.(?P<attr>binding|persistent|access|paramDirection|onChange)(?:[.\[]|$)`
(owner must be a real component path). Only `custom.*` entries with a
binding are checked; exempt when the binding type is in the list or the
binding is a tag whose `tag.{mode}` token is listed. Missing `persistent`
means true. Messages:

* expected false, entry persistent:
  `{full path}: bound property is persistent; set persistent=false to stop storing designer results in view.json`
* expected true, entry not persistent:
  `{full path}: bound property is not persistent; this project expects persistent=true`
* expected true, persistent but no stored value (no key equal to or under
  `{owner.}custom.{prop}`):
  `{full path}: persistent bound property has no stored default value`

Order: first appearance of the entry in the flattened map.

## 6. Auto-fix

### 6.1 Model and flags

A fix is `{rule_name, violation_message, description, operations, is_safe,
safety_notes}`. Operations address the parsed JSON by a key/index path:

* `SET_VALUE(json_path, old_value, new_value)`
* `STRING_REPLACE(json_path, old_substring, new_substring)`: replaces every
  occurrence of the substring in the string value (no-op if absent)
* `DELETE_KEY(json_path, old_value)`: dict keys only, never list elements

Model paths are translated to JSON paths by walking the JSON exactly like the
flattener (component names map to the object holding `meta`).

Fix mode is on with `--fix`, `--fix-unsafe` or `--fix-dry-run`. Rules only
generate fixes in fix mode. Per-rule `allow_fix: false` makes a rule refuse
fix context (no fixes, not even in dry-run); `--fix-rules A,B` re-enables
`allow_fix` for the named rules and filters fixes to those rules.

Applying (`FixEngine.apply_fixes`), per file, fixes in generation order:

1. Conflict detection: a fix conflicts with an earlier accepted fix when any
   of its operations targets the same JSON path as an accepted operation and
   they cannot compose. `SET_VALUE` or `DELETE_KEY` against anything on the
   same path conflicts. Two `STRING_REPLACE` conflict when their old
   substrings are equal with different replacements, or one contains the
   other. The later fix is skipped (`conflicts with another fix`).
2. Fixes from rules not in `--fix-rules` are skipped (`filtered by --fix-rules`).
3. Unsafe fixes are skipped unless `--fix-unsafe` (`unsafe: {safety_notes}`
   or `unsafe fix (use --fix-unsafe)`).
4. Errors while applying (`KeyError`, `TypeError`, `ValueError`) skip the fix
   (`error: {e}`); operations already applied are not rolled back.

If at least one fix applied, the whole file is rewritten and the file is
linted **once more** (no fixes collected); the second pass's results are the
ones printed and counted.

### 6.2 Write-back format

`json.dumps(obj, indent=2, ensure_ascii=False)`, trailing whitespace stripped
(so **no final newline**), then every `'`, `<`, `>`, `&`, `=` character
anywhere in the text (keys and values) is replaced by `'`, `<`,
`>`, `&`, `=`. Written with `\n` newlines. Key order is
preserved. Everything else is normalised: number formatting (Python float
repr), other escapes, non-ASCII stays raw, original indentation is lost.

### 6.3 NamePatternRule rename

For component violations from the pattern check (also for length/forbidden
violations if a suggestion differs, since the fix is generated for every
component violation): suggestion from [5.1](#51-namepatternrule); skip when
there is no suggestion, it equals the current name, the component's meta.name
path is unknown, the suggestion equals an existing sibling name, or another
pending rename in the same container already claimed it.

Operation 1: `SET_VALUE [...component json path, "meta", "name"]`.
References are searched in every non-empty string of the flattened map:

* keys ending in `.script` or `.code`: Python-tokenized (CPython 3
  `tokenize`) `getSibling`/`getChild` `(` STRING `)` calls whose argument
  equals the old name; if tokenizing fails, the regex
  `\.(getSibling|getChild)\(\s*(['"])((?:[^'"\\]|\\.)*?)\2\s*\)` is used and
  the reference is **not rewritable**;
* other strings: a whole-value property path (`^(\.+/|/)(.+)$`) whose
  segments include the name, else expression references
  (`\{{1,2}(\.+/|/)([^}]+?)\}{1,2}`) whose segments include the name, else a
  "free text" hit (navigation call naming it, or `runScript(` plus the name
  anywhere) which is not rewritable.

References match by name anywhere in the view, not by resolution. Any
non-rewritable reference: no fix at all. Any reference, or any string under
the component's model path containing `this.meta.name`: unsafe fix with
safety notes `component uses 'this.meta.name' binding` and/or
`updates {n} reference(s)` joined by `; `, plus one `STRING_REPLACE` per
distinct reference text replacing the full reference with the renamed one
(every occurrence of the old name preceded by `/` or `{` and followed by `/`,
`.`, `}` or the end is replaced, regex
`(?<=[/{]){old}(?=[/.}]|$)`; script arguments keep their quote style).
Otherwise a safe fix.
Description: `Rename component '{old}' to '{new}'`.

### 6.4 PylintScriptRule trailing whitespace

Only when C0303 findings exist (not with the bundled rcfile) and not in batch
mode: safe `SET_VALUE` of the whole script with every line `rstrip()`ed.
Description `Remove trailing whitespace from script at {path}`, violation
message `{path}: Trailing whitespace (C0303)`. The JSON path is resolved from
`{node path}.code` (transforms), `.config.script` or `.script` (events),
`.script` (others).

### 6.5 UnusedCustomPropertiesRule delete

Only safe fixes. Never for view parameters, never when a removed propConfig
entry contains `onChange`, never when a matching propConfig entry contains
`binding`, never when the component cannot be resolved unambiguously (index
stripped path matched against component paths; two same-named siblings make
it ambiguous). Operations: `DELETE_KEY` of `[owner..., "custom", name]` if
present, plus `DELETE_KEY` of every `propConfig` key equal to `custom.{name}`
or starting with `custom.{name}.` or `custom.{name}[`. Description
`Remove unused custom property '{name}'`.

### 6.6 PropertyAccessRule

Safe. Expected `PUBLIC`: delete the `access` key of the entry, or the whole
entry when `access` was its only key; nothing if no access declared.
Otherwise: set `access` on an existing entry; else add the entry
`{"access": X}` when the owner's `propConfig` exists (or an earlier fix in
this run creates it); else create `propConfig` as `{"custom.{name}": {"access": X}}`.
A guard drops any fix whose operations do not target a `custom.*` propConfig
key. Description `Set access mode of '{full path}' to {X}`.

### 6.7 PropertyPersistenceRule

Safe, only for `expected_persistent: false`: `SET_VALUE` of
`[owner..., "propConfig", prop_key, "persistent"]` to false, plus
`DELETE_KEY` of the stored value at `[owner..., *prop_key.split('.')]` when
present and the key has no `[`. Description
`Make bound property '{full path}' non-persistent`.

## 7. Configuration file

`--config PATH` (default `rule_config.json` in the cwd). A JSON object:

```json
{
  "_comment": "ignored",
  "RuleClassName": {
    "enabled": true,
    "allow_fix": true,
    "kwargs": { "_comment": "ignored", "key": "value" }
  }
}
```

* Every registered rule runs unless its entry has a falsy `enabled`. Absent
  rules run with default kwargs. (The upstream docs claim omitting a rule
  disables it; the code does not.)
* Top-level keys starting with `_` or whose value is not an object are
  ignored silently. Unknown rule names print `Unknown rule in config: {name}`
  and are otherwise ignored.
* Inside a rule entry only `enabled`, `allow_fix` and `kwargs` are read;
  other keys are ignored.
* `kwargs` keys starting with `_` are dropped. Unknown kwargs raise
  `TypeError` (rule not loaded) except for NamePatternRule, which ignores
  them. A non-object `kwargs` is also a creation error.
* `allow_fix` must be a JSON boolean, else
  `Error creating rule {name}: allow_fix must be true or false; got {v!r}`.
  On a rule without fix support it prints
  `Note: allow_fix has no effect on {name} (rule does not support auto-fix)`.
* Creation errors print `Error creating rule {name}: {error}`; the run
  continues without that rule.
* Missing or unparsable file: `Error loading config file {path}: {error}`
  then `❌ No valid configuration found`, exit 1. A JSON array at top level
  crashes. No loaded rules: `❌ No valid rules configured`, exit 1.
* An empty object `{}` runs every rule with defaults.

## 8. CLI

Entry point `ign-lint` (also `python -m ignition_lint`). argparse uses the
executable name as program name; the deprecation notice hard-codes
`ignition-lint`.

### 8.1 Options

| option | meaning |
|---|---|
| `--version` | prints `ign-lint dev` (the version lookup uses the wrong distribution name, bug) |
| `--config PATH` | config file, default `rule_config.json` |
| `--files VALUE` | one glob or file, or a comma-separated list of them; default `**/view.json` when no files are given |
| `FILE ...` (positional) | explicit files |
| `-v`, `--verbose` | rule breakdown, per-file statistics, whitelist info |
| `--stats-only` | build the model and print statistics, no config, no rules |
| `--debug-nodes [TYPE ...]` | print up to 10 nodes (of the given node types, or all) per file; greedy, so use `--` before positional files |
| `--analyze-rules` | per rule: targets, node count, samples (ignored with `--stats-only`) |
| `--debug-output DIR` | write `{stem}_flattened.json`, `{stem}_model.json`, `{stem}_stats.json` per lint pass |
| `--ignore-warnings` | warnings do not fail the run |
| `--timing-output PATH` | write a timing report |
| `--results-output PATH` | write the results report |
| `--whitelist PATH` | skip files listed in PATH |
| `--no-whitelist` | ignore `--whitelist` |
| `--generate-whitelist PATTERN ...` | write a whitelist and exit |
| `--whitelist-output PATH` | target of `--generate-whitelist`, default `.whitelist.txt` |
| `--append` | merge with the existing whitelist |
| `--dry-run` | print what would be whitelisted |
| `--fix` | apply safe fixes |
| `--fix-unsafe` | apply safe and unsafe fixes (implies fix mode) |
| `--fix-dry-run` | report pre-fix results and print proposed fixes |
| `--fix-rules A,B` | restrict fixes to these rule names; overrides `allow_fix: false`; unknown names print `⚠️  --fix-rules: '{name}' does not match any loaded rule; its fixes cannot apply`; rules without fixes print `⚠️  --fix-rules: '{name}' does not support auto-fix` |

### 8.2 File selection

* Positional files and `--files` are merged: if both are present (the
  `--files A B C` form, where argparse binds A to `--files`), a deprecation
  notice is printed and A is linted first. Explicit paths are not filtered by
  name; non-existent ones print `Warning: File {f} does not exist`; a file
  whose JSON is falsy (`{}`, `[]`) prints `❌ Failed to read file, skipping`
  and still counts as processed.
* Glob mode (`--files` alone or nothing): split on `,`, strip each pattern,
  `glob.glob(pattern, recursive=True)` relative to the cwd. Python glob
  semantics: `**` matches any number of directories only as a whole segment,
  `*`/`?`/`[...]` do not match a leading `.` (hidden files and directories
  are skipped), `[...]` is a character class (no escaping), results come in
  directory-listing order (unsorted, filesystem dependent). Results whose
  basename is not exactly `view.json` are dropped.
* De-duplication by resolved absolute path, first occurrence wins.
* Whitelist check per file (resolved path); skipped files print
  `🔒 Skipped (whitelisted): {path}`.
* Paths are printed as given (relative stays relative).
* Processing order is the collected order.

### 8.3 Whitelist file

Plain text, one path per line, relative to the **current directory** (not to
the whitelist file); leading/trailing whitespace stripped; blank lines and
lines starting with `#` ignored; no globs, no inline comments. A missing file
is an empty whitelist (with `--verbose`:
`⚠️  Whitelist file specified but empty or not found: {path}`).

`--generate-whitelist P1 P2 ...`: each pattern globbed recursively from the
cwd (any file, not only `view.json`), made relative to the cwd (outside the
cwd: kept as given), de-duplicated and sorted. With `--dry-run`:
`🔍 Would add {n} files to whitelist:` followed by up to 20 `  {path}` lines
and `  ... and {n-20} more files`. Otherwise the file is written (parent
directories created):

```text
# Ignition-lint whitelist - files to ignore during linting
# Lines starting with # are comments
# One file path per line (relative to repository root)
# Generated: {number of newly matched files} files added
# Existing: {n} files          <- only with --append and an existing list

{sorted paths, one per line}
```

and `✓ Whitelist generated: {path}` (or `✓ Whitelist appended to: {path}`),
`  Total files: {n}`, with `--append` also `  New files: {n}` and
`  Existing files: {n}`. Exit 0 (1 on write error).

### 8.4 Fix output

`--fix-dry-run` prints the normal file results, then:

```text

========================================================================================================================
Proposed Fixes for {path}:
========================================================================================================================

  Fix {i} ({rule}) [SAFE]:            or [UNSAFE] / [UNSAFE - {safety notes}]
    {description}
    Operations:
      SET {path}: '{old}' -> '{new}'
      REPLACE in {path}:
              '{old}' -> '{new}'
      DELETE {path}

Summary: {n} fixes ({s} safe, {u} unsafe)
  Filtered out: {f} (not in --fix-rules)          <- only when f > 0
  --fix would apply: {s} safe fix(es)             <- when not --fix-unsafe and u > 0
  --fix --fix-unsafe would apply: {n} fix(es)
  Would apply: {n} fix(es)                        <- otherwise
```

Operation paths are rendered as `seg > seg[0] > seg` (`root > children[0] > meta > name`).
Fix numbering counts filtered fixes too. Conflicts are not shown in dry-run.

`--fix` / `--fix-unsafe` print, when fixes exist:

```text

Applying fixes to {path}:                <- only if something applied
  Applied: {description}
  File updated: {path}
  Skipped: {description} ({reason})

Applied: {a} fix(es) | Skipped: {s} fix(es)
```

followed by the (re-linted) file results.

### 8.5 Debug options

* `--debug-nodes`: `\n🔧 Debug info for node types: {t1, t2}` (or
  `\n🔧 Debug info for all nodes:`), then `  {i}. {path} ({type})` for the
  first 10 nodes and `     ... and {n} more nodes`. The node list includes
  the generic collections, so nodes appear twice (bug). Unknown types print
  `Warning: Unknown node type '{t}'. Available types: [...]`.
* `--analyze-rules`: `\n🔍 Rule Impact Analysis:` and per rule
  `  📋 {rule}:`, `    Targets: {sorted types | all}`,
  `    Will process: {n} nodes`, then either `    Sample nodes:` with up to 3
  `      • {path}: {summary}` lines or `    Sample paths: {up to 3 paths}`, and
  a blank line. Counts include duplicates. Summaries: `Component '{name}' of type '{type}'`,
  `Expression: {first 50 chars}...`, `Tag path: {p}`, `Property path: {p}`,
  `Script: {first 30 chars}...`, `{type} node`.
* `--verbose`/`--stats-only` statistics:
  `\n📊 Model statistics for {path}:`, `  Total nodes: {n}`,
  `  Node types found:` with `    {type}: {n}` in NodeType order (zero counts
  omitted), `  Components by type:` with `    {component type}: {n}` in first
  appearance order, and with rules loaded `  Rule coverage:` with
  `    {rule}: {n} nodes ({sorted types, comma separated} | all)`.
* `--debug-output DIR`: creates DIR; on every lint pass writes the three JSON
  files (sorted keys, indent 2) named after the file **stem**, so every
  `view.json` overwrites the previous one (bug); prints
  `✅ Debug files saved:` and three `   - ...: {path}` lines.

### 8.6 Exit codes

| situation | exit |
|---|---|
| `--generate-whitelist` | 0 (1 if writing fails) |
| no files selected | 0 (`❌ No files specified or found`) |
| config missing/invalid, or no rule loaded | 1 |
| any selected file with invalid JSON | 1, immediately, no summary (`ERROR: Invalid JSON in {abs path}: {msg}` on stderr via logging) |
| `--stats-only` | 0 |
| no warnings and no errors | 0 |
| errors > 0 | 1 |
| warnings only | 1, or 0 with `--ignore-warnings` |
| `--stats-only` with `--timing-output` | 1 (traceback, bug) |
| argparse usage error | 2 |

Counts after `--fix` are the post-fix counts.

## 9. Output formats

### 9.1 Normal stdout

Per run (the golden `crates/iglint_linter/tests/fixtures/ign-lint/cli-default.stdout.txt`
shows a full example):

```text
🔧 Loaded configuration from {config}
{config warnings, verbose rule breakdown, whitelist skips}

📄 Evaluating file:
    {path}
{optional debug/statistics blocks}
{🐛 Pylint found issues. Debug file saved to: {path}}

❌ Found {E} errors:

  {Rule}:
    • {message}
    • {message}
{custom block}

⚠️  Found {W} warnings:
  ...same layout...
```

* Errors block first, then warnings. Each rule block: an empty line, two
  spaces + rule name + `:`, one `    • {message}` per non-empty message, then
  the custom block if any (PylintScriptRule), then an empty line.
* PylintScriptRule custom block for one severity: for each category in order
  F, E, W, C, R mapped to that severity:
  `\n    Pylint - {Fatal|Error|Warning|Convention|Refactor} ({letter}):`
  followed by `      • {message}` lines.
* The counts `{E}`, `{W}` include pylint findings.
* A clean file prints `✅ No issues found`.

Final summary:

```text

📈 Summary:
  Files processed: {n}
  ⚠️  Total warnings: {W}          <- only if W > 0
  ❌ Total errors: {E}             <- only if E > 0
  📁 Files with issues: {k}
  📁 Clean files: {n-k}
  ✅ No errors found (ignoring warnings)    <- --ignore-warnings and E == 0
```

or `  ✅ No style inconsistencies found!` when there are no findings, or
`  📊 Statistics analysis complete` with `--stats-only`. Other lines:
`📁 Processing {n} files` (verbose), `🔍 Performance profiling enabled (output: {p})`,
`\n📊 Timing report written to: {p}`, `\n📝 Results written to: {p}`,
`\n📊 Aggregated summary written to: {p}`.

Batch mode, multi-file section (after all files):
`\n` + 80 `=`, `📦 Batch Rule Results (All Files)`, 80 `=`, then
`⚠️ {rule}: {message}` / `❌ {rule}: {message}` lines. With a single file the
batch results are printed as `\n⚠️ Found {n} warnings:` /
`  📋 {rule} (warning):` / `    • {msg}` (and the error equivalent).

### 9.2 `--results-output`

Written once at the end (only when at least one file was processed and not
`--stats-only`). Lines of 120 `=` or `-`. Layout:

```text
========== (120)
IGNITION-LINT RESULTS
========== (120)

SUMMARY
---------- (120)
Files processed: {n}
Total warnings:  {W}
Total errors:    {E}
Files with issues: {k}
Clean files:     {n-k}
Files whitelisted: {w}        <- only if w > 0

WHITELISTED FILES (SKIPPED)   <- only if w > 0
---------- (120)
The following {w} file(s) were skipped due to whitelist:

  🔒 {path}
                              <- blank line
PER-FILE RESULTS
========== (120)

---------- (120)
{❌|⚠️|✅} {path}
---------- (120)

ERRORS ({e} total):

  {Rule}:
    • {message}
{custom block}

  {Rule}:
    ...

WARNINGS ({w} total):
...
                              <- one blank line closes each file
========== (120)              <- batch section only with batch findings
📦 BATCH RULE FINALIZATION RESULTS
...
========== (120)
END OF RESULTS
========== (120)
```

Each rule block is `  {Rule}:`, message lines, the custom block, then one
blank line. Status icon: `❌` if the file has errors, else `⚠️` if warnings,
else `✅` (no extra space after `⚠️` here, unlike stdout). The golden
`cli-default.results.txt` is authoritative for whitespace.

If the target file already exists and is younger than 5 seconds (a parallel
pre-commit batch), the report goes to `{stem}_pid{PID}_batch{n}{suffix}`
instead, and `{stem}_AGGREGATED_SUMMARY.txt` is (re)written by summing the
SUMMARY numbers of `{stem}.txt` and all `{stem}_pid*.txt` (80-column format:
`AGGREGATED IGNITION-LINT RESULTS SUMMARY`, `Generated: {timestamp}`,
`Total batches: {n}`, totals, `BREAKDOWN BY BATCH` lines
`{name:<45} Files: {n:>3}  Warnings: {n:>4}  Errors: {n:>4}  Issues: {n:>3}  Clean: {n:>3}`).
At start-up, an existing `{stem}` file, `{stem}*batch*.txt` files from other
PIDs and the aggregated summary are deleted when at least 5 seconds old.

### 9.3 `--timing-output`

Same uniqueness and clean-up rules. 80-column report:

```text
================================================================================
IGNITION-LINT PERFORMANCE TIMING REPORT
================================================================================
Report Generated: {YYYY-MM-DD HH:MM:SS}

OVERALL SUMMARY
--------------------------------------------------------------------------------
Total Duration: {ms:.2f} ms ({s:.2f}s)
Files Processed: {n}
Average Per File: {ms:.2f} ms

PHASE BREAKDOWN (Totals)
--------------------------------------------------------------------------------
File Reading:    {:>10.2f} ms
JSON Flattening: {:>10.2f} ms
Model Building:  {:>10.2f} ms
Rule Execution:  {:>10.2f} ms

PHASE BREAKDOWN (Averages per file)
...same four lines...

RULE EXECUTION SUMMARY
--------------------------------------------------------------------------------
{'Rule Name':<40} {'Total (ms)':>12} {'Avg (ms)':>12} {'Executions':>12}
--------------------------------------------------------------------------------
{rule:<40} {total:>12.2f} {avg:>12.2f} {count:>12}     <- sorted by total desc

PER-FILE TIMING DETAILS
================================================================================

File: {path}
Total: {ms:.2f} ms
  - File Read:      {:>10.2f} ms
  - JSON Flatten:   {:>10.2f} ms
  - Model Build:    {:>10.2f} ms
  - Rule Execution: {:>10.2f} ms
  Rule breakdown:
    - {rule}: {ms:.2f} ms                              <- sorted desc

================================================================================
END OF REPORT
================================================================================
```

"Model Building" is the statistics call (which builds the model); the rule
timings include each rule's own processing. Batch finalize timings are not
included.

### 9.4 Debug output files

`{stem}_flattened.json`: the flattened map. `{stem}_model.json`: per
collection `{"count": n, "nodes": [serialized]}` where a node serializes to
`path`, `node_type` and type-specific fields (component `name`, `type`,
`properties_count`; bindings their fields and `config`; scripts
`script_length`, `script_preview` (first 100 chars + `...`) and type fields;
property `name`, `value`, `value_type`, optional `persistent`,
`private_access`). `{stem}_stats.json`: `total_nodes`, `node_type_counts`,
`components_by_type`, `rule_coverage`, `model_keys`.

## 10. Integrations

### 10.1 pre-commit

`.pre-commit-hooks.yaml` in the repository:

```yaml
- id: ign-lint
  name: Ignition Lint
  entry: ign-lint
  language: python
  types: [json]
  files: view\.json$
  args: ['--config=.ignition-lint-precommit.json']
  pass_filenames: true
  minimum_pre_commit_version: '2.9.0'
```

The staged files arrive as positional arguments. The default config file
`.ignition-lint-precommit.json` is **not** shipped in v0.6.4, so the hook
fails with exit 1 unless the consumer repository provides it or overrides
`args`. Users commonly add `--ignore-warnings` and `--whitelist=...`.
Parallel pre-commit batches are what the `_pid*_batch*` report naming and
the 5-second clean-up guard exist for.

### 10.2 GitHub Actions

v0.6.4 ships **no** `action.yml`. A composite action existed up to v0.2.4
(inputs `files` default `**/view.json`, `config` default `rule_config.json`,
`verbose` default `false`; it ran `poetry run ignition-lint --config ... --files ...`).
The current docs show plain `pip install ign-lint` plus
`ign-lint --config rule_config.json --files "views/**/view.json"` (and still
reference the removed action).

### 10.3 Docker

`docker/docker-compose.yaml` only starts an Ignition 8.1 gateway
(`bwdesigngroup/ignition-docker:8.1.44`) with the test views mounted, for
authoring fixtures. There is no linter container image.

## 11. Other user-visible behaviour

* No environment variables are read.
* `logging.basicConfig(level=INFO, format="%(levelname)s: %(message)s")` is
  installed at import time; only JSON read/write errors use it (stderr).
* Everything else is printed to stdout, including warnings and errors.
* pylint's own stdout/stderr is captured and never shown.
* Files are processed sequentially in one process; pylint runs in-process
  once per view (most of the run time, see [14](#14-private-corpus-run-aggregates)).
* A process-global hash seed influences abbreviation processing order only
  for same-length abbreviations (no observed effect).

## 12. Unreleased changes on dev after v0.6.4

`origin/dev` (24 commits after v0.6.4, head `073055f`, 2026-10-06) adds, per
its changelog:

* Lint domains: a `scripting` domain for project library scripts
  (`ignition/script-python/**/code.py`) with node types `script_module`,
  `script_package`; rules `LibraryScriptPylintRule` (own rcfile
  `.config/.ignition-library-pylintrc`, real line numbers, auto-declared
  top-level packages as builtins) and `LibraryNamePatternRule`; an
  `ign-lint-scripting` pre-commit hook.
* A `view` node type (view name, folder path, root container type, default
  size) and NamePatternRule targeting `view` (view and folder names,
  `check_view_folders`).
* `PylintScriptRule` renamed `PerspectiveScriptPylintRule` (old name kept as
  an alias), pylint run with `--jobs=1`, start-up failures reported as F0001,
  rcfile diagnostics reported without a line.
* Bundled rcfiles disable `use-maxsplit-arg` and `import-outside-toplevel` and
  declare Jython builtins (`xrange`, `basestring`, `raw_input`, `reduce`,
  `cmp`, `unichr`, `file`, `execfile`, `reload`, `apply`, `buffer`,
  `intern`, `coerce`, `StandardError`).
* `--debug-output` writes one folder per file mirroring the source path,
  with a manifest-scoped clean-up.
* The pre-commit default config `.ignition-lint-precommit.json` is bundled
  and used as a fallback.
* Glob results are filtered by domain instead of the name `view.json`.

None of this is released; iglint should plan for it but parity tests target
v0.6.4.

## 13. Bug list

Reproduced against 0.6.4. "Probe" refers to
`crates/iglint_linter/tests/fixtures/ign-lint/probes/`.

| # | Area | Bug | Probe |
|---|---|---|---|
| 1 | model | View-level `events.*` scripts are never modelled; every script rule ignores them | |
| 2 | model | Any object with `meta.name` (also inside `custom`) becomes a component and renames paths | |
| 3 | model | Component propConfig metadata leaves (`persistent`, `access`, `onChange.*`) become Property nodes | ScriptLines |
| 4 | model | Property node paths have indices stripped, so messages of NamePattern and Unused rules lack `[N]` | UnusedQuirks |
| 5 | model | A custom property named `type` is never a property | |
| 6 | NamePattern | Abbreviation substring processing: camelCase false positives (`idleTimer`, `restartButton`, `clientPanel`), SCREAMING false negatives, Title Case abbreviations always fail | NamingAbbreviations |
| 7 | NamePattern | Sibling-collision check includes the component itself ("suggestion 'x' already in use by a sibling" for its own name) | NamingAbbreviations |
| 8 | NamePattern | Suggestions drop digits and split capital runs (`HTTPServer` -> `H T T P Server`) | NamingAbbreviations |
| 9 | NamePattern | Unknown kwargs are silently ignored | |
| 10 | Polling | Substring matching (`snow(5)`, `now()` in string literals); non-numeric `now(x)` ignored when a numeric `now(n)` is present | PollingQuirks |
| 11 | Pylint | Python 2 syntax in one script hides all other pylint findings of the view | ScriptPy2Syntax |
| 12 | Pylint | Reported line = body line + 1 | ScriptLines |
| 13 | Pylint | Transform wrapper lacks `quality`, `timestamp`; wrapper names and camelCase arguments trigger C0103 | ScriptLines |
| 14 | Pylint | Module-wide checks (inconsistent-quotes) leak across scripts of a view | ReferenceQuirks |
| 15 | Pylint | Batch mode prints empty messages and hides grouped output on stdout | |
| 16 | Pylint | Debug-directory clean-up deletes every `*.py` in the chosen directory | |
| 17 | Pylint | Non `error`/`warning` category mapping values are counted as errors but displayed as warnings | |
| 18 | BadComponentReference | `../` always also matches `./`; matches in comments and strings | ReferenceQuirks |
| 19 | ComponentReferenceValidation | `self.getSibling('X')` missing sibling reported twice | ReferenceQuirks |
| 20 | ComponentReferenceValidation | View-level bindings resolve from a fake `root` component | ReferenceQuirks |
| 21 | Unused | Substring crediting (`foo` credited by `foobar`), `this.custom.x` anywhere credits every component's `x` | UnusedQuirks |
| 22 | Unused | Component definitions keyed by component name: same-named components merge into one finding | UnusedQuirks |
| 23 | Unused | Bogus definitions from component propConfig leaves (`...propConfig.custom.x`) | ScriptLines |
| 24 | ExcessiveContextData | Only view-level custom; top-level breadth never measured; empty objects in arrays invisible | |
| 25 | CLI | One invalid JSON file aborts the whole run | |
| 26 | CLI | `--version` prints `dev` | |
| 27 | CLI | `--stats-only --timing-output` crashes | |
| 28 | CLI | `--debug-output` files overwrite each other (named after the stem `view`) | |
| 29 | CLI | `--debug-nodes`/`--analyze-rules` count nodes twice | |
| 30 | CLI | Rule order depends on filesystem order | |
| 31 | fix | Write-back reformats the whole file and drops the final newline | |
| 32 | hook | The pre-commit hook's default config file is not shipped | |
| 33 | docs | Docs say absent rules are disabled; they run | |

## 14. Private corpus run (aggregates)

A private Ignition 8.3 project with **287 views** was linted with ign-lint
0.6.4 exactly as that project's CI does:
`ign-lint --config rule_config.json --whitelist {whitelist} --ignore-warnings --files "projects/**/view.json" --results-output {file outside the repo}`,
from the project root, with the project's own `rule_config.json`
(PropertyAccess and PropertyPersistence stay inert). The detailed results are
kept outside this repository for local parity diffing.

With the project's whitelist (34 views skipped, 253 linted): exit 0,
4199 warnings, 0 errors, 221 files with findings.

| rule | warnings |
|---|---|
| PylintScriptRule | 1933 |
| NamePatternRule | 1815 |
| BadComponentReferenceRule | 277 |
| UnusedCustomPropertiesRule | 169 |
| ExcessiveContextDataRule | 5 |
| ComponentReferenceValidationRule | 0 |
| PollingIntervalRule | 0 |

Without the whitelist (all 287 views): exit 1.

| rule | warnings | errors |
|---|---|---|
| NamePatternRule | 3197 | 0 |
| PylintScriptRule | 2709 | 43 |
| BadComponentReferenceRule | 548 | 0 |
| UnusedCustomPropertiesRule | 213 | 0 |
| ComponentReferenceValidationRule | 0 | 70 |
| PollingIntervalRule | 0 | 23 |
| ExcessiveContextDataRule | 5 | 0 |

Pylint findings in the whitelisted run by category: C 913, W 675, E 295,
R 50; top codes C0103 709, W1405 454, E0602 293, W0311 136, C0301 57,
C0415 42, C0121 40, R1705 38, W1401 36, C0104 28, W0702 25, C0200 21,
W0612 13, C0207 10, W0622 7, E0001 1.

Timing (all 287 views, one process): 31.2 s total, 108.6 ms per view;
PylintScriptRule 26.9 s (86 %), NamePatternRule 1.07 s,
UnusedCustomPropertiesRule 0.94 s, model building 1.86 s, every other rule
under 50 ms in total.

## 15. Appendix: pylint messages enabled by the bundled rcfile

Output of `pylint --rcfile ignition_lint/.config/.ignition-pylintrc --list-msgs-enabled`
with pylint 4.1.3 (345 messages). Messages also disabled by the combined-file
header (unused-argument W0613, missing-module-docstring C0114,
missing-class-docstring C0115, missing-function-docstring C0116,
redefined-outer-name W0621, function-redefined E0102) never appear.

**Fatal (F, 5)**: F0001 fatal, F0002 astroid-error, F0010 parse-error, F0011 config-parse-error, F0202 method-check-failed.

**Error (E, 126)**: E0001 syntax-error, E0011 unrecognized-inline-option, E0013 bad-plugin-value, E0014 bad-configuration-section, E0015 unrecognized-option, E0100 init-is-generator, E0101 return-in-init, E0102 function-redefined, E0103 not-in-loop, E0104 return-outside-function, E0105 yield-outside-function, E0107 nonexistent-operator, E0108 duplicate-argument-name, E0110 abstract-class-instantiated, E0111 bad-reversed-sequence, E0112 too-many-star-expressions, E0113 invalid-star-assignment-target, E0114 star-needs-assignment-target, E0115 nonlocal-and-global, E0117 nonlocal-without-binding, E0118 used-prior-global-declaration, E0119 misplaced-format-function, E0202 method-hidden, E0203 access-member-before-definition, E0211 no-method-argument, E0213 no-self-argument, E0236 invalid-slots-object, E0237 assigning-non-slot, E0238 invalid-slots, E0239 inherit-non-class, E0240 inconsistent-mro, E0241 duplicate-bases, E0242 class-variable-slots-conflict, E0243 invalid-class-object, E0244 invalid-enum-extension, E0245 declare-non-slot, E0301 non-iterator-returned, E0302 unexpected-special-method-signature, E0303 invalid-length-returned, E0304 invalid-bool-returned, E0305 invalid-index-returned, E0306 invalid-repr-returned, E0307 invalid-str-returned, E0308 invalid-bytes-returned, E0309 invalid-hash-returned, E0310 invalid-length-hint-returned, E0311 invalid-format-returned, E0312 invalid-getnewargs-returned, E0313 invalid-getnewargs-ex-returned, E0402 relative-beyond-top-level, E0601 used-before-assignment, E0602 undefined-variable, E0603 undefined-all-variable, E0604 invalid-all-object, E0605 invalid-all-format, E0606 possibly-used-before-assignment, E0633 unpacking-non-sequence, E0643 potential-index-error, E0701 bad-except-order, E0702 raising-bad-type, E0704 misplaced-bare-raise, E0705 bad-exception-cause, E0710 raising-non-exception, E0711 notimplemented-raised, E0712 catching-non-exception, E1003 bad-super-call, E1102 not-callable, E1111 assignment-from-no-return, E1120 no-value-for-parameter, E1121 too-many-function-args, E1123 unexpected-keyword-arg, E1124 redundant-keyword-arg, E1125 missing-kwoa, E1126 invalid-sequence-index, E1127 invalid-slice-index, E1128 assignment-from-none, E1129 not-context-manager, E1130 invalid-unary-operand-type, E1131 unsupported-binary-operation, E1132 repeated-keyword, E1133 not-an-iterable, E1134 not-a-mapping, E1135 unsupported-membership-test, E1136 unsubscriptable-object, E1137 unsupported-assignment-operation, E1138 unsupported-delete-operation, E1139 invalid-metaclass, E1141 dict-iter-missing-items, E1142 await-outside-async, E1143 unhashable-member, E1144 invalid-slice-step, E1145 async-context-manager-with-regular-with, E1200 logging-unsupported-format, E1201 logging-format-truncated, E1205 logging-too-many-args, E1206 logging-too-few-args, E1300 bad-format-character, E1301 truncated-format-string, E1302 mixed-format-string, E1303 format-needs-mapping, E1304 missing-format-string-key, E1305 too-many-format-args, E1306 too-few-format-args, E1307 bad-string-format-type, E1310 bad-str-strip-call, E1507 invalid-envvar-value, E1519 singledispatch-method, E1520 singledispatchmethod-function, E1700 yield-inside-async-function, E1701 not-async-context-manager, E1901 bare-name-capture-pattern, E1902 invalid-match-args-definition, E1903 too-many-positional-sub-patterns, E1904 multiple-class-sub-patterns, E2501 invalid-unicode-codec, E2502 bidirectional-unicode, E2510 invalid-character-backspace, E2511 invalid-character-carriage-return, E2512 invalid-character-sub, E2513 invalid-character-esc, E2514 invalid-character-nul, E2515 invalid-character-zero-width-space, E3102 positional-only-arguments-expected, E3701 invalid-field-call, E4702 modified-iterating-dict, E4703 modified-iterating-set.

**Warning (W, 123)**: W0101 unreachable, W0102 dangerous-default-value, W0104 pointless-statement, W0105 pointless-string-statement, W0106 expression-not-assigned, W0107 unnecessary-pass, W0108 unnecessary-lambda, W0109 duplicate-key, W0120 useless-else-on-loop, W0122 exec-used, W0123 eval-used, W0124 confusing-with-statement, W0125 using-constant-test, W0126 missing-parentheses-for-call-in-test, W0127 self-assigning-variable, W0128 redeclared-assigned-name, W0129 assert-on-string-literal, W0130 duplicate-value, W0131 named-expr-without-context, W0133 pointless-exception-statement, W0134 return-in-finally, W0135 contextmanager-generator-missing-cleanup, W0136 continue-in-finally, W0137 break-in-finally, W0143 comparison-with-callable, W0150 lost-exception, W0177 nan-comparison, W0199 assert-on-tuple, W0211 bad-staticmethod-argument, W0212 protected-access, W0213 implicit-flag-alias, W0231 super-init-not-called, W0233 non-parent-init-called, W0236 invalid-overridden-method, W0237 arguments-renamed, W0238 unused-private-member, W0239 overridden-final-method, W0240 subclassed-final-class, W0244 redefined-slots-in-subclass, W0245 super-without-brackets, W0246 useless-parent-delegation, W0301 unnecessary-semicolon, W0311 bad-indentation, W0401 wildcard-import, W0404 reimported, W0407 preferred-module, W0410 misplaced-future, W0416 shadowed-import, W0601 global-variable-undefined, W0602 global-variable-not-assigned, W0604 global-at-module-level, W0611 unused-import, W0612 unused-variable, W0613 unused-argument, W0614 unused-wildcard-import, W0621 redefined-outer-name, W0622 redefined-builtin, W0631 undefined-loop-variable, W0632 unbalanced-tuple-unpacking, W0640 cell-var-from-loop, W0641 possibly-unused-variable, W0642 self-cls-assignment, W0644 unbalanced-dict-unpacking, W0702 bare-except, W0705 duplicate-except, W0706 try-except-raise, W0711 binary-op-exception, W0715 raising-format-tuple, W0716 wrong-exception-operation, W0718 broad-exception-caught, W0719 broad-exception-raised, W1113 keyword-arg-before-vararg, W1114 arguments-out-of-order, W1115 non-str-assignment-to-dunder-name, W1116 isinstance-second-argument-not-valid-type, W1117 kwarg-superseded-by-positional-arg, W1201 logging-not-lazy, W1202 logging-format-interpolation, W1203 logging-fstring-interpolation, W1300 bad-format-string-key, W1301 unused-format-string-key, W1302 bad-format-string, W1303 missing-format-argument-key, W1304 unused-format-string-argument, W1305 format-combined-specification, W1306 missing-format-attribute, W1307 invalid-format-index, W1308 duplicate-string-formatting-argument, W1310 format-string-without-interpolation, W1401 anomalous-backslash-in-string, W1402 anomalous-unicode-escape-in-string, W1404 implicit-str-concat, W1405 inconsistent-quotes, W1501 bad-open-mode, W1503 redundant-unittest-assert, W1506 bad-thread-instantiation, W1507 shallow-copy-environ, W1508 invalid-envvar-default, W1509 subprocess-popen-preexec-fn, W1510 subprocess-run-check, W1514 unspecified-encoding, W1515 forgotten-debug-statement, W1518 method-cache-max-size-none, W2101 useless-with-lock, W2301 unnecessary-ellipsis, W2402 non-ascii-file-name, W2602 using-final-decorator-in-unsupported-version, W2603 using-exception-groups-in-unsupported-version, W2604 using-generic-type-syntax-in-unsupported-version, W2605 using-assignment-expression-in-unsupported-version, W2606 using-positional-only-args-in-unsupported-version, W2607 using-comprehension-unpacking-in-unsupported-version, W3101 missing-timeout, W3301 nested-min-max, W3601 bad-chained-comparison, W4701 modified-iterating-list, W4801 looping-through-iterator, W4901 deprecated-module, W4902 deprecated-method, W4903 deprecated-argument, W4904 deprecated-class, W4905 deprecated-decorator, W4906 deprecated-attribute.

**Convention (C, 45)**: C0103 invalid-name, C0104 disallowed-name, C0105 typevar-name-incorrect-variance, C0112 empty-docstring, C0114 missing-module-docstring, C0115 missing-class-docstring, C0116 missing-function-docstring, C0117 unnecessary-negation, C0121 singleton-comparison, C0123 unidiomatic-typecheck, C0131 typevar-double-variance, C0132 typevar-name-mismatch, C0200 consider-using-enumerate, C0201 consider-iterating-dictionary, C0202 bad-classmethod-argument, C0203 bad-mcs-method-argument, C0204 bad-mcs-classmethod-argument, C0205 single-string-used-for-slots, C0206 consider-using-dict-items, C0207 use-maxsplit-arg, C0208 use-sequence-for-iteration, C0209 consider-using-f-string, C0301 line-too-long, C0302 too-many-lines, C0304 missing-final-newline, C0321 multiple-statements, C0325 superfluous-parens, C0327 mixed-line-endings, C0328 unexpected-line-ending-format, C0401 wrong-spelling-in-comment, C0402 wrong-spelling-in-docstring, C0403 invalid-characters-in-docstring, C0410 multiple-imports, C0412 ungrouped-imports, C0413 wrong-import-position, C0414 useless-import-alias, C0415 import-outside-toplevel, C1802 use-implicit-booleaness-not-len, C1803 use-implicit-booleaness-not-comparison, C2401 non-ascii-name, C2403 non-ascii-module-import, C2503 bad-file-encoding, C2801 unnecessary-dunder-call, C3001 unnecessary-lambda-assignment, C3002 unnecessary-direct-lambda-call.

**Refactor (R, 46)**: R0123 literal-comparison, R0124 comparison-with-itself, R0133 comparison-of-constants, R0202 no-classmethod-decorator, R0203 no-staticmethod-decorator, R0206 property-with-parameters, R0401 cyclic-import, R0402 consider-using-from-import, R1701 consider-merging-isinstance, R1703 simplifiable-if-statement, R1704 redefined-argument-from-local, R1705 no-else-return, R1706 consider-using-ternary, R1707 trailing-comma-tuple, R1708 stop-iteration-return, R1709 simplify-boolean-expression, R1711 useless-return, R1712 consider-swap-variables, R1713 consider-using-join, R1714 consider-using-in, R1715 consider-using-get, R1716 chained-comparison, R1717 consider-using-dict-comprehension, R1718 consider-using-set-comprehension, R1719 simplifiable-if-expression, R1720 no-else-raise, R1721 unnecessary-comprehension, R1722 consider-using-sys-exit, R1723 no-else-break, R1724 no-else-continue, R1726 simplifiable-condition, R1727 condition-evals-to-constant, R1728 consider-using-generator, R1729 use-a-generator, R1730 consider-using-min-builtin, R1731 consider-using-max-builtin, R1732 consider-using-with, R1733 unnecessary-dict-index-lookup, R1734 use-list-literal, R1735 use-dict-literal, R1736 unnecessary-list-index-lookup, R1737 use-yield-from, R1738 chained-comparison-all-equal, R1739 impossible-comparison, R1905 match-class-bind-self, R1906 match-class-positional-attributes.
