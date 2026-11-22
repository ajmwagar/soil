#!/usr/bin/env sh
# Build the Marbles UI from the snapshot. Cargo is frozen; use an existing binary.
set -eu
cd "$(dirname "$0")/../.."

BIN=${SOIL_LANG:-}
if [ -z "$BIN" ]; then
  if [ -x .bin/soil-lang ]; then
    BIN=.bin/soil-lang
  else
    BIN=target/debug/soil-lang
  fi
fi
if [ ! -x "$BIN" ]; then
  echo "no soil-lang binary at $BIN; cargo is frozen" >&2
  exit 1
fi

mkdir -p examples/marbles/dist
STATE=examples/marbles/.state.soil
python3 - <<'PY'
import json
with open("examples/marbles/snapshot.json") as handle:
    issues = json.load(handle)
with open("examples/marbles/.state.soil", "w") as handle:
    handle.write("(defun marbles-snapshot () (json-decode ")
    handle.write(json.dumps(json.dumps(issues)))
    handle.write("))\n")
PY

"$BIN" \
  crates/soil-lang/stdlib/prelude.soil \
  crates/soil-lang/stdlib/ui.soil \
  crates/soil-lang/stdlib/highlight.soil \
  examples/ui/prelude.soil \
  "$STATE" \
  examples/marbles/ui.soil \
  examples/marbles/page.soil \
  > examples/marbles/dist/index.html

echo "wrote examples/marbles/dist/index.html"
