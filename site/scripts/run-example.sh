#!/usr/bin/env bash
# Replays the "Getting started" tutorial on a fresh copy of
# site/examples/quickstart and prints every command with its output, so the
# transcripts in src/content/docs/getting-started.md can be compared with the
# current binary after a change to the rules.
#
# usage: site/scripts/run-example.sh [IGLINT] [WORK_DIR]
#
# IGLINT defaults to `iglint` on PATH. WORK_DIR (default
# ~/.cache/iglint-docs-example) is replaced; it must lie outside the iglint
# repository, whose test fixtures would otherwise join the workspace.
set -euo pipefail
iglint="${1:-iglint}"
work="${2:-$HOME/.cache/iglint-docs-example}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

rm -rf "$work"
mkdir -p "$work"
cp -R "$here/../examples/quickstart/." "$work/"
cd "$work"
git init -q
git add -A
git -c user.name=docs -c user.email=docs@example.invalid commit -qm "example project"

run() {
  echo "\$ $*"
  "$@" || echo "(exit status $?)"
  echo
}

run "$iglint" check --color never
run "$iglint" check --statistics --color never
run "$iglint" check --diff --color never
run "$iglint" check --fix --color never --output-format concise
run "$iglint" check --unsafe-fixes --diff --color never
run "$iglint" check --select BIND,REF --color never --output-format concise
git checkout -q .
run "$iglint" baseline --color never
run "$iglint" check --color never
printf '\n\ndef area_label(area):\n\treturn "Area " + aera\n' >> projects/demo-hmi/ignition/script-python/plant/pumps/code.py
run "$iglint" check --color never
sed -i.bak -e 's/log\.warn/logger.warn/' -e 's/+ aera/+ area/' projects/demo-hmi/ignition/script-python/plant/pumps/code.py
rm -f projects/demo-hmi/ignition/script-python/plant/pumps/code.py.bak
run "$iglint" check --prune-baseline --color never
