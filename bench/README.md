# Benchmarks and parity

Tools to measure iglint against [ign-lint](https://github.com/bw-design-group/ignition-lint)
0.6.4, the linter it replaces. The target (DESIGN.md) is at least 10x faster
than ign-lint in its best configuration, on every corpus.

| Tool | Purpose |
|---|---|
| `bench/gen` (crate `iglint_bench_gen`, binary `iglint-bench-gen`) | Deterministic generator of synthetic Ignition 8.3 repositories |
| `bench/run.sh` | hyperfine comparison of ign-lint configurations and iglint modes |
| `bench/summarize.py` | Markdown tables from a `run.sh` results directory |
| `bench/parity.py` | Finding-by-finding diff of ign-lint and `iglint ign-lint` |
| `bench/check-regression.sh` + `bench/baseline.json` | CI gate: fails on a slowdown over 10% on the medium corpus |
| `bench/configs/` | ign-lint `rule_config.json` files used by the examples below |

Corpora, the ign-lint virtualenv and build output live under
`~/.cache/iglint-bench/` (`$BENCH_HOME`), on disk: the xl corpus alone is
about 1 GB and must not go to a RAM-backed `/tmp`. Results go to
`bench/results/` (gitignored). Only numbers for the public, synthetic
corpora are committed here.

## Synthetic corpora

```sh
cargo run --release -p iglint_bench_gen -- --size medium --seed 1 --out ~/.cache/iglint-bench/corpora/medium
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
library functions), every view property is read and every local variable
is used.

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
generator prints the counts per kind; `iglint check` finds each of them
(unknown style classes only with `--strict`, because the corpus has no
gateway theme).

## Running the benchmark

```sh
bench/run.sh                    # small medium large
bench/run.sh medium             # one corpus
IGN_LINT_VARIANTS="jobs1 rules-only" bench/run.sh large xl
IGLINT_PRIVATE_CORPUS=/path/to/repo bench/run.sh private
```

`run.sh` installs hyperfine (`cargo install hyperfine`) if needed, creates
an ign-lint 0.6.4 virtualenv with `uv` (pylint 4.1.3, astroid 4.3.4, the
versions in `docs/compat/ign-lint.md`), builds the release binaries
(`CARGO_TARGET_DIR` defaults to `$BENCH_HOME/target`), generates missing
corpora and writes hyperfine JSON, `/usr/bin/time -v` reports and
`summary.md` to `bench/results/<timestamp>/`. Other agents and builds on the
same machine add noise: compare minimum times, and rerun final numbers on
a quiet machine.

### What is measured

ign-lint runs from the corpus root with `--files 'projects/**/view.json'`:

| Variant | Configuration |
|---|---|
| `ign-lint-default` | `{}`: every rule with its defaults, as shipped. `PylintScriptRule` uses the bundled rcfile, which sets `jobs=4`: pylint starts a process pool for every view. Grows superlinearly (about 20 minutes on large). |
| `ign-lint-jobs1` | rcfile with `jobs=1`, passed as `PylintScriptRule.kwargs.pylintrc` (the first branch of ign-lint's rcfile resolution; a repository `.config/.ignition-pylintrc` with `jobs=1` has the same effect). **The best working configuration.** |
| `ign-lint-batch`, `ign-lint-batch-jobs1` | `batch_mode: true` (with the bundled or the `jobs=1` rcfile): one pylint run over one combined file at the end. **Broken in 0.6.4:** it runs pylint but prints every pylint finding as an empty `⚠️ PylintScriptRule: ` line, so its output is unusable. Measured for reference only; not counted as "best". |
| `ign-lint-rules-only` | `PylintScriptRule` disabled. |

iglint variants:

| Variant | Command |
|---|---|
| `iglint-ign-lint-pylint` | `iglint ign-lint` with the same flags and the default configuration: all of ign-lint's rules, `PylintScriptRule` as `SCR900` (real pylint, same pylint version) |
| `iglint-ign-lint-rules-only` | the same with `PylintScriptRule` disabled |
| `iglint-check-same-rules` | `iglint check --config <rules-only rule_config.json> .`: the native CLI with ign-lint's rule selection |
| `iglint-cold` | `iglint check --no-cache .` (iglint's default rules) |
| `iglint-cold-write` | empty cache directory before every run, cache written |
| `iglint-warm` | populated cache, nothing changed |
| `iglint-incremental` | populated cache, one `view.json` changed before every run |
| `iglint-select-all` | `--select ALL --no-cache` |
| `*-1core` | the same pinned to one CPU with `taskset -c 0` (pylint workers included) |

Wall time is from hyperfine (minimum and mean ± σ); peak RSS is
hyperfine's per-run maximum resident set size, cross-checked with one
`/usr/bin/time -v` run. With child processes (pylint) both report the
largest single process, not the sum.

### Equal work

* ign-lint lints only `view.json` files, each in isolation, with 7 rules:
  `NamePatternRule`, `PollingIntervalRule`, `PylintScriptRule`,
  `BadComponentReferenceRule`, `ComponentReferenceValidationRule`,
  `UnusedCustomPropertiesRule`, `ExcessiveContextDataRule`
  (`PropertyAccessRule` and `PropertyPersistenceRule` are inert without
  configuration).
* `iglint ign-lint` runs the iglint rules that declare an ign-lint
  equivalent, configured from the same `rule_config.json`, on the same
  files, and writes ign-lint's output. With pylint enabled both sides run
  the same pylint over the same scripts: ign-lint concatenates the scripts
  of a view into one module, iglint lints every script as its own module
  (no messages leaking between scripts, exact lines). These are the
  equal-work comparisons: `ign-lint-jobs1` versus `iglint-ign-lint-pylint`,
  and `ign-lint-rules-only` versus `iglint-ign-lint-rules-only`.
* `iglint check` parses every resource of the repository (views, script
  library, gateway events, named queries, style classes, page
  configuration, tags and UDTs), builds the project index with inheritance
  and runs its default rules, including the native script rules (`SCR`)
  over view scripts and the whole library and the cross-resource `REF`,
  `STYLE` and `TAG` rules. It does more work than ign-lint. With an
  ign-lint `rule_config.json` it selects ign-lint's rules instead
  (`iglint-check-same-rules`), still over the whole repository.

### SCR900 (pylint) speed

SCR900 runs the real pylint, so its time is bounded by pylint itself: about
0.2 s to start and set up a linter, then pylint's per-module cost. iglint
makes it as cheap as pylint allows:

* pylint runs while the native rules run;
* on POSIX one Python process imports pylint, then forks one worker per CPU (at least 8 scripts each), so workers do
  not import pylint again: on 846 view scripts 0.40 s instead of 0.67 s
  wall and 4.3 s instead of 7.3 s CPU compared with pylint's own
  `--jobs=16`, with identical messages (tested against the plain pylint
  command);
* elsewhere, or with a wrapped command, the pylint command runs once with
  `--jobs`.

On very small corpora the fixed pylint cost dominates and the equal-work
speedup with pylint stays below 10x (small: 50 views); from a few hundred
views on it is well above. Without pylint every corpus is above 10x.

## Results (synthetic corpora)

Development machine: 16 CPUs (8 cores, 2 threads each), 23 GB RAM, Linux,
shared with other jobs. Minimum wall times; `x` is ign-lint's time divided
by iglint's. ign-lint's best working configuration is `jobs1` on every
corpus (see the batch-mode note above).

| | small (50 views) | medium (500) | large (5,000) |
|---|---|---|---|
| ign-lint, as shipped (`jobs=4`) | 5.25 s | 72.5 s | 1,292 s ¹ |
| ign-lint `jobs=1` (best working) | 1.62 s | 16.2 s | 176 s |
| ign-lint `batch_mode` + `jobs=1` (broken output) | 1.13 s | 17.0 s | 1,236 s |
| ign-lint without pylint | 232 ms | 1.62 s | 15.7 s |
| `iglint ign-lint`, with pylint (equal work) | 223 ms (**7.3x**) | 550 ms (**29x**) | 3.97 s (**44x**) |
| same, one CPU | 588 ms (2.8x) | 4.51 s (3.6x) | 46.8 s (3.8x) |
| `iglint ign-lint`, without pylint (equal work) | 15 ms (**15x**) | 71 ms (**23x**) | 405 ms (**39x**) |
| same, one CPU | 29 ms (8.0x) | 276 ms (5.9x) | 1.94 s (8.1x) |
| `iglint check`, ign-lint's rules, whole repository | 19 ms (12x) | 100 ms (16x) | 596 ms (26x) |
| `iglint check`, default rules, cold | 22 ms (75x vs best) | 120 ms (134x) | 696 ms (253x) |
| same, one CPU | 70 ms | 656 ms | 3.96 s |
| warm cache / one file changed | 18 / 23 ms | 95 / 134 ms | 553 / 774 ms |
| `--select ALL` | 23 ms | 128 ms | 779 ms |
| peak RSS: ign-lint `jobs=1` / `iglint check` | 66 / 151 MB | 193 / 407 MB | 1,428 / 1,889 MB |

¹ From an earlier run of the same corpus; `ign-lint-batch` (bundled rcfile)
took 1,326 s there.

Not reached yet: with pylint on the small corpus (pylint's fixed start-up
cost) and single-CPU runs. **xl (20,000 views) was not run on this
machine:** iglint's peak memory grows by about 0.38 MB per view (1.9 GB on
large, a projected 7 to 8 GB on xl), more than was free next to the other
jobs. Memory is the first item of the profile below.

On a private production project (287 views, its CI command with its own
`rule_config.json` and whitelist; aggregates only): ign-lint `jobs=1`
6.55 s, batch mode 2.09 s (broken output), without pylint 1.97 s;
`iglint ign-lint` with pylint 462 ms (14x; 1.65 s on one CPU), without
pylint 56 ms (35x); `iglint check --no-cache projects services` 108 ms.

## Profile

Profiled with a `pprof` sampling build (`cargo build --profile profiling`;
`perf` needs privileges this machine does not grant). Fixed in this branch:

* NAME001 rename fixes re-detected the JSON formatting of the whole view
  for every finding (a third of all samples on the private project): the
  style is now detected once per file.
* Discovery normalised every whitelist entry for every file and called
  `getcwd` per file: now one hash lookup.
* Parallel parsing serialised on the system allocator: mimalloc.
* Freeing all parse trees and models at exit took 7% of a run: the CLI
  skips it (`CheckOptions::release_memory`).
* `iglint ign-lint` built ign-lint's flattened node model of every view on
  one thread although only debug options show it.
* SCR900: forking pylint driver, started in parallel with the local rules.

Together: private project 0.33 s to 0.11 s (`iglint check`), 0.29 s to
0.06 s (`iglint ign-lint` without pylint), 0.87 s to 0.46 s with pylint;
large corpus 1.50 s to 0.70 s.

Remaining hotspots (large corpus):

1. Memory: about 9 bytes of model per byte of input. A third of the
   samples are mimalloc initialising fresh pages; `Vec<Member>` growth in
   the JSON parser alone is 13%. Smaller JSON nodes (no owned `Cow`
   strings for keys, exact-capacity member vectors) and dropping each
   resource's syntax tree after its summary would cut both time and the
   xl blocker.
2. The incremental run is slower than a cold run: the cache key includes
   every file of the workspace, so one changed view re-analyses everything
   and rewrites the cache. Per-resource keys plus a project-index key
   would make one-file changes cost one file.
3. Script parsing and the semantic model: about 25%. Each script is
   parsed once (first in the summary phase, `add_script_references`) and
   shared with the rules through the unit cache; the parser and the
   semantic model builder are the next targets.
4. Discovery walks the tree twice (engine discovery and
   `Workspace::discover`, 30 + 10 ms on the private project) on one thread.
5. Text output: `display_line` and `LineIndex::new` for code frames (6%).

## Parity

```sh
bench/parity.py ~/.cache/iglint-bench/corpora/medium -- --config bench/configs/rules-only.json --files 'projects/**/view.json'
```

`parity.py CORPUS [--out DIR] [--list] [--private] [--reuse] -- IGN-LINT FLAGS`
runs ign-lint and `iglint ign-lint` with the same flags, parses both
`--results-output` reports (multi-line messages included) and matches
findings by file, rule, node path and message. Unmatched findings are
matched again after normalising documented wording differences (bad
component reference pattern counts, name suggestions), then classified:
**expected** (a category of `docs/compat/ign-lint-differences.md`),
**ign-lint-only** (unexplained, potential iglint false negative) or
**iglint-only** (unexplained, potential false positive). Pylint findings
are compared separately by file, node path and message code, because
ign-lint's combined file shifts lines and adds file-level messages.
Details go to `differences.json` in the output directory. For a private
corpus (`--private`, or the corpus is `$IGLINT_PRIVATE_CORPUS`) only
aggregates are printed and the output directory must be outside the
repository (default `~/.cache/iglint-bench/parity-private`).

Results without pylint (`bench/configs/rules-only.json`, seed 1):

| corpus | identical | expected | unexplained ign-lint-only | unexplained iglint-only |
|---|---|---|---|---|
| small | 24 | 7 | 0 | 0 |
| medium | 218 | 106 | 0 | 2 |
| large | 2,247 | 1,142 | 0 | 4 |
| private project (aggregates) | 1,482 | 4,044 ² | 0 | 4 |

² Including 3,072 pylint findings counted as "not compared" in the
rule table and compared by message code instead.

* All unexplained synthetic differences are one case: `BIND005` reports
  an unresolved component reference in a **query binding parameter**,
  which ign-lint does not check. The finding is right, but in
  `iglint ign-lint` it carries iglint's own node path
  (`root/A/B.meta.visible@binding.parameters.p`) instead of an ign-lint
  path, and the difference is not documented yet.
* Private project: 3 `NAME001` findings on custom property names and 1
  `VIEW015` finding that ign-lint does not report and no documented
  difference explains (potential false positives, to be checked).
* Pylint, private project, by message code: 1,152 messages on both sides.
  ign-lint-only are `E0602` undefined names that iglint provides
  (`system`, library packages: 279) and messages caused by ign-lint's
  combined file (`C0103` module-level names 343, `W1405` quote consistency
  across scripts 201). iglint-only are 32 `W0311` (indentation, ign-lint
  re-indents scripts), 14 `C0103`, 9 `W1405` and 3 others.
* Pylint, medium corpus, with an explicit `pylintrc`: iglint additionally
  reports `missing-module-docstring` (103), `unused-argument` (70) and
  `missing-function-docstring` (32). ign-lint's combined file disables
  `unused-argument`, `missing-docstring`, `redefined-outer-name` and
  `function-redefined` in its header whatever the rcfile says; iglint
  applies those four only to its bundled rcfile. In `iglint ign-lint` mode
  they should be disabled for a user rcfile too.

## Regression gate

```sh
bench/check-regression.sh            # fails if >10% slower than bench/baseline.json
bench/check-regression.sh --update   # record a baseline (on the CI runner)
```

Lints the medium corpus with `iglint check --no-cache`, 2 warm-up runs and
15 measured runs, and compares the minimum with the baseline's minimum.
The minimum is the least noisy statistic on shared machines. Baselines are
machine-specific; the committed one was recorded on the development
machine (16 CPUs) and must be re-recorded on the CI runner.

## Private corpora

Never commit anything derived from a private corpus except aggregate numbers
(DESIGN.md). `run.sh private` writes to `~/.cache/iglint-bench/private` and
`parity.py --private` to `~/.cache/iglint-bench/parity-private`.
