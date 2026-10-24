#!/usr/bin/env bash
# Records Jython's verdict on str.format cases with literal arguments, for
# SCR112's format spec check (crates/iglint_linter/src/rules/script/
# format_string.rs, test `format_specs_match_jython`).
#
# usage: oracle/format-spec.sh all OUT          every generated case
#        oracle/format-spec.sh sample SEED COUNT OUT
#
# The committed replay file
# crates/iglint_linter/tests/fixtures/format-spec/sample.tsv is
# `python3 oracle/format_spec_cases.py stratify ALL_RESULTS 10 4000` of the
# results of `all` (3,110,400 cases, about ten minutes).
# Needs Docker and the inductiveautomation/ignition image (override with
# IGLINT_ORACLE_IMAGE).
set -euo pipefail
image="${IGLINT_ORACLE_IMAGE:-inductiveautomation/ignition:8.3.3}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
out="${@: -1}"
work="$(mktemp -d)"
container="iglint-oracle-$$-format-spec"
cleanup() {
  docker rm -f "$container" >/dev/null 2>&1 || true
  rm -rf "$work"
}
trap cleanup EXIT
trap 'exit 130' INT TERM
chmod 777 "$work"
python3 "$here/format_spec_cases.py" "${@:1:$#-1}" > "$work/cases.txt"
docker run --rm --name "$container" \
  -v "$work":/out \
  -v "$here":/oracle:ro \
  --entrypoint sh "$image" -c '
    CP=$(ls /usr/local/bin/ignition/lib/core/common/*.jar | tr "\n" ":")
    exec /usr/local/bin/ignition/lib/runtime/jre-nix/bin/java -cp "$CP" \
      -Dpython.path=/usr/local/bin/ignition/user-lib/pylib:/oracle \
      -Dpython.import.site=false -Dpython.cachedir.skip=true \
      org.python.util.jython /oracle/jython_format_spec_oracle.py \
      /out/cases.txt /out/result.txt'
cp "$work/result.txt" "$out"
