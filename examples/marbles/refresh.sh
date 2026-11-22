#!/usr/bin/env sh
# Fetch the four Marbles reports and assemble the UI snapshot.
#
# Defaults to the records-backed `mb` command. Run from a project directory (or
# set MARBLES_DIR) so `mb` sees the same project the interactive shell would.
# The scope is written into the snapshot; the UI never infers it from cwd.
set -eu
cd "$(dirname "$0")/../.."

MARBLES_BIN=${MARBLES_BIN:-mb}
MARBLES_DIR=${MARBLES_DIR:-.}
MARBLES_SCOPE=${MARBLES_SCOPE:-project}
DAYS=${MARBLES_DAYS:-30}
PLAN=${MARBLES_PLAN:-}
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

PROJECT=${MARBLES_PROJECT:-}
if [ "$MARBLES_SCOPE" != "store" ] && [ -z "$PROJECT" ]; then
  for marker in .marbles .marbles-local; do
    manifest="$MARBLES_DIR/$marker/project.toml"
    if [ -r "$manifest" ]; then
      PROJECT=$(sed -n 's/^[[:space:]]*slug[[:space:]]*=[[:space:]]*"\(.*\)".*/\1/p' "$manifest" | head -n 1)
      [ -n "$PROJECT" ] && break
    fi
  done
fi
if [ "$MARBLES_SCOPE" = "store" ]; then
  PROJECT=
fi

run() {
  (cd "$MARBLES_DIR" && "$MARBLES_BIN" "$@")
}

run burndown --days "$DAYS" --json > "$TMP/burndown.json"
run portfolio --json > "$TMP/portfolio.json"
run goal list --json > "$TMP/goals.json"
GOAL_ID=$(python3 - "$TMP/goals.json" <<'PY'
import json, sys
goals = json.load(open(sys.argv[1]))
print(goals[0]["id"] if goals else "")
PY
)
if [ -n "$GOAL_ID" ]; then
  run goal show "$GOAL_ID" --json > "$TMP/goal_show.json"
else
  printf '{}\n' > "$TMP/goal_show.json"
fi
if [ -n "$PLAN" ]; then
  run plan gantt "$PLAN" --json > "$TMP/gantt.json"
else
  printf 'null\n' > "$TMP/gantt.json"
fi
run list --json > "$TMP/issues.json"

python3 - "$TMP" "$PROJECT" <<'PY'
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
project = sys.argv[2]
scope = (
    {"kind": "project", "slug": project, "title": project}
    if project
    else {"kind": "store", "title": "all projects"}
)
snapshot = {
    "schema": "marbles.snapshot/1",
    "scope": scope,
    "burndown": json.loads((root / "burndown.json").read_text()),
    "portfolio": json.loads((root / "portfolio.json").read_text()),
    "goals": json.loads((root / "goals.json").read_text()),
    "goal_show": json.loads((root / "goal_show.json").read_text()),
    "gantt": json.loads((root / "gantt.json").read_text()),
    "issues": json.loads((root / "issues.json").read_text()),
}
with open("examples/marbles/snapshot.json", "w") as handle:
    json.dump(snapshot, handle, indent=2)
    handle.write("\n")
PY

echo "refreshed examples/marbles/snapshot.json for ${PROJECT:-store}"
