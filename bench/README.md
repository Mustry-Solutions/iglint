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
CPU_LIMITS="2" bench/run.sh medium        # pinned runs on 2 CPUs only
```

`run.sh private` benchmarks a copy of the corpus (`$BENCH_HOME/private-corpus`,
refreshed with `rsync`): the incremental runs edit a view.

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
| `iglint-migrated` | the migration path, in a copy of the corpus (`$BENCH_HOME/migrated/<corpus>`): `iglint migrate` of ign-lint's configuration (`ign-lint-default`'s, so `SCR900` runs on views) and whitelist, `iglint baseline`, then `iglint check --no-cache` as the CI gate (pylint on `PATH`) |
| `cpu<N>-*` | pinned to N CPUs with `taskset -c 0-<N-1>` (`CPU_LIMITS`, default `2 1`; a hosted CI agent has 2): `ign-lint-default`, `ign-lint-jobs1` (when in `IGN_LINT_VARIANTS`), `iglint-ign-lint-pylint`, `iglint-migrated` and `iglint-cold` |

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
speedup with pylint stays below 10x (small: 50 views, 6.9x); from a few
hundred views on it is well above (medium 28x, large 42x). Without pylint
the speedup is 10x or more from about 300 views on (private project, 287
views: 23x; medium 16x; large 25x). On the small corpus it is 8x to 11x
depending on the run: iglint takes about 20 ms there, where a few
milliseconds of noise move the ratio (11x in the run below, 8.3x measured
by a reviewer on a busier machine).

### Where the time goes on a small corpus

`iglint ign-lint` with `bench/configs/rules-only.json` (minimum of 30
runs, all CPUs, 2026-10-24):

| | wall time |
|---|---|
| empty directory (process start, configuration, no files) | 2.3 ms |
| the small corpus, `--files` matching nothing (discovery only) | 2.4 ms |
| one view alone in a project | 4.3 ms |
| one view, with the rest of the small repository (script library, gateway configuration, tags) | 14.4 ms |
| the whole small corpus (50 views) | 19.9 ms |
| the same on one CPU | 64.0 ms |

So there is no fixed start-up cost worth removing: starting the process
and reading the configuration take about 2 ms. Most of the time on a
small corpus is reading and indexing the repository's other resources,
which iglint uses to resolve what views refer to; each further view
costs about 0.1 ms.

## Results

Development machine: 16 CPUs (8 cores, 2 threads each), 23 GB RAM, Linux.
Measured 2026-10-24 on branch `fix-r21` (release build, one benchmark at
a time); the machine also ran an Ignition gateway and SQL Server in
containers (load average about 1.5), so iglint's times are 10 to 60%
above those of an earlier, idle run of the same code (main and the branch
measured side by side give the same `iglint ign-lint` times; `iglint
check` cold on medium is 5% slower on the branch, the safety net's
resolution map). Minimum wall times; `x` is ign-lint's best working
configuration (`jobs1`, see the batch-mode note above) divided by
iglint's. The xl corpus was not re-measured; its column is from the
earlier idle run.

| | small (50 views) | medium (500) | large (5,000) | xl (20,000, earlier run) |
|---|---|---|---|---|
| ign-lint, as shipped (`jobs=4`) | 5.30 s | 73.6 s | not run ¹ | not run ¹ |
| ign-lint `jobs=1` (best working) | 1.64 s | 16.4 s | 177 s | 713 s |
| ign-lint `batch_mode` + `jobs=1` (broken output) | 1.13 s | 16.9 s | not run ¹ | not run ¹ |
| ign-lint without pylint | 226 ms | 1.60 s | 15.4 s | 65.6 s |
| `iglint ign-lint`, with pylint (equal work) | 236 ms (6.9x) | 587 ms (**28x**) | 4.25 s (**42x**) | 16.5 s (**43x**) |
| same, one CPU | 631 ms (2.6x) | 4.95 s (3.3x) | 49.0 s (3.6x) | 186 s (3.8x) |
| `iglint ign-lint`, without pylint (equal work) | 21 ms (**11x**) | 103 ms (**16x**) | 614 ms (**25x**) | 1.24 s (**53x**) |
| same, one CPU | 63 ms (3.6x) | 584 ms (2.7x) | 3.70 s (4.2x) | 5.47 s (12x) |
| `iglint check`, ign-lint's rules, whole repository | 25 ms (9.0x) | 137 ms (12x) | 768 ms (20x) | 1.61 s (41x) |
| `iglint check`, default rules, cold | 29 ms (57x vs best) | 164 ms (100x) | 878 ms (202x) | 1.89 s (377x) |
| same, one CPU | 124 ms | 1.16 s | 6.61 s | 11.0 s |
| cold, writing the cache | 32 ms | 189 ms | 980 ms | 2.13 s |
| warm cache / one view changed | 23 / 23 ms | 126 / 125 ms | 658 / 664 ms | 1.51 / 1.51 s |
| `--select ALL` | 30 ms | 169 ms | 964 ms | 2.42 s |
| peak RSS: ign-lint `jobs=1` / `iglint check` | 66 / 174 MB | 193 / 304 MB | 1,427 / 1,115 MB | 5,560 / 3,195 MB |

¹ `jobs=4` and batch mode grow superlinearly (about 20 minutes each on
large); they are never the best working configuration.

"Without pylint" ratios compare ign-lint without pylint to `iglint ign-lint`
without pylint. "One CPU" is `taskset -c 0`, a CPU the rest of the machine
also uses; on 2 CPUs (below) iglint is faster per CPU.

Not reached: 10x with pylint on the small corpus (pylint's fixed start-up
and set-up cost of about 0.2 s against ign-lint's 1.64 s); 10x for
`iglint check` with ign-lint's rules on the small corpus (9.0x); 10x
without pylint on the small corpus in every run (8x to 11x, see above);
and 10x on one CPU in every column of the re-measured corpora. Everything
else is 10x or more, and the speedup grows with the corpus.

On a private production project (287 views, its CI command with its own
`rule_config.json` and whitelist; aggregates only; measured on a copy of
the project's revision of 2026-10-21, so the numbers depend on that
revision), same run: ign-lint as shipped 21.2 s, `jobs=1` 6.12 s, batch
mode 2.02 s (broken output), without pylint 1.89 s; `iglint ign-lint`
with pylint 436 ms (**14x**; 1.72 s on one CPU, 3.6x), without pylint
83 ms (**23x**; 320 ms on one CPU, 5.9x); `iglint check` with ign-lint's
rules 120 ms (**16x**), `iglint check --no-cache projects services`
124 ms (540 ms on one CPU); the migrated gate 613 ms (10x).

### Pinned to 2 CPUs, and the migration path

The CI today runs ign-lint as shipped (bundled rcfile, `jobs=4`) on a
hosted agent with 2 CPUs. Same machine and run, pinned with `taskset -c
0-1` (minimum wall times; x = ign-lint as shipped / this). The large
corpus was run without `ign-lint-default` (about 20 minutes per run), so
its ratios are against `jobs=1`. The runs pinned to one CPU of the earlier
idle run are not repeated here.

| | small, 2 CPUs | medium, 2 CPUs | large, 2 CPUs | private, 2 CPUs |
|---|---|---|---|---|
| ign-lint as shipped (`jobs=4`) | 5.55 s | 75.8 s | not run | 22.2 s |
| ign-lint `jobs=1` | 1.64 s (3.4x) | 16.4 s (4.6x) | 174.5 s | 6.15 s (3.6x) |
| `iglint ign-lint`, with pylint | 377 ms (**15x**) | 2.74 s (**28x**) | 26.4 s (6.6x vs `jobs=1`) | 1.10 s (**20x**) |
| `iglint migrate` + `iglint baseline`, then `iglint check` | 414 ms (**13x**) | 3.08 s (**25x**) | 27.8 s (6.3x vs `jobs=1`) | 1.49 s (**15x**) |
| `iglint check`, default rules | 70 ms (79x) | 611 ms (124x) | 3.53 s (49x vs `jobs=1`) | 327 ms (68x) |
| ign-lint without pylint / `iglint ign-lint` without pylint | 228 ms / 37 ms (6.1x) | 1.59 s / 316 ms (5.0x) | 15.4 s / 2.01 s (7.7x) | 1.90 s / 198 ms (9.6x) |

On 2 CPUs iglint is 10x or more faster than ign-lint as shipped (the CI
today) on every corpus measured against it, with and without the
migration; against ign-lint's best configuration on 2 CPUs it is 4x to 7x
with pylint (pylint's own per-module cost) and 5x to 10x without.

All CPUs (16) for comparison: migrated `iglint check` 243 ms (small),
633 ms (medium), 4.54 s (large; ign-lint `jobs=1` 177 s).

With pylint, iglint's time on few CPUs is pylint's own per-module cost:
`iglint migrate` keeps ign-lint's scope (`SCR900` on `view.json` files
only), so the migrated gate runs pylint on the same scripts as
`iglint ign-lint` plus the native rules on the whole repository, about
10% more. Before that scope fix the migrated gate ran pylint on library
modules and WebDev scripts as well (on the private project 4.4x instead of
16x on 2 CPUs).

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

Memory and the cache:

* Peak RSS on large went from 1.92 GB to 1.07 GB at the same speed. Per-unit
  caches (Python parse trees above all, about 25 bytes per byte of
  Python) now live for one phase only; phase B re-parses the scripts of the
  units it lints, and suppression state and pylint results are carried
  over. JSON arrays and objects are built on a scratch stack and allocated
  at their exact size (the tree went from about 6 to 4 bytes per byte of
  input, and the `Vec<Member>` growth hotspot is gone).
* The cache context is now the layout, the manifests and a hash of every
  resource summary, instead of the text of every file. A change that
  leaves the summaries as they were keeps every other unit's results:
  on large, one changed view costs 0.56 s (as much as a warm run) instead
  of 0.77 s. A cold run that writes the cache parses everything once more
  (0.81 s instead of 0.68 s without the cache). `crates/iglint_linter/tests/incremental.rs`
  checks every one-file change of the fixture workspace (and a dependent
  view) against an uncached run.

Remaining hotspots (large corpus):

1. Memory: still about 5 bytes per byte of input. `JsonString` keeps the
   raw text slice next to the decoded `Cow` (48 bytes per string), and
   Python syntax trees own every identifier as a `String`.
2. Warm runs still parse every file to build the summaries; caching the
   summaries themselves (they are owned data) would let unchanged files
   skip parsing.
3. Script parsing and the semantic model: about 25% of the samples.
4. Discovery walks the tree twice (engine discovery and
   `Workspace::discover`, 30 + 10 ms on the private project) on one thread.
5. Text output: `display_line` and `LineIndex::new` for code frames (6%).

## Parity

```sh
bench/parity.py ~/.cache/iglint-bench/corpora/medium -- --config bench/configs/rules-only.json --files 'projects/**/view.json'
```

Real ign-lint 0.6.4 stages the scripts of a view into a debug directory
whenever pylint reports something, whatever `debug` says. Without a
`debug_dir` in its configuration it picks one itself: `tests/debug` in
the nearest ancestor that is or holds a `tests` directory (for the
bundled fixtures, `crates/iglint_linter/tests/debug` in this
repository), else `.ignition-lint/debug/` in the corpus. So running it
on a corpus writes into the corpus or the source tree. `parity.py` never
does that: it passes ign-lint a scratch copy of the `--config` (or, without
one, of the corpus's `rule_config.json`, which ign-lint reads by
default) with `debug` off and every `debug_dir` pointed into `--out`,
and gives `PylintScriptRule` (the only rule with debug output) both
settings when its entry or the keys are absent, unless it is disabled;
`--debug-output` is pointed into `--out` too. `cargo test -p iglint
--test bench_parity` checks that a run with pylint enabled leaves `git
status` unchanged.
Pass the project's own `--whitelist` so both linters skip the same legacy
views (otherwise `iglint` reports on files ign-lint was told to ignore).

`parity.py CORPUS [--out DIR] [--list] [--private] [--reuse] -- IGN-LINT FLAGS`
runs ign-lint and `iglint ign-lint` with the same flags, parses both
`--results-output` reports (multi-line messages included) and matches
findings by file, rule, node path and message. Unmatched findings are
matched again after normalising documented wording differences (bad
component reference pattern counts, name suggestions), then classified:
**expected** (a category of `docs/compat/ign-lint-differences.md`),
**ign-lint-only** (unexplained, potential iglint false negative) or
**iglint-only** (unexplained, potential false positive). Matched findings
(identical, or equal after normalising the wording) are also compared by
severity: the output gives the number of matched findings with the same
severity, the expected severity differences per category (a documented
mapping; none is needed for the compared rules) and the unexplained ones
per rule and severity pair.

**Pylint findings are outside the parity comparison.** Every
`PylintScriptRule` finding (`SCR900` and the native `SCR` rules it maps
to) is set aside before matching: it is in none of the columns
(identical, expected, unexplained) and in no severity comparison, and
the output and `summary.json` (`not compared`) give their number per
side. An earlier version matched them like the other findings and
counted the ones with identical text as identical. Pylint findings are
compared separately,
by file, node path and message code, because ign-lint's combined file
shifts lines and adds file-level messages, and they differ from ign-lint's
by design: iglint drops the messages caused by ign-lint's combined file
and its generated header (`C0103` module-level names, `W1405` quote
consistency across scripts), does not report `E0602` undefined names
that Ignition injects (`system`, the project's library packages), and
reports `W0702` (bare `except:`) once, as `SCR106`, instead of twice (see
`docs/compat/ign-lint-differences.md`, "PylintScriptRule").
Details go to `differences.json` and `severity-differences.json` in the
output directory, the counts to `summary.json`. For a private
corpus (`--private`, or the corpus is `$IGLINT_PRIVATE_CORPUS`) only
aggregates are printed and the output directory must be outside the
repository (default `~/.cache/iglint-bench/parity-private`).

On the bundled fixtures (`crates/iglint_linter/tests/fixtures/ign-lint`,
`--files 'cases/**/view.json'`) every difference is expected and all 147
matched findings have the same severity. `cargo test -p iglint --test
bench_parity` runs that and fails on any unexplained finding or severity
difference; it is skipped without `python3` or the ign-lint virtualenv
(`$IGN_LINT`, else `$BENCH_HOME/venv/bin/ign-lint`).

Results without pylint (`bench/configs/rules-only.json`, seed 1), and the
private project with its own configuration (pylint included):

| corpus | identical | expected | unexplained ign-lint-only | unexplained iglint-only |
|---|---|---|---|---|
| small | 24 | 7 | 0 | 0 |
| medium | 218 | 108 | 0 | 0 |
| large | 2,247 | 1,146 | 0 | 0 |
| private project (aggregates) ³ | 1,463 ² | 915 | 0 | 0 |

² Without the pylint findings, which are not compared (ign-lint 1,933,
iglint 1,220) and are compared by message code instead (below). Counted
the earlier way (pylint findings with the same text as identical, the
others as expected), the same run gives 1,531 identical and 3,932
expected.

³ Numbers from the private project depend on its revision. These were
measured on 2026-10-24, on a copy of the revision of 2026-10-21, with
ign-lint 0.6.4 (0.6.3 gave the same numbers in the earlier count).

Every expected difference has a category in
`docs/compat/ign-lint-differences.md`. The ones found with this tool and
added there: BIND005 checks query parameters and other binding
expressions; NAME001 skips dataset encoding keys and checks
`propConfig`-only component properties; VIEW015 treats script
assignments as writes and stays silent when a script reads a property
container dynamically or calls a library function whose module evaluates
computed strings or reaches a frame (the frame and introspection
analysis treats the call as reading every property it could reach, rounds
4 and 5). The last class covers the private project's `custom`/`params`
differences; re-running with it gives 0 unexplained.

Pylint on the private project, by message code: 1,053 messages on both
sides. ign-lint-only are `E0602` undefined names that iglint provides
(`system`, library packages: 293), messages caused by ign-lint's combined
file (`C0103` module-level names 343, `W1405` quote consistency across
scripts 201), and `W0702` 25, `W0612` 7 and `W0101` 3, which iglint
reports through its native rules (`SCR106`, `SCR103`, `SCR105`) instead.
iglint-only are 32 `W0311` (indentation; ign-lint re-indents scripts), 14
`C0103`, 9 `W1405` and 1 other. With an explicit
`pylintrc`, iglint applies the disables of ign-lint's combined-file header
(SCR900 option `disable`), so equal configurations give the same messages.

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
