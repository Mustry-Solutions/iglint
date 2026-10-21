#!/usr/bin/env bash
# Differential test on a private Ignition project, which must never end up in
# this repository: its path comes from IGLINT_PRIVATE_CORPUS, its sources are
# copied to a temporary directory under neutral names, and only aggregate
# numbers are printed.
#
# usage: IGLINT_PRIVATE_CORPUS=/path/to/project oracle/private-corpus.sh
set -euo pipefail
corpus="${IGLINT_PRIVATE_CORPUS:?set IGLINT_PRIVATE_CORPUS}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
chmod 755 "$work"

python3 "$here/extract_view_scripts.py" "$corpus" "$work/src"
chmod -R a+rX "$work/src"
"$here/run-oracle.sh" "$work/src" "$work/src/list.txt" "$work/oracle.txt"
cd "$here/.."
IGLINT_ORACLE_ROOT="$work/src" IGLINT_ORACLE_RESULTS="$work/oracle.txt" \
  cargo test -q -p iglint_python --test oracle private_corpus_matches_jython -- --ignored --nocapture
