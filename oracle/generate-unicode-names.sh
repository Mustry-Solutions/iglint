#!/usr/bin/env bash
# Regenerates the `\N{...}` name table of iglint_python from the ucnhash
# table of IA's Jython (crates/iglint_python/src/unicode_names/table.bin and
# table.rs). See jython_unicode_names.py.
#
# usage: oracle/generate-unicode-names.sh [NAMES_TSV]
#
#   NAMES_TSV  optional file that gets every accepted name with its code point
#
# Needs Docker and the inductiveautomation/ignition image (override with
# IGLINT_ORACLE_IMAGE).
set -euo pipefail
image="${IGLINT_ORACLE_IMAGE:-inductiveautomation/ignition:8.3.3}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
dest="$here/../crates/iglint_python/src/unicode_names"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
chmod 777 "$work"

docker run --rm \
  -v "$work":/out \
  -v "$here":/oracle:ro \
  --entrypoint sh "$image" -c '
    CP=$(ls /usr/local/bin/ignition/lib/core/common/*.jar | tr "\n" ":")
    exec /usr/local/bin/ignition/lib/runtime/jre-nix/bin/java -cp "$CP" \
      -Dpython.import.site=false -Dpython.cachedir.skip=true \
      org.python.util.jython /oracle/jython_unicode_names.py /out'
cp "$work/table.bin" "$work/table.rs" "$dest/"
if [[ $# -gt 0 ]]; then
  cp "$work/names.tsv" "$1"
fi
echo "wrote $dest/table.bin and $dest/table.rs"
