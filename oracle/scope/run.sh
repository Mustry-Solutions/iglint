#!/usr/bin/env bash
# Differential test of iglint_python_semantic's name resolution against
# Jython's own compiler (jython_scope_oracle.py, run inside the Ignition
# image): which frame accessor every name read, store and delete compiles
# to, with private-name mangling, and every attribute name. Inputs are the
# hand-written hand.cases and programs generate.py makes from a seed; no
# project data is used.
#
#   oracle/scope/run.sh record [SEED] [COUNT] [PER_FEATURE]
#       Records crates/iglint_python_semantic/tests/oracle/scope.cases and
#       scope.expected: hand.cases plus a stratified sample (up to
#       PER_FEATURE cases per feature tag, default 60) of COUNT generated
#       programs (default seed 1, 20000). The test
#       `cargo test -p iglint_python_semantic --test scope_oracle` replays
#       them without Docker.
#   oracle/scope/run.sh random [SEED] [COUNT]
#       Fresh run of COUNT generated programs (default 20000) plus
#       hand.cases, then the ignored test
#       `name_access_matches_jython_random_run` on all of them.
#
# Environment: IGLINT_ORACLE_IMAGE (default inductiveautomation/ignition:8.3.3).
# Requires docker, python3 and cargo.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
image="${IGLINT_ORACLE_IMAGE:-inductiveautomation/ignition:8.3.3}"
recorded="$root/crates/iglint_python_semantic/tests/oracle"
work="$(mktemp -d)"
container="iglint-oracle-$$-scope"
cleanup() {
  docker rm -f "$container" >/dev/null 2>&1 || true
  rm -rf "$work"
}
trap cleanup EXIT
trap 'exit 130' INT TERM
chmod 777 "$work"

# oracle CASES OUT (file names in $work)
oracle() {
  docker run --rm --name "$container" \
    -v "$here/..:/oracle:ro" -v "$work:/work" \
    --entrypoint sh "$image" -c '
      CP=$(ls /usr/local/bin/ignition/lib/core/common/*.jar | tr "\n" ":")
      exec /usr/local/bin/ignition/lib/runtime/jre-nix/bin/java -Xss16m -cp "$CP" \
        -Dpython.path=/usr/local/bin/ignition/user-lib/pylib:/oracle \
        -Dpython.import.site=false -Dpython.cachedir.skip=true \
        -Dpython.console.encoding=UTF-8 \
        org.python.util.jython /oracle/scope/jython_scope_oracle.py "/work/$0" "/work/$1"' \
    "$1" "$2" 2>&1 | { grep -v -e ' DEBUG ' -e 'SyntaxWarning' || true; }
}

case "${1:-}" in
  record)
    seed="${2:-1}"
    count="${3:-20000}"
    per="${4:-60}"
    python3 "$here/generate.py" "$seed" "$count" "$work/generated.cases"
    oracle generated.cases generated.expected
    python3 "$here/generate.py" stratify "$work/generated.cases" "$work/generated.expected" \
      "$per" "$work/sample.cases" "$work/sample.expected"
    cp "$here/hand.cases" "$work/hand.cases"
    oracle hand.cases hand.expected
    cat "$work/hand.cases" "$work/sample.cases" > "$recorded/scope.cases"
    cat "$work/hand.expected" "$work/sample.expected" > "$recorded/scope.expected"
    echo "recorded $(grep -c '^### ' "$recorded/scope.cases") cases in $recorded/scope.cases"
    ;;
  random)
    seed="${2:-$RANDOM$RANDOM}"
    count="${3:-20000}"
    python3 "$here/generate.py" "$seed" "$count" "$work/all.cases"
    cat "$here/hand.cases" >> "$work/all.cases"
    oracle all.cases all.expected
    echo "seed $seed: $(grep -c '^### ' "$work/all.cases") cases"
    IGLINT_SCOPE_CASES="$work/all.cases" IGLINT_SCOPE_EXPECTED="$work/all.expected" \
      cargo test --manifest-path "$root/Cargo.toml" -q -p iglint_python_semantic \
      --test scope_oracle -- --ignored name_access_matches_jython_random_run --nocapture
    ;;
  *)
    sed -n '2,23p' "$0"
    exit 2
    ;;
esac
