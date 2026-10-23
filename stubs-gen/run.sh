#!/usr/bin/env bash
# Regenerates the Ignition facts in crates/iglint_stubs from scratch:
#
#   1. starts a throwaway gateway container (iglint-stubs-gen) from the
#      inductiveautomation/ignition:8.3.3 image with the project in
#      stubs-gen/project, whose WebDev endpoint runs posted Jython;
#   2. dumps the gateway-scope system.* API, the expression functions, the
#      Perspective component registry and Jython's builtins (introspect/*.py);
#   3. finds the Vision Client and Designer script module registrations in the
#      client/designer bytecode (registrations.py) and replays them inside the
#      gateway without running any client code (introspect/scopes.py);
#   4. removes the container, normalizes the dumps into
#      crates/iglint_stubs/data/*.json (normalize.py), generates
#      crates/iglint_stubs/src/generated.rs (codegen.py) and compares the result
#      with ignition-devs/ignition-api (crosscheck.py -> CROSSCHECK.md).
#
#   stubs-gen/run.sh            full run
#   stubs-gen/run.sh --keep     leave the container running afterwards
#   stubs-gen/run.sh --offline  skip the gateway, reuse stubs-gen/out
#   stubs-gen/run.sh --vetted   only regenerate the vetted-code tables
#                               (crates/iglint_python_semantic/src/generated/
#                               vetted_generated.rs), from gen_vetted.py and
#                               crates/iglint_stubs/data/system.json; needs no
#                               gateway, just the image's Jython and jars
#
# Environment: IGLINT_STUBS_IMAGE (default inductiveautomation/ignition:8.3.3),
# IGLINT_STUBS_PORT (default 18089), IGLINT_IGNITION_API (an existing
# ignition-api checkout; default: shallow clone of branch 8.3 into a temp dir).
# Requires docker, curl, git and python3. Only the container named
# iglint-stubs-gen is created or removed.
set -euo pipefail
[[ -n "${IGLINT_STUBS_TRACE:-}" ]] && set -x

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/.." && pwd)"
image="${IGLINT_STUBS_IMAGE:-inductiveautomation/ignition:8.3.3}"
name="iglint-stubs-gen"
port="${IGLINT_STUBS_PORT:-18089}"
url="http://127.0.0.1:$port/system/webdev/iglint-stubs/exec"
crate="$root/crates/iglint_stubs"
raw="$here/out"
keep=0
offline=0
vetted=0
for arg in "$@"; do
  case "$arg" in
    --keep) keep=1 ;;
    --offline) offline=1 ;;
    --vetted) vetted=1 ;;
    *) echo "unknown argument: $arg" >&2; exit 2 ;;
  esac
done

