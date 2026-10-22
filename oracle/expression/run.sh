#!/usr/bin/env bash
# Differential test of iglint_expression against Ignition's own expression
# parser (ELParser), Perspective's PropertyRef/LegacyPropertyRef and the tag
# path parser (TagPathParser), run through Jython inside the Ignition image.
# All inputs are generated from seeds by generate.py; no project data is used.
#
#   oracle/expression/run.sh record
#       Regenerates the recorded expectations for the fixed seed set in
#       crates/iglint_expression/tests/oracle/*.txt. The test
#       `cargo test -p iglint_expression --test oracle` compares against them
#       without Docker.
#   oracle/expression/run.sh random [SEED] [COUNT]
#       Fresh run with a random (or given) seed and COUNT cases per kind
#       (default 20000), then runs the ignored test `oracle_random_run` on it.
#
# Environment: IGLINT_ORACLE_IMAGE (default inductiveautomation/ignition:8.3.3).
# Requires docker, python3 and cargo.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
image="${IGLINT_ORACLE_IMAGE:-inductiveautomation/ignition:8.3.3}"
recorded="$root/crates/iglint_expression/tests/oracle"

# Containers are named iglint-oracle-<pid>-<kind>-<n> and removed when the
# script ends or is interrupted (a killed `docker run` client leaves the
# container running).
containers=()
runs=0

# oracle KIND CASES_FILE OUT_FILE (both in $work)
oracle() {
  runs=$((runs + 1))
  local container="iglint-oracle-$$-expression-$1-$runs"
  containers+=("$container")
  docker run --rm --name "$container" --user "$(id -u):$(id -g)" \
    -v "$here:/oracle:ro" -v "$work:/work" \
    --entrypoint /usr/local/bin/ignition/lib/runtime/jre-nix/bin/java \
    "$image" \
    -Dpython.cachedir=/work/.jython-cache \
    -cp '/usr/local/bin/ignition/lib/core/common/*' \
    org.python.util.jython -S /oracle/ignition_oracle.py "$1" "/work/$2" "/work/$3" \
    2>&1 | { grep -v ' DEBUG ' || true; }
}

# expect KIND SEED COUNT OUT_PATH
expect() {
  python3 "$here/generate.py" "$1" "$2" "$3" "$work/$1.cases"
  oracle "$1" "$1.cases" "$1.results"
  python3 "$here/generate.py" merge "$work/$1.cases" "$work/$1.results" "$4"
  echo "$1: $(wc -l < "$4") cases (seed $2) -> $4"
}

work="$(mktemp -d)"
cleanup() {
  local c
  for c in "${containers[@]}"; do
    docker rm -f "$c" >/dev/null 2>&1 || true
  done
  rm -rf "$work"
}
trap cleanup EXIT
trap 'exit 130' INT TERM

case "${1:-}" in
  record)
    mkdir -p "$recorded"
    expect expr 1 2000 "$recorded/expressions.txt"
    expect props 2 2000 "$recorded/property_refs.txt"
    expect tags 3 2000 "$recorded/tag_paths.txt"
    ;;
  random)
    seed="${2:-$RANDOM$RANDOM}"
    count="${3:-20000}"
    out="$work/expectations"
    mkdir -p "$out"
    expect expr "$seed" "$count" "$out/expressions.txt"
    expect props "$((seed + 1))" "$count" "$out/property_refs.txt"
    expect tags "$((seed + 2))" "$count" "$out/tag_paths.txt"
    IGLINT_EXPRESSION_ORACLE_DIR="$out" cargo test --manifest-path "$root/Cargo.toml" \
      -p iglint_expression --release --test oracle -- --ignored oracle_random_run --nocapture
    ;;
  *)
    sed -n '2,19p' "$0"
    exit 2
    ;;
esac
