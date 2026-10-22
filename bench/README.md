# Benchmarks and parity

Tools to measure iglint against [ign-lint](https://github.com/bw-design-group/ignition-lint)
0.6.4, the linter it replaces. The target (DESIGN.md) is at least 10x faster
than ign-lint in its best configuration, on every corpus.

| Tool | Purpose |
|---|---|
| `crates/iglint_bench_gen` (`iglint-bench-gen`) | Deterministic generator of synthetic Ignition 8.3 repositories |
| `bench/run.sh` | hyperfine comparison of ign-lint configurations and iglint modes |
| `bench/summarize.py` | Markdown tables from a `run.sh` results directory |
| `bench/parity.py` | Finding-by-finding diff of ign-lint and `iglint ign-lint` |
| `bench/check-regression.sh` + `bench/baseline.json` | CI gate: fails on a slowdown over 10% on the medium corpus |

Results go to `bench/results/` (gitignored). Only numbers for the public,
synthetic corpora are committed below.

## Synthetic corpora

```sh
cargo run --release -p iglint_bench_gen -- --size medium --seed 1 --out /tmp/iglint-corpora/medium
```

Sizes: `small` 50 views, `medium` 500, `large` 5,000, `xl` 20,000 (or
`--views N`). The output is a pure function of `--views`, `--seed` and
`--defects` (default 0.05). Each corpus is a repository with two projects
and a gateway configuration tree:

* `projects/bench-base` (inheritable): script library (`ignition/script-python`,
  about 0.75 modules per view, at most 1,500, nested packages, functions
  that call each other and `system.*`), named queries, style classes.
* `projects/bench-hmi` (child of `bench-base`): Perspective views (pages,
  reusable cards, popups, docks), page configuration, session properties,
  gateway events (startup, timer, message, tag change).
* `services/gateway/config/resources/`: config collections with a tag
  provider, tag group, UDT definitions (motor, valve, pump, with parameter
  bindings, alarms and tag event scripts) and one tag folder per line with
  UDT instances.

Views are written the way the Designer writes them: Gson pretty printing,
keys sorted at every level, HTML-safe `=`-style escapes, no trailing
newline. Component trees mix flex and coordinate containers, labels, SVGs,
icons, embedded views, flex repeaters, tables, inputs, buttons and charts;
bindings of every type (property, expression, tag in direct, indirect and
expression mode, expression structure, query, tag history) with map, script,
expression and format transforms; event scripts and actions (script, popup,
navigation), message handlers, custom methods, change scripts and view
events with Jython 2.7 bodies that call the generated library and
`system.*`. Apart from seeded defects every reference resolves (views, view
parameters, named queries and their parameters, style classes, tags, UDTs,
library functions) and every view property is read.

Distributions (components per view, children per container, leaf type mix,
binding type mix, transforms per binding, action types, script length
histogram, view parameters and custom properties) were calibrated on the
aggregate statistics of a production project; no names or content were
used. Mean view size is about 22 KB and 14 components.

Seeded defects (each opportunity becomes a defect with probability
`--defects`, some scaled down): unused custom properties, oversized custom
data, expression syntax errors, unknown expression functions, fast `now()`
polling, unresolved component references, undefined names in scripts,
invalid component names, duplicate sibling names, unknown views, view
parameters and named queries, unknown style classes, message handlers that
listen to no scope, images without alt text, unknown UDT types. The
generator prints the counts per kind.

## Running the benchmark

```sh
bench/run.sh                    # small medium large xl
bench/run.sh medium             # one corpus
IGLINT_PRIVATE_CORPUS=/path/to/repo bench/run.sh private
```

`run.sh` installs hyperfine (`cargo install hyperfine`) if needed, creates
an ign-lint 0.6.4 virtualenv with `uv` in `/tmp/ignlint-venv` (pylint 4.1.3,
astroid 4.3.4, the versions in `docs/compat/ign-lint.md`), builds the
release binaries, generates missing corpora in `/tmp/iglint-corpora` and
writes hyperfine JSON, `/usr/bin/time -v` reports and `summary.md` to
`bench/results/<timestamp>/`.

### What is measured

ign-lint runs from the corpus root with `--files 'projects/**/view.json'`:

| Variant | Configuration |
|---|---|
| `ign-lint-default` | `{}`: every rule with its defaults, as shipped. `PylintScriptRule` uses the bundled rcfile, which sets `jobs=4`: pylint starts a process pool for every view. |
| `ign-lint-jobs1` | rcfile with `jobs=1`, passed as `PylintScriptRule.kwargs.pylintrc`. This is the first branch of ign-lint's rcfile resolution; a repository `.config/.ignition-pylintrc` with `jobs=1` is found by the fallback branch and has the same effect. |
| `ign-lint-batch` | `batch_mode: true`: one pylint run over all views at the end. |
| `ign-lint-batch-jobs1` | both. |
| `ign-lint-rules-only` | `PylintScriptRule` disabled. |

**ign-lint best** is the fastest of the first four. iglint variants:

| Variant | Command |
|---|---|
| `iglint-cold` | `iglint check --no-cache .` (default rule set, all CPUs) |
| `iglint-cold-1thread` | same with `--threads 1` |
| `iglint-cold-write` | empty cache directory before every run, cache written |
| `iglint-warm` | populated cache, nothing changed |
| `iglint-incremental` | populated cache, one `view.json` changed before every run |
| `iglint-select-all` | `--select ALL --no-cache` |
| `iglint-ign-lint-mode` | `iglint ign-lint` with ign-lint's flags and the rules-only configuration |

Wall time is hyperfine's mean (minimum also reported); peak RSS is
hyperfine's per-run maximum resident set size, cross-checked with one
`/usr/bin/time -v` run. For pylint with `jobs=4` both report the largest
single process, not the sum of the pool.

### Equal work

* ign-lint lints only `view.json` files, each in isolation, with 7 rules:
  `NamePatternRule`, `PollingIntervalRule`, `PylintScriptRule`,
  `BadComponentReferenceRule`, `ComponentReferenceValidationRule`,
  `UnusedCustomPropertiesRule`, `ExcessiveContextDataRule`
  (`PropertyAccessRule` and `PropertyPersistenceRule` are inert without
  configuration).
* `iglint check` (default set) parses every resource of the repository
  (views, script library, gateway events, named queries, style classes,
  page configuration, tags and UDTs), builds the project index with
  inheritance and runs the default rules: view, binding and expression
  rules, the native script rules (`SCR`, the equivalent of the pylint
  checks, over view scripts and the whole library) and the cross-resource
  `REF`, `STYLE` and `TAG` rules. `NAME001`, `VIEW012`, `VIEW013` and
  `VIEW018` are not in the default set. It therefore does strictly more
  work than ign-lint; `--select ALL` adds the opt-in rules.
* `iglint ign-lint` runs exactly the iglint rules that declare an ign-lint
  equivalent, configured from the same `rule_config.json`, on the same
  files, and writes ign-lint's output. `PylintScriptRule` has no mapped
  equivalent yet, so the equal-work pair is `ign-lint-rules-only` versus
  `iglint-ign-lint-mode`.

## Results (synthetic corpora)

RESULTS_PLACEHOLDER

## Parity

```sh
bench/parity.py /tmp/iglint-corpora/medium -- --config bench/configs/rules-only.json --files 'projects/**/view.json'
```

`parity.py CORPUS [--out DIR] [--list] [--private] -- IGN-LINT FLAGS` runs
ign-lint and `iglint ign-lint` with the same flags, parses both
`--results-output` reports and matches findings by file, rule, node path and
message. Unmatched findings are matched again after normalising documented
wording differences (bad component reference pattern counts, name
suggestions), then classified: **expected** (a category of
`docs/compat/ign-lint-differences.md`), **ign-lint-only** (unexplained,
potential iglint false negative) or **iglint-only** (unexplained, potential
false positive). Details go to `differences.json` in the output directory.
For a private corpus (`--private`, or the corpus is
`$IGLINT_PRIVATE_CORPUS`) only aggregates are printed and the output
directory must be outside the repository (default
`/tmp/iglint-parity-private`).

PARITY_PLACEHOLDER

## Regression gate

```sh
bench/check-regression.sh            # fails if >10% slower than bench/baseline.json
bench/check-regression.sh --update   # record a baseline (on the CI runner)
```

Lints the medium corpus (generated into `target/bench-corpora/`) with
`iglint check --no-cache`, 2 warm-up runs and 15 measured runs, and compares
the minimum with the baseline's minimum. The minimum is the least noisy
statistic on shared machines. Baselines are machine-specific; the committed
one was recorded on the development machine and must be re-recorded on the
CI runner.

## Private corpora

Never commit anything derived from a private corpus except aggregate numbers
(DESIGN.md). `run.sh private` writes to `/tmp/iglint-bench-private` and
`parity.py --private` to `/tmp/iglint-parity-private`.
