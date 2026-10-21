#!/usr/bin/env bash
# Records Jython's output for the hand-written oracle cases of
# crates/iglint_python/tests/oracle/*.cases into the matching *.expected
# files. Run after adding or changing cases; review the diff.
#
# usage: oracle/record-expected.sh [CASES_FILE...]
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
dir="$here/../crates/iglint_python/tests/oracle"
if [[ $# -eq 0 ]]; then
  set -- "$dir"/*.cases
fi
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
chmod 755 "$work"
for cases in "$@"; do
  name="$(basename "$cases" .cases)"
  cp "$cases" "$work/$name.cases"
  echo "$name.cases" > "$work/list.txt"
  "$here/run-oracle.sh" "$work" "$work/list.txt" "$dir/$name.expected"
  echo "recorded $dir/$name.expected"
done
