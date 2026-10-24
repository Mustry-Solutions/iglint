#!/usr/bin/env bash
# Regenerates crates/iglint_project/src/generated/perspective_fields.rs: the
# Perspective binding and transform fields that Ignition parses as
# expressions or property paths, and the component script members that
# return the component's name. Runs stubs-gen/gen_perspective_fields.py in
# a throwaway --rm container of the Ignition image (Jython and the image's
# jars only; no gateway is started).
#
# Environment: IGLINT_STUBS_IMAGE (default inductiveautomation/ignition:8.3.3).
# Requires docker. Only the container it starts is created or removed.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/.." && pwd)"
image="${IGLINT_STUBS_IMAGE:-inductiveautomation/ignition:8.3.3}"
out="$root/crates/iglint_project/src/generated/perspective_fields.rs"

work="$(mktemp -d)"
chmod 777 "$work"
cp "$here/gen_perspective_fields.py" "$work/"
container="iglint-perspective-fields-$$"
trap 'docker rm -f "$container" >/dev/null 2>&1 || true; rm -rf "$work"' EXIT
docker run --rm --name "$container" -v "$work":/g --entrypoint sh "$image" -c '
  CP=$(ls /usr/local/bin/ignition/lib/core/common/*.jar | tr "\n" ":")
  exec /usr/local/bin/ignition/lib/runtime/jre-nix/bin/java -Xmx2g -Xss16m -cp "$CP" \
    -Dpython.path=/usr/local/bin/ignition/user-lib/pylib \
    -Dpython.import.site=false -Dpython.cachedir.skip=true \
    -Dpython.console.encoding=UTF-8 \
    org.python.util.jython /g/gen_perspective_fields.py /g/perspective_fields.rs \
    /g/gen_perspective_fields.py'
mkdir -p "$(dirname "$out")"
cp "$work/perspective_fields.rs" "$out"
echo "wrote ${out#"$root/"}" >&2
