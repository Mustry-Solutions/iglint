#!/usr/bin/env bash
# CI performance gate: lints the medium synthetic corpus (500 views, seed 1)
# with the release build and fails when the fastest of N runs is more than
# 10% slower than bench/baseline.json.
#
#   bench/check-regression.sh            compare with the baseline
#   bench/check-regression.sh --update   record a new baseline
#
# Noise handling: N runs (default 15, $RUNS) after 2 warm-up runs; the
# minimum is compared, because the minimum is the run least disturbed by
# other load. Baselines are machine-specific: record them on the CI runner.
#
# Environment: RUNS, THRESHOLD (default 0.10), CORPORA_DIR (default
# ~/.cache/iglint-bench/corpora), IGLINT_THREADS (default: all CPUs),
# CARGO_TARGET_DIR.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(dirname "$here")"
baseline="$here/baseline.json"
runs="${RUNS:-15}"
threshold="${THRESHOLD:-0.10}"
corpus="${CORPORA_DIR:-$HOME/.cache/iglint-bench/corpora}/medium"
target="${CARGO_TARGET_DIR:-$repo/target}"

# shellcheck disable=SC1091
[[ -f "$HOME/.cargo/env" ]] && . "$HOME/.cargo/env"
(cd "$repo" && CARGO_TARGET_DIR="$target" cargo build --release -q -p iglint -p iglint_bench_gen)
if [[ ! -f "$corpus/.complete" ]]; then
  mkdir -p "$(dirname "$corpus")"
  "$target/release/iglint-bench-gen" --size medium --seed 1 --out "$corpus" --force >/dev/null
  touch "$corpus/.complete"
fi

threads=()
[[ -n "${IGLINT_THREADS:-}" ]] && threads=(--threads "$IGLINT_THREADS")
python3 -I - "$target/release/iglint" "$corpus" "$runs" "$threshold" "$baseline" "${1:-}" "${threads[@]}" <<'PY'
import json, subprocess, sys, time
iglint, corpus, runs, threshold, baseline, mode, *extra = sys.argv[1:]
cmd = [iglint, "check", "--no-cache", "--exit-zero", "-q", *extra, "."]

def once():
    t = time.perf_counter()
    subprocess.run(cmd, cwd=corpus, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True)
    return time.perf_counter() - t

for _ in range(2):
    once()
times = sorted(once() for _ in range(int(runs)))
best, median = times[0], times[len(times) // 2]
print(f"iglint check, medium corpus: min {best * 1000:.1f} ms, median {median * 1000:.1f} ms ({runs} runs)")
if mode == "--update":
    json.dump({"corpus": "medium", "seed": 1, "command": "iglint check --no-cache", "min_seconds": round(best, 4),
               "median_seconds": round(median, 4)}, open(baseline, "w"), indent=2)
    open(baseline, "a").write("\n")
    print(f"baseline written to {baseline}")
    sys.exit(0)
base = json.load(open(baseline))["min_seconds"]
ratio = best / base
print(f"baseline min {base * 1000:.1f} ms: {ratio:.2f}x")
if ratio > 1 + float(threshold):
    print(f"FAIL: more than {float(threshold):.0%} slower than the baseline")
    sys.exit(1)
print("ok")
PY
