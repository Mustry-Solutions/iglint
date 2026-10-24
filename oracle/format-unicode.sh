#!/usr/bin/env bash
# Records Jython's verdict on string formatting that mixes byte strings and
# unicode (oracle/format_unicode_cases.py), for SCR112's Unicode checks
# (crates/iglint_linter/src/rules/script/format_string.rs, test
# `unicode_formatting_matches_jython`).
#
# usage: oracle/format-unicode.sh all OUT
#        oracle/format-unicode.sh sample SEED COUNT OUT
#
# The committed replay file
# crates/iglint_linter/tests/fixtures/format-spec/unicode.tsv is
# `python3 oracle/format_unicode_cases.py pick ALL_RESULTS 14 5000` of the
# results of `all` (16,947 cases, a few seconds).
# Needs Docker and the inductiveautomation/ignition image (override with
# IGLINT_ORACLE_IMAGE).
set -euo pipefail
image="${IGLINT_ORACLE_IMAGE:-inductiveautomation/ignition:8.3.3}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
out="${@: -1}"
work="$(mktemp -d)"
container="iglint-oracle-$$-format-unicode"
cleanup() {
  docker rm -f "$container" >/dev/null 2>&1 || true
  rm -rf "$work"
}
trap cleanup EXIT
trap 'exit 130' INT TERM
chmod 777 "$work"
python3 "$here/format_unicode_cases.py" "${@:1:$#-1}" > "$work/cases.txt"
docker run --rm --name "$container" \
  -v "$work":/out \
  -v "$here":/oracle:ro \
  --entrypoint sh "$image" -c '
    CP=$(ls /usr/local/bin/ignition/lib/core/common/*.jar | tr "\n" ":")
    exec /usr/local/bin/ignition/lib/runtime/jre-nix/bin/java -cp "$CP" \
      -Dpython.path=/usr/local/bin/ignition/user-lib/pylib:/oracle \
      -Dpython.import.site=false -Dpython.cachedir.skip=true \
      -Dpython.console.encoding=UTF-8 \
      org.python.util.jython /oracle/jython_expression_oracle.py \
      /out/cases.txt /out/result.txt'
cp "$work/result.txt" "$out"
