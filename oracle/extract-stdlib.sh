#!/usr/bin/env bash
# Extracts the Python standard library that ships with Ignition's Jython
# (user-lib/pylib) and records Jython's parse of every .py file in it.
#
# usage: oracle/extract-stdlib.sh DEST
#
# Afterwards run the differential test:
#   IGLINT_ORACLE_ROOT=DEST/pylib IGLINT_ORACLE_RESULTS=DEST/oracle.txt \
#     cargo test -p iglint_python --test oracle corpus_matches_jython -- --ignored
set -euo pipefail
image="${IGLINT_ORACLE_IMAGE:-inductiveautomation/ignition:8.3.3}"
dest="$(mkdir -p "$1" && cd "$1" && pwd)"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

container="$(docker create "$image")"
trap 'docker rm -f "$container" >/dev/null' EXIT
rm -rf "$dest/pylib"
docker cp "$container:/usr/local/bin/ignition/user-lib/pylib" "$dest/pylib"
chmod -R a+rX "$dest/pylib"

(cd "$dest/pylib" && find . -name '*.py' -type f | sed 's|^\./||' | LC_ALL=C sort) > "$dest/list.txt"
echo "$(wc -l < "$dest/list.txt") files"
"$here/run-oracle.sh" "$dest/pylib" "$dest/list.txt" "$dest/oracle.txt"
echo "recorded $dest/oracle.txt"
