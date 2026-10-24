---
title: Migrating from ign-lint
description: Switch from ign-lint to iglint with iglint migrate, or keep ign-lint's command line with iglint ign-lint, without breaking the build.
---

iglint has an equivalent for every rule of [ign-lint](https://github.com/bw-design-group/ignition-lint) 0.6.4 and reads ign-lint's configuration (`rule_config.json`) and whitelist files. There are two ways to switch:

- **Convert once** with `iglint migrate`, and use `iglint check` from then on. Recommended.
- **Keep the old command line** with `iglint ign-lint`, which accepts ign-lint's arguments and prints ign-lint's output format.

## Rule mapping

| ign-lint rule | iglint |
|---|---|
| `NamePatternRule` | [`NAME001`](/reference/rules/name001/) |
| `PollingIntervalRule` | [`BIND004`](/reference/rules/bind004/) |
| `PylintScriptRule` | [`SCR900`](/reference/rules/scr900/) (the real pylint) and every default `SCR` rule that no other ign-lint rule maps to |
| `BadComponentReferenceRule` | [`VIEW018`](/reference/rules/view018/) |
| `ComponentReferenceValidationRule` | [`BIND005`](/reference/rules/bind005/) (expressions and property bindings) and [`SCR305`](/reference/rules/scr305/) (scripts) |
| `UnusedCustomPropertiesRule` | [`VIEW015`](/reference/rules/view015/) |
| `ExcessiveContextDataRule` | [`VIEW006`](/reference/rules/view006/) |
| `PropertyAccessRule` | [`VIEW012`](/reference/rules/view012/) |
| `PropertyPersistenceRule` | [`VIEW013`](/reference/rules/view013/) |

ign-lint's option names are mapped too, so a `rule_config.json` configures the iglint rules (`minimum_interval` becomes `minimum-interval`, and so on).

## Option 1: `iglint migrate`

Run it next to `rule_config.json`. `--dry-run` prints the result without writing files:

```sh
iglint migrate --dry-run
```

For a `rule_config.json` that configures `NamePatternRule` and `PollingIntervalRule` and disables `PylintScriptRule`, the output starts like this (shortened):

```toml
# iglint configuration, migrated from rule_config.json by `iglint migrate`.

[rules.NAME001]
# name-pattern: Name does not follow the configured naming convention, from ign-lint NamePatternRule.
convention = "PascalCase"
target-node-types = ["component"]

[rules.BIND004]
# polling-interval: Expression polls faster than the minimum interval, from ign-lint PollingIntervalRule.
severity = "error"
minimum-interval = 5000

[rules.SCR001]
# syntax-error: Script does not parse with Jython 2.7, from ign-lint PylintScriptRule.
enabled = false

# ... one entry per native SCR rule, all disabled because PylintScriptRule is ...

# ign-lint runs every rule that is not disabled, on `view.json` files only.
[[overrides]]
files = ["**/view.json"]
select = ["NAME001", "VIEW012", "VIEW013", "VIEW018"]
```

followed by a summary of the mapping:

```text
  NamePatternRule -> NAME001
  PollingIntervalRule -> BIND004
  PylintScriptRule: disabled (SCR001, SCR002, SCR003, SCR101, ...)
  BadComponentReferenceRule -> VIEW018
  ComponentReferenceValidationRule -> BIND005, SCR305
  ...
  NAME001, VIEW012, VIEW013, VIEW018 run on `**/view.json` files only, like ign-lint
  1 whitelisted file(s) would be linted, with their findings written to .iglint-baseline.json
```

What it does:

- Each ign-lint rule becomes its iglint rule with the same options and severity. Most ign-lint rules default to `error` where the iglint rule defaults to `warning`, so the migrated file sets `severity = "error"` for them, and the build fails on the same findings as before.
- Disabling `PylintScriptRule` disables the native `SCR` rules it maps to, so the migrated configuration reports what ign-lint did. Remove those `enabled = false` entries to get iglint's script checks, which is the main reason to switch.
- ign-lint lints only `view.json` files, so the rules it runs on top of iglint's defaults are selected for views only, in an `[[overrides]]` entry with `files = ["**/view.json"]`. Library modules, gateway events and WebDev scripts get iglint's native rules. Move a rule out of that entry (into `extend-select`) to run it everywhere.
- If an ign-lint whitelist exists (`.ignition-lint-whitelist.txt` or `.whitelist.txt` next to the configuration, or `--whitelist FILE`), the whitelisted files' current findings go into a [baseline](/guides/baseline/) instead. Those files are linted from now on, and only their new findings fail.

Then write the files and gate on `iglint check`:

```sh
iglint migrate          # writes iglint.toml (and .iglint-baseline.json)
iglint baseline         # record today's findings of the other files too
iglint check
```

`--output`, `--baseline` and `--force` choose where the files go and whether existing files are overwritten.

## Option 2: `iglint ign-lint`

`iglint ign-lint [ARGS]` accepts ign-lint 0.6.4's arguments (`--config`, `--files`, `--whitelist`, `--ignore-warnings`, `--results-output`, `--fix`, `--generate-whitelist` and the debug options) and prints ign-lint's output format, with ign-lint's node paths:

```text
$ iglint ign-lint --config rule_config.json --files '**/view.json'
🔧 Loaded configuration from rule_config.json

📄 Evaluating file:
    projects/demo-hmi/com.inductiveautomation.perspective/views/Line/Overview/view.json

❌ Found 3 errors:

  PollingIntervalRule:
    • root.root.children[1].Clock.propConfig.props.text: 'dateFormat(now(1000), "HH:mm:ss")'


  ComponentReferenceValidationRule:
    • root.root.children[0].Title.propConfig.props.text: `view.custom.aera`: custom property `aera` is not defined


  UnusedCustomPropertiesRule:
    • custom.legacyFilter: custom property 'legacyFilter' is defined but never referenced
```

Called through a link named `ign-lint`, the binary behaves the same, so existing scripts keep working:

```sh
ln -s "$(command -v iglint)" /usr/local/bin/ign-lint
ign-lint --config rule_config.json --files "**/view.json"
```

There is also a drop-in [pre-commit hook](/guides/pre-commit/#replacing-ign-lints-hook).

`iglint check` also reads a `rule_config.json` when no `iglint.toml` exists, with a deprecation warning.

## Expect more findings

:::caution
`iglint ign-lint` finds more than ign-lint, so a CI gate that passes with ign-lint can fail after the switch.
:::

For example, `BIND005` reports references to view `custom` properties and `params` that do not exist, which ign-lint's `ComponentReferenceValidationRule` does not check, and `PylintScriptRule` also runs iglint's own script rules. These extra findings are real: a reference that resolves nowhere evaluates to `null` with a bad quality.

`iglint ign-lint` has no baseline (ign-lint's `--whitelist` skips whole files). To adopt iglint without breaking the build, run `iglint migrate`, then `iglint baseline`, and gate on `iglint check`: today's findings are recorded and only new ones fail.

## Intended differences

iglint reproduces ign-lint's findings except where ign-lint is wrong. Each difference is listed, with its reason, in [Differences from ign-lint](/reference/ign-lint-differences/), and pinned by a parity test against output recorded from real ign-lint 0.6.4. The main ones:

- One invalid JSON file is reported (`VIEW001`) and the other files are still linted; ign-lint aborts.
- Deeply nested views are linted; ign-lint aborts the whole run with a `RecursionError` from about 250 nested components.
- References are matched exactly: `{view.custom.foobar}` does not count as a use of `foo`, and comments and strings never match a component reference pattern.
- Scripts are linted one by one, as Ignition compiles them, instead of as one combined file per view. Messages caused by the combined file (module-level naming, quote consistency across scripts) are gone, and names Ignition injects (`system`, the project's library packages) are not reported as undefined.
- `PylintScriptRule` findings depend on the installed pylint, so they are compared by message code, not as part of the parity test.

## Speed

On the synthetic benchmark corpora, `iglint ign-lint` does the same work as ign-lint 7x (50 views) to 43x (20,000 views) faster with pylint, and 11x to 53x faster without, on a 16-CPU machine. On a production customer project of 287 views, with its own configuration and whitelist, `iglint ign-lint` was 14x faster than ign-lint's best configuration with pylint and 23x without. Details and the caveats are in [Performance and caching](/guides/performance/).
