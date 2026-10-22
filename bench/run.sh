#!/usr/bin/env bash
# Benchmarks iglint against ign-lint 0.6.4 with hyperfine.
#
#   bench/run.sh [small] [medium] [large] [xl] [private]
#
# Default: small medium large xl. `private` benchmarks the corpus named by
# $IGLINT_PRIVATE_CORPUS exactly like its CI runs ign-lint; its results go
# to $IGLINT_PRIVATE_RESULTS (default /tmp/iglint-bench-private), never into
# the repository. See bench/README.md.
#
# Environment:
#   CORPORA_DIR      where synthetic corpora live (default /tmp/iglint-corpora)
#   IGN_LINT_VENV    ign-lint virtualenv (default /tmp/ignlint-venv)
#   RESULTS_DIR      output (default bench/results/<UTC timestamp>)
#   IGN_LINT_RUNS    hyperfine runs per ign-lint variant (default: 3 for
#                    corpora under 1,000 views, else 1)
#   IGLINT_RUNS      minimum hyperfine runs per iglint variant (default 10)
#   SKIP_IGN_LINT    set to 1 to benchmark iglint only
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(dirname "$here")"
corpora_dir="${CORPORA_DIR:-/tmp/iglint-corpora}"
venv="${IGN_LINT_VENV:-/tmp/ignlint-venv}"
results="${RESULTS_DIR:-$repo/bench/results/$(date -u +%Y%m%dT%H%M%SZ)}"
iglint_runs="${IGLINT_RUNS:-10}"
targets=("$@")
[[ ${#targets[@]} -eq 0 ]] && targets=(small medium large xl)

# shellcheck disable=SC1091
[[ -f "$HOME/.cargo/env" ]] && . "$HOME/.cargo/env"
export PATH="$HOME/.local/bin:$HOME/.cargo/bin:$PATH"

need() { command -v "$1" >/dev/null 2>&1; }
need hyperfine || cargo install hyperfine
[[ -x /usr/bin/time ]] || { echo "run.sh: /usr/bin/time (GNU time) is required" >&2; exit 2; }

if [[ "${SKIP_IGN_LINT:-0}" != 1 && ! -x "$venv/bin/ign-lint" ]]; then
  need uv || { echo "run.sh: uv is required to create $venv" >&2; exit 2; }
  uv venv -p 3.13 "$venv"
  # pylint and astroid as in docs/compat/ign-lint.md (ign-lint pins pylint = "*").
  uv pip install -p "$venv" "ign-lint==0.6.4" "pylint==4.1.3" "astroid==4.3.4"
fi
ign_lint="$venv/bin/ign-lint"

(cd "$repo" && cargo build --release -q -p iglint -p iglint_bench_gen)
iglint="$repo/target/release/iglint"
gen="$repo/target/release/iglint-bench-gen"

mkdir -p "$results"
configs="$results/configs"
mkdir -p "$configs"
if [[ "${SKIP_IGN_LINT:-0}" != 1 ]]; then
  bundled_rc="$("$venv/bin/python" -c 'import ignition_lint, os; print(os.path.join(os.path.dirname(ignition_lint.__file__), ".config", ".ignition-pylintrc"))')"
  # The bundled rcfile sets jobs=4: pylint starts a process pool per view.
  sed 's/^jobs=.*/jobs=1/' "$bundled_rc" > "$configs/pylintrc-jobs1"
fi

# ign-lint variants as rule_config.json overlays: name -> python expression
# applied to the base config (a dict `c`).
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

# bench NAME RUNS WARMUP CMD [extra hyperfine args...]
bench() {
  local name="$1" runs="$2" warmup="$3" cmd="$4"; shift 4
  local args=(--ignore-failure --style basic --export-json "$out/$name.json" --command-name "$name" -w "$warmup")
  if [[ "$runs" == min:* ]]; then args+=(-m "${runs#min:}"); else args+=(-r "$runs"); fi
  hyperfine "${args[@]}" "$@" "$cmd" | grep -E "Time|Range" || true
}

# Peak RSS: hyperfine records it for every run (getrusage of the child,
# like GNU time). rss NAME CMD adds one run under `/usr/bin/time -v` as a
# cross-check. For commands with child processes (pylint jobs=4) both report
# the largest single process, not the sum.
rss() {
  local name="$1" cmd="$2"
  /usr/bin/time -v -o "$out/$name.time-v" bash -c "exec $cmd" >/dev/null 2>&1 || true
  sed -n 's/.*Maximum resident set size (kbytes): //p' "$out/$name.time-v" > "$out/$name.rss"
}

run_corpus() { # label corpus-dir ign-lint-flags-without-config base-config
  local label="$1" dir="$2" flags="$3" base="$4" n ign_runs
  out="$results/$label"
  mkdir -p "$out"
  cd "$dir"
  n=$(find . -name view.json -not -path '*/.*' | wc -l)
  ign_runs="${IGN_LINT_RUNS:-$([[ $n -lt 1000 ]] && echo 3 || echo 1)}"
  echo "== $label: $n views"
  echo "$n" > "$out/views.txt"
  write_configs "$base" "$configs/$label"
  local cache="$results/cache-$label"

  if [[ "${SKIP_IGN_LINT:-0}" != 1 ]]; then
    for v in default jobs1 batch batch-jobs1 rules-only; do
      local cmd="$ign_lint --config $configs/$label-$v.json $flags"
      bench "ign-lint-$v" "$ign_runs" 0 "$cmd"
    done
    rss ign-lint-rules-only "$ign_lint --config $configs/$label-rules-only.json $flags"
  fi

  local check="$iglint check --exit-zero -q"
  bench iglint-ign-lint-mode "min:$iglint_runs" 2 "$iglint ign-lint --config $configs/$label-rules-only.json $flags"
  rss iglint-ign-lint-mode "$iglint ign-lint --config $configs/$label-rules-only.json $flags"
  bench iglint-cold "min:$iglint_runs" 2 "$check --no-cache $ICHECK"
  rss iglint-cold "$check --no-cache $ICHECK"
  bench iglint-cold-1thread "min:$iglint_runs" 1 "$check --no-cache --threads 1 $ICHECK"
  bench iglint-cold-write "min:$iglint_runs" 1 "$check --cache-dir $cache $ICHECK" --prepare "rm -rf $cache"
  $check --cache-dir "$cache" $ICHECK >/dev/null 2>&1 || true
  bench iglint-warm "min:$iglint_runs" 2 "$check --cache-dir $cache $ICHECK"
  rss iglint-warm "$check --cache-dir $cache $ICHECK"
  # Incremental: one view's content changes before every run.
  local victim backup
  victim="$(find . -name view.json -not -path '*/.*' | sort | head -1)"
  backup="$(mktemp)"
  cp "$victim" "$backup"
  bench iglint-incremental "min:$iglint_runs" 2 "$check --cache-dir $cache $ICHECK" \
    --prepare "perl -0pi -e 's/\\n\\z// or s/\\z/\\n/' $victim"
  cp "$backup" "$victim"
  rm -f "$backup"
  bench iglint-select-all "min:$iglint_runs" 2 "$check --no-cache --select ALL $ICHECK"
  rss iglint-select-all "$check --no-cache --select ALL $ICHECK"
  rm -rf "$cache"
  cd - >/dev/null
}

for t in "${targets[@]}"; do
  case "$t" in
    small|medium|large|xl)
      dir="$corpora_dir/$t"
      if [[ ! -f "$dir/.complete" ]]; then
        "$gen" --size "$t" --seed 1 --out "$dir" --force > "$dir.gen.txt"
        touch "$dir/.complete"
      fi
      ICHECK="." run_corpus "$t" "$dir" "--files 'projects/**/view.json'" ""
      ;;
    private)
      : "${IGLINT_PRIVATE_CORPUS:?set IGLINT_PRIVATE_CORPUS}"
      private_results="${IGLINT_PRIVATE_RESULTS:-/tmp/iglint-bench-private}"
      case "$private_results" in "$repo"|"$repo"/*) echo "run.sh: private results must be outside the repository" >&2; exit 2;; esac
      saved="$results"; results="$private_results"; configs="$results/configs"; mkdir -p "$configs"
      cp "$saved/configs/pylintrc-jobs1" "$configs/" 2>/dev/null || true
      # Exactly the CI invocation (ops/ci/validate.py check_views).
      ICHECK="--config $IGLINT_PRIVATE_CORPUS/rule_config.json --whitelist .ignition-lint-whitelist.txt projects" \
        run_corpus private "$IGLINT_PRIVATE_CORPUS" \
        "--whitelist .ignition-lint-whitelist.txt --ignore-warnings --files 'projects/**/view.json'" \
        "$IGLINT_PRIVATE_CORPUS/rule_config.json"
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
