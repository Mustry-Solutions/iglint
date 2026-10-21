#!/usr/bin/env bash
# Fails when tracked files contain markers from private corpora.
# Markers live outside the repository, one case-insensitive fixed string per
# line, in $IGLINT_LEAK_MARKERS (default ~/.config/iglint/leak-markers.txt).
set -euo pipefail
markers="${IGLINT_LEAK_MARKERS:-$HOME/.config/iglint/leak-markers.txt}"
if [[ ! -s "$markers" ]]; then
  echo "check-no-leak: no marker file at $markers, skipping" >&2
  exit 0
fi
cd "$(git rev-parse --show-toplevel)"
if git ls-files -z --cached --others --exclude-standard | xargs -0 grep -I -n -i -F -f "$markers" -- 2>/dev/null; then
  echo "check-no-leak: private corpus markers found (see above)" >&2
  exit 1
fi
echo "check-no-leak: clean"
