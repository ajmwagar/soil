#!/usr/bin/env sh
# Fetch the four Marbles reports and assemble the UI snapshot.
set -eu
cd "$(dirname "$0")/../.."

MARBLES_BIN=${MARBLES_BIN:-marbles}
MARBLES_DIR=${MARBLES_DIR:-.}
MARBLES_PROJECT=${MARBLES_PROJECT:-marbles}
DAYS=${MARBLES_DAYS:-30}
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

"$MARBLES_BIN" -C "$MARBLES_DIR" --json burndown --project "$MARBLES_PROJECT" --days "$DAYS" > "$TMP/burndown.json"
"$MARBLES_BIN" -C "$MARBLES_DIR" --json portfolio > "$TMP/portfolio.json"
"$MARBLES_BIN" -C "$MARBLES_DIR" --json goal list > "$TMP/goals.json"
"$MARBLES_BIN" -C "$MARBLES_DIR" --json list > "$TMP/issues.json"

python3 - "$TMP" <<'PY'
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
snapshot = {
    "burndown": json.loads((root / "burndown.json").read_text()),
    "portfolio": json.loads((root / "portfolio.json").read_text()),
    "goals": json.loads((root / "goals.json").read_text()),
    "issues": json.loads((root / "issues.json").read_text()),
}
with open("examples/marbles/snapshot.json", "w") as handle:
    json.dump(snapshot, handle, indent=2)
    handle.write("\n")
PY

echo "refreshed examples/marbles/snapshot.json"
