---
title: Performance and caching
description: How fast iglint is, how it gets there, the result cache, and how to measure it yourself.
---

iglint is designed to be fast without skipping work: each file is read and parsed once, files are processed in parallel, no subprocesses are started (except pylint for `SCR900`, which is off by default), and results are cached between runs.

## Benchmark results

The numbers below are from [`bench/README.md`](https://github.com/Mustry-Solutions/iglint/blob/main/bench/README.md), measured on 2026-10-24 on a development machine with 16 CPUs (8 cores, 2 threads each) and 23 GB of RAM, release build. The corpora are synthetic Ignition repositories made by the deterministic generator in `bench/gen`, with distributions calibrated on the aggregate statistics of a production project (no names or content were used). Times are minimum wall times; `x` is ign-lint's best working configuration (`jobs=1`) divided by iglint's time.

| | small (50 views) | medium (500) | large (5,000) | xl (20,000) ¹ |
|---|---|---|---|---|
| ign-lint `jobs=1` (best working) | 1.64 s | 16.4 s | 177 s | 713 s |
| `iglint ign-lint`, with pylint (equal work) | 236 ms (6.9x) | 587 ms (28x) | 4.25 s (42x) | 16.5 s (43x) |
| ign-lint without pylint | 226 ms | 1.60 s | 15.4 s | 65.6 s |
| `iglint ign-lint`, without pylint (equal work) | 21 ms (11x) | 103 ms (16x) | 614 ms (25x) | 1.24 s (53x) |
| `iglint check`, default rules, cold | 29 ms | 164 ms | 878 ms | 1.89 s |
| `iglint check`, warm cache | 23 ms | 126 ms | 658 ms | 1.51 s |
| peak memory: ign-lint `jobs=1` / `iglint check` | 66 / 174 MB | 193 / 304 MB | 1,427 / 1,115 MB | 5,560 / 3,195 MB |

¹ The xl column is from an earlier run on an idle machine; the others were re-measured while the machine also ran other workloads.

How to read this honestly:

- **Equal work** means the same files, the same rules and, with pylint, the same pylint version over the same scripts: `iglint ign-lint` against ign-lint. `iglint check` does more work than ign-lint (it parses every resource of the repository and runs 88 rules), so its ratios are not like-for-like.
- **With pylint**, iglint's time is mostly pylint's own: about 0.2 s to start, then pylint's cost per module. That is why the 50-view corpus stays below 10x (6.9x).
- **Without pylint**, the speedup is 10x or more from about 300 views on. On the 50-view corpus it is 8x to 11x depending on the run, because iglint takes about 20 ms there and a few milliseconds of noise move the ratio.
- **On fewer CPUs** the ratios are smaller: pinned to one CPU, `iglint ign-lint` is 2.6x to 3.8x faster with pylint and 2.7x to 12x without. On 2 CPUs (a typical hosted CI agent), it is 10x or more faster than ign-lint as shipped (bundled rcfile, `jobs=4`) on every corpus measured, and 4x to 7x faster than ign-lint's best configuration with pylint.

On a production customer project (287 views, with its own configuration and whitelist; aggregate numbers only), `iglint ign-lint` took 436 ms with pylint (14x faster than ign-lint's best configuration) and 83 ms without (23x), and `iglint check` over the whole repository took 124 ms.

There is no fixed start-up cost worth removing: starting the process and reading the configuration take about 2 ms. On a small project, most of the time is spent reading and indexing the rest of the repository (script library, gateway configuration, tags), which iglint needs to resolve what views refer to.

## The result cache

`iglint check` caches results in `.iglint_cache` in the project root. The directory contains a `.gitignore` that ignores everything in it and a `CACHEDIR.TAG`, so it stays out of Git and backups without configuration.

- A file whose content and context have not changed is not linted again.
- The cache context is the layout, the resource manifests and a hash of every resource summary. An edit inside one view or script that does not change those summaries keeps every other file's results.
- A test checks every one-file change of the fixture workspace (and a dependent view) against an uncached run, so the cache cannot hide findings.

Options:

| | |
|---|---|
| `--no-cache` | do not read or write the cache |
| `--cache-dir DIR` or `cache-dir = "..."` | put the cache somewhere else |

## Threads

iglint uses one worker thread per CPU. `--threads N` limits that, for example on a shared build agent.

## Measuring

`--timing` prints the time spent in each phase to standard error. To reproduce the benchmarks, see `bench/run.sh` and `bench/README.md` in the repository: it generates the corpora, installs ign-lint 0.6.4 in a virtual environment and runs [hyperfine](https://github.com/sharkdp/hyperfine) over every configuration. `bench/check-regression.sh` fails on a slowdown of more than 10% on the medium corpus, compared with a recorded, machine-specific baseline.
