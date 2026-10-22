#!/usr/bin/env bash
# Benchmarks iglint against ign-lint 0.6.4 with hyperfine.
#
#   bench/run.sh [small] [medium] [large] [xl] [private]
#
# Default: small medium large. `private` benchmarks the corpus named by
# $IGLINT_PRIVATE_CORPUS exactly like its CI runs ign-lint; its results go
# to $IGLINT_PRIVATE_RESULTS (default ~/.cache/iglint-bench/private), never
# into the repository. See bench/README.md.
#
# Everything large lives under $BENCH_HOME (default ~/.cache/iglint-bench),
# on disk: corpora, the ign-lint virtualenv, the cargo target directory.
#
# Environment:
#   BENCH_HOME         base directory (default ~/.cache/iglint-bench)
#   RESULTS_DIR        output (default bench/results/<UTC timestamp>)
#   IGN_LINT_VARIANTS  ign-lint variants (default: default jobs1 batch
#                      batch-jobs1 rules-only). `default` (pylint jobs=4)
#                      grows superlinearly: about 20 minutes on large.
#   IGN_LINT_RUNS      hyperfine runs per ign-lint variant (default: 3 below
#                      1,000 views, else 1)
#   IGLINT_RUNS        minimum hyperfine runs per iglint variant (default 10)
#   SKIP_IGN_LINT      1: benchmark iglint only
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(dirname "$here")"
home="${BENCH_HOME:-$HOME/.cache/iglint-bench}"
corpora_dir="$home/corpora"
venv="$home/venv"
results="${RESULTS_DIR:-$repo/bench/results/$(date -u +%Y%m%dT%H%M%SZ)}"
iglint_runs="${IGLINT_RUNS:-10}"
ign_variants="${IGN_LINT_VARIANTS:-default jobs1 batch batch-jobs1 rules-only}"
targets=("$@")
[[ ${#targets[@]} -eq 0 ]] && targets=(small medium large)

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$home/target}"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-6}"
# shellcheck disable=SC1091
[[ -f "$HOME/.cargo/env" ]] && . "$HOME/.cargo/env"
export PATH="$HOME/.local/bin:$HOME/.cargo/bin:$PATH"

command -v hyperfine >/dev/null || cargo install hyperfine
[[ -x /usr/bin/time ]] || { echo "run.sh: GNU time (/usr/bin/time) is required" >&2; exit 2; }
single_core=()
command -v taskset >/dev/null && single_core=(taskset -c 0)

if [[ ! -x "$venv/bin/pylint" ]]; then
  command -v uv >/dev/null || { echo "run.sh: uv is required to create $venv" >&2; exit 2; }
  uv venv -q -p 3.13 "$venv"
  # pylint and astroid as in docs/compat/ign-lint.md (ign-lint pins pylint = "*").
  uv pip install -q -p "$venv" "ign-lint==0.6.4" "pylint==4.1.3" "astroid==4.3.4"
fi
ign_lint="$venv/bin/ign-lint"
# iglint's SCR900 finds pylint on PATH; only the equal-work runs with
# pylint get it, so the native runs measure iglint's own rules.
with_pylint="env PATH=$venv/bin:$PATH"

(cd "$repo" && cargo build --release -q -p iglint -p iglint_bench_gen)
iglint="$CARGO_TARGET_DIR/release/iglint"
gen="$CARGO_TARGET_DIR/release/iglint-bench-gen"

mkdir -p "$results/configs"
configs="$results/configs"
bundled_rc="$("$venv/bin/python" -c 'import ignition_lint, os; print(os.path.join(os.path.dirname(ignition_lint.__file__), ".config", ".ignition-pylintrc"))')"
# The bundled rcfile sets jobs=4: pylint starts a process pool per view.
sed 's/^jobs=.*/jobs=1/' "$bundled_rc" > "$configs/pylintrc-jobs1"

# ign-lint variants as overlays of a base rule_config.json (a dict `c`).
variants=(
  "default|pass"
  "jobs1|c.setdefault('PylintScriptRule', {}).setdefault('kwargs', {})['pylintrc'] = RC"
  "batch|c.setdefault('PylintScriptRule', {}).setdefault('kwargs', {})['batch_mode'] = True"
  "batch-jobs1|k = c.setdefault('PylintScriptRule', {}).setdefault('kwargs', {}); k['batch_mode'] = True; k['pylintrc'] = RC"
  "rules-only|c.setdefault('PylintScriptRule', {})['enabled'] = False"
)

write_configs() { # base-config-or-empty out-prefix
  local base="$1" prefix="$2" v name code
  for v in "${variants[@]}"; do
    name="${v%%|*}"; code="${v#*|}"
    python3 -I - "$base" "$prefix-$name.json" "$configs/pylintrc-jobs1" <<PY
import json, sys
base, out, RC = sys.argv[1], sys.argv[2], sys.argv[3]
c = json.load(open(base, encoding="utf-8")) if base else {}
$code
json.dump(c, open(out, "w", encoding="utf-8"), indent=2)
PY
  done
}

# bench NAME RUNS WARMUP CMD [extra hyperfine args...]; RUNS is a count or
# min:N. hyperfine also records the peak RSS of every run.
bench() {
  local name="$1" runs="$2" warmup="$3" cmd="$4"; shift 4
  local args=(-N --ignore-failure --style basic --export-json "$out/$name.json" --command-name "$name" -w "$warmup")
  [[ "$cmd" == *"'"* || "$cmd" == *"|"* ]] && args=("${args[@]:1}")  # needs a shell
  if [[ "$runs" == min:* ]]; then args+=(-m "${runs#min:}"); else args+=(-r "$runs"); fi
  hyperfine "${args[@]}" "$@" "$cmd" 2>&1 | grep -E "Time|Range" || true
}

# rss NAME CMD: one run under `/usr/bin/time -v` as a cross-check of
# hyperfine's peak RSS. With child processes both report the largest
# single process, not the sum.
rss() {
  local name="$1" cmd="$2"
  /usr/bin/time -v -o "$out/$name.time-v" bash -c "exec $cmd" >/dev/null 2>&1 || true
  sed -n 's/.*Maximum resident set size (kbytes): //p' "$out/$name.time-v" > "$out/$name.rss"
}

run_corpus() { # label corpus-dir ign-lint-flags base-config check-paths
  local label="$1" dir="$2" flags="$3" base="$4" check_paths="$5" n ign_runs v
  out="$results/$label"
  mkdir -p "$out"
  cd "$dir"
  n=$(find . -name view.json -not -path '*/.*' | wc -l)
  ign_runs="${IGN_LINT_RUNS:-$([[ $n -lt 1000 ]] && echo 3 || echo 1)}"
  echo "== $label: $n views, $(nproc) CPUs"
  echo "$n" > "$out/views.txt"
  nproc > "$out/cpus.txt"
  write_configs "$base" "$configs/$label"
  local cache="$results/cache-$label"

  if [[ "${SKIP_IGN_LINT:-0}" != 1 ]]; then
    for v in $ign_variants; do
      bench "ign-lint-$v" "$ign_runs" 0 "$ign_lint --config $configs/$label-$v.json $flags"
    done
    rss ign-lint-rules-only "$ign_lint --config $configs/$label-rules-only.json $flags"
  fi

  # Equal work with ign-lint: its CLI, its configuration, its files.
  local full="$with_pylint $iglint ign-lint --config $configs/$label-default.json $flags"
  local rules="$iglint ign-lint --config $configs/$label-rules-only.json $flags"
  bench iglint-ign-lint-pylint "min:$iglint_runs" 1 "$full"
  [[ ${#single_core[@]} -gt 0 ]] && bench iglint-ign-lint-pylint-1core "min:3" 1 "${single_core[*]} $full"
  rss iglint-ign-lint-pylint "$full"
  bench iglint-ign-lint-rules-only "min:$iglint_runs" 2 "$rules"
  [[ ${#single_core[@]} -gt 0 ]] && bench iglint-ign-lint-rules-only-1core "min:$iglint_runs" 1 "${single_core[*]} $rules"
  rss iglint-ign-lint-rules-only "$rules"
  # Native CLI with ign-lint's rule selection (rule_config.json mapped).
  local check="$iglint check --exit-zero -q"
  bench iglint-check-same-rules "min:$iglint_runs" 2 "$check --no-cache --config $configs/$label-rules-only.json $check_paths"

  # Native CLI, iglint's default rules.
  bench iglint-cold "min:$iglint_runs" 2 "$check --no-cache $check_paths"
  [[ ${#single_core[@]} -gt 0 ]] && bench iglint-cold-1core "min:$iglint_runs" 1 "${single_core[*]} $check --no-cache $check_paths"
  rss iglint-cold "$check --no-cache $check_paths"
  bench iglint-cold-write "min:$iglint_runs" 1 "$check --cache-dir $cache $check_paths" --prepare "rm -rf $cache"
  $check --cache-dir "$cache" $check_paths >/dev/null 2>&1 || true
  bench iglint-warm "min:$iglint_runs" 2 "$check --cache-dir $cache $check_paths"
  rss iglint-warm "$check --cache-dir $cache $check_paths"
  # Incremental: one view's content changes before every run.
  local victim backup
  victim="$(find . -name view.json -not -path '*/.*' | sort | sed -n 1p)"
  backup="$(mktemp -p "$home")"
  cp "$victim" "$backup"
  bench iglint-incremental "min:$iglint_runs" 2 "$check --cache-dir $cache $check_paths" \
    --prepare "perl -0pi -e 's/\\n\\z// or s/\\z/\\n/' $victim"
  cp "$backup" "$victim"
  rm -f "$backup"
  bench iglint-select-all "min:$iglint_runs" 2 "$check --no-cache --select ALL $check_paths"
  rss iglint-select-all "$check --no-cache --select ALL $check_paths"
  rm -rf "$cache"
  cd - >/dev/null
}

for t in "${targets[@]}"; do
  case "$t" in
    small|medium|large|xl)
      dir="$corpora_dir/$t"
      if [[ ! -f "$dir/.complete" ]]; then
        mkdir -p "$corpora_dir"
        "$gen" --size "$t" --seed 1 --out "$dir" --force > "$dir.gen.txt"
        touch "$dir/.complete"
      fi
      run_corpus "$t" "$dir" "--files 'projects/**/view.json'" "" "."
      ;;
    private)
      : "${IGLINT_PRIVATE_CORPUS:?set IGLINT_PRIVATE_CORPUS}"
      private_results="${IGLINT_PRIVATE_RESULTS:-$home/private}"
      case "$private_results" in "$repo"|"$repo"/*) echo "run.sh: private results must be outside the repository" >&2; exit 2;; esac
      saved="$results"; results="$private_results"
      mkdir -p "$results/configs"; cp "$configs/pylintrc-jobs1" "$results/configs/"; configs="$results/configs"
      # Exactly the project's CI invocation of ign-lint.
      run_corpus private "$IGLINT_PRIVATE_CORPUS" \
        "--whitelist .ignition-lint-whitelist.txt --ignore-warnings --files 'projects/**/view.json'" \
        "$IGLINT_PRIVATE_CORPUS/rule_config.json" "--whitelist .ignition-lint-whitelist.txt projects services"
      python3 "$here/summarize.py" "$results" > "$results/summary.md"
      results="$saved"; configs="$results/configs"
      ;;
    *) echo "run.sh: unknown corpus $t" >&2; exit 2 ;;
  esac
done

if ls "$results"/*/views.txt >/dev/null 2>&1; then
  python3 "$here/summarize.py" "$results" > "$results/summary.md"
  cat "$results/summary.md"
fi