# The vetted-code tables need only the image's Jython, jars and reflection,
# not a running gateway: run gen_vetted.py in a throwaway --rm container (it
# unpacks the module jars inside the container).
if [[ $vetted -eq 1 ]]; then
  vwork="$(mktemp -d)"
  chmod 777 "$vwork"
  cp "$here/gen_vetted.py" "$vwork/"
  cp "$root/crates/iglint_python_semantic/src/vetted.rs" "$vwork/vetted.rs"
  cp "$crate/data/system.json" "$vwork/system.json"
  vcontainer="iglint-vetted-gen-$$"
  trap 'docker rm -f "$vcontainer" >/dev/null 2>&1 || true; rm -rf "$vwork"' EXIT
  docker run --rm --name "$vcontainer" -v "$vwork":/g --entrypoint sh "$image" -c '
    CP=$(ls /usr/local/bin/ignition/lib/core/common/*.jar | tr "\n" ":")
    exec /usr/local/bin/ignition/lib/runtime/jre-nix/bin/java -Xmx2g -Xss16m -cp "$CP" \
      -Dpython.path=/usr/local/bin/ignition/user-lib/pylib:/g \
      -Dpython.import.site=false -Dpython.cachedir.skip=true \
      -Dpython.console.encoding=UTF-8 \
      org.python.util.jython /g/gen_vetted.py /g/vetted.rs /g/system.json /g/vetted_generated.rs'
  cp "$vwork/vetted_generated.rs" \
    "$root/crates/iglint_python_semantic/src/generated/vetted_generated.rs"
  echo "wrote crates/iglint_python_semantic/src/generated/vetted_generated.rs" >&2
  exit 0
fi

work="$(mktemp -d)"
cleanup() {
  rm -rf "$work"
  if [[ $offline -eq 0 && $keep -eq 0 ]]; then
    docker rm -f "$name" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

# Posts a Jython file to the gateway and stores the JSON it returns.
dump() { # dump <script> <output>
  curl -fsS --max-time 900 -X POST -H 'Content-Type: text/plain' \
    --data-binary @"$1" "$url" -o "$2"
  if python3 -I -c 'import json,sys; d=json.load(open(sys.argv[1])); sys.exit(isinstance(d, dict) and "error" in d and len(d) == 1)' "$2"; then
    :
  else
    echo "gateway error while running $1:" >&2
    python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["error"])' "$2" >&2
    exit 1
  fi
}

if [[ $offline -eq 0 ]]; then
  docker rm -f "$name" >/dev/null 2>&1 || true
  docker create --name "$name" -p "127.0.0.1:$port:8088" \
    -e ACCEPT_IGNITION_EULA=Y \
    -e GATEWAY_ADMIN_USERNAME=admin \
    -e GATEWAY_ADMIN_PASSWORD=password \
    -e IGNITION_EDITION=standard \
    "$image" -n "$name" >/dev/null
  # The project goes into data/projects, owned by the gateway user (2003).
  tar --owner=2003 --group=2003 --transform 's,^\.,projects,' \
    -C "$here/project" -cf "$work/project.tar" .
  docker cp - "$name:/usr/local/bin/ignition/data/" < "$work/project.tar"
  # Client, designer and module jars for registrations.py.
  mkdir -p "$work/core" "$work/modules"
  for sub in common client designer; do
    docker cp "$name:/usr/local/bin/ignition/lib/core/$sub" "$work/core/$sub"
  done
  docker cp "$name:/usr/local/bin/ignition/user-lib/modules/." "$work/modules/"
  docker start "$name" >/dev/null

  echo "waiting for the gateway on port $port ..." >&2
  ready=0
  for _ in $(seq 1 150); do
    if curl -fsS -o /dev/null -X POST -H 'Content-Type: text/plain' \
      --data-binary 'result = 1' "$url" 2>/dev/null; then
      ready=1
      break
    fi
    sleep 2
  done
  if [[ $ready -eq 0 ]]; then
    echo "gateway did not come up; last log lines:" >&2
    docker logs --tail 40 "$name" >&2 || true
    exit 1
  fi

  rm -rf "$raw"
  mkdir -p "$raw"
  for part in scripting expression perspective builtins; do
    echo "dumping $part ..." >&2
    dump "$here/introspect/$part.py" "$raw/$part.json"
  done
  echo "dumping client and designer scopes ..." >&2
  python3 -I "$here/registrations.py" "$work/core" "$work/modules" "$raw/registrations.json"
  python3 -I "$here/payload.py" "$raw/registrations.json" "$here/introspect" "$work/scopes.py"
  dump "$work/scopes.py" "$raw/scopes.json"
fi

echo "normalizing ..." >&2
python3 -I "$here/normalize.py" "$raw" "$crate/data"
python3 -I "$here/codegen.py" "$crate/data" "$crate/src/generated.rs"

api="${IGLINT_IGNITION_API:-}"
if [[ -z "$api" ]]; then
  api="$work/ignition-api"
  git clone --quiet --depth 1 --branch 8.3 https://github.com/ignition-devs/ignition-api "$api"
fi
python3 -I "$here/crosscheck.py" "$api" "$crate/data/system.json" "$here/CROSSCHECK.md"
echo "done" >&2
