#!/usr/bin/env bash
# Differential fuzzing against Jython: generates cases with the fuzz_gen
# example (mutated snippets of SOURCE_DIR plus random programs), records
# Jython's output and compares it with iglint's.
#
# usage: oracle/fuzz.sh SEED COUNT SOURCE_DIR [WORK_DIR]
#
# SOURCE_DIR is typically the standard library extracted by
# oracle/extract-stdlib.sh (DEST/pylib).
set -euo pipefail
seed="$1"
count="$2"
sources="$(cd "$3" && pwd)"
work="${4:-$(mktemp -d)}"
mkdir -p "$work"
chmod 755 "$work"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$here/.."

cargo run -q --release -p iglint_python --example fuzz_gen -- \
  "$seed" "$count" "$sources" "$work/fuzz.cases"
chmod 644 "$work/fuzz.cases"
echo fuzz.cases > "$work/list.txt"
"$here/run-oracle.sh" "$work" "$work/list.txt" "$work/fuzz.expected"
IGLINT_FUZZ_CASES="$work/fuzz.cases" IGLINT_FUZZ_EXPECTED="$work/fuzz.expected" \
  cargo test -q -p iglint_python --test oracle fuzz_cases_match_jython -- --ignored --nocapture
