#!/usr/bin/env bash
# Parses Python sources with IA's Jython 2.7 and records, per source, the
# ast.dump output or the SyntaxError (default), or the token stream that the
# parser sees (--tokens), or whether full compilation succeeds (--compile:
# compile-time errors such as 'return' outside function), or the
# SyntaxWarnings full compilation issues (--warnings), or the names that
# running each source creates implicitly in Ignition's script contexts
# (--implicit, see jython_implicit_names_oracle.py), or the names running
# each source looks up in its module's globals, the interpreter's implicit
# reads included (--reads, see jython_implicit_reads_oracle.py).
#
# usage: oracle/run-oracle.sh [--tokens | --compile | --warnings | --implicit | --reads] ROOT LIST OUT
#
#   ROOT  directory that contains the inputs
#   LIST  file with one path per line, relative to ROOT; ".cases" files hold
#         many small sources (see jython_ast_oracle.py)
#   OUT   output file
#
# Needs Docker and the inductiveautomation/ignition image (override with
# IGLINT_ORACLE_IMAGE). Everything runs in a single JVM.
set -euo pipefail
script=jython_ast_oracle.py
if [[ "${1:-}" == "--tokens" ]]; then
  script=jython_token_oracle.py
  shift
elif [[ "${1:-}" == "--compile" ]]; then
  script=jython_compile_oracle.py
  shift
elif [[ "${1:-}" == "--warnings" ]]; then
  script=jython_warning_oracle.py
  shift
elif [[ "${1:-}" == "--implicit" ]]; then
  script=jython_implicit_names_oracle.py
  shift
elif [[ "${1:-}" == "--reads" ]]; then
  script=jython_implicit_reads_oracle.py
  shift
fi
image="${IGLINT_ORACLE_IMAGE:-inductiveautomation/ignition:8.3.3}"
root="$(cd "$1" && pwd)"
list="$(cd "$(dirname "$2")" && pwd)/$(basename "$2")"
out="$3"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
work="$(mktemp -d)"
# A named container, removed when the script ends or is interrupted (a
# killed `docker run` client leaves the container running).
container="iglint-oracle-$$-${script%.py}"
cleanup() {
  docker rm -f "$container" >/dev/null 2>&1 || true
  rm -rf "$work"
}
trap cleanup EXIT
trap 'exit 130' INT TERM
cp "$list" "$work/list.txt"
chmod 777 "$work"

docker run --rm --name "$container" \
  -v "$root":/w:ro \
  -v "$work":/out \
  -v "$here":/oracle:ro \
  --entrypoint sh "$image" -c '
    CP=$(ls /usr/local/bin/ignition/lib/core/common/*.jar | tr "\n" ":")
    cd /w
    exec /usr/local/bin/ignition/lib/runtime/jre-nix/bin/java -Xss16m -cp "$CP" \
      -Dpython.path=/usr/local/bin/ignition/user-lib/pylib:/oracle \
      -Dpython.import.site=false -Dpython.cachedir.skip=true \
      -Dpython.console.encoding=UTF-8 \
      org.python.util.jython "/oracle/$0" /out/list.txt /out/result.txt' "$script"
cp "$work/result.txt" "$out"
