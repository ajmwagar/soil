#!/bin/sh
# Core must compile to an import-free core Wasm module.
set -eu
cd "$(dirname "$0")/.."

# Static policy: core sources may not name host capabilities or import forms.
if rg -n '^[[:space:]]*\(import\b|\b(wasi|soil):' core >/tmp/core-imports.txt; then
  # Comments may discuss capabilities; only code references are policy failures.
  if rg -v ':[0-9]+:[[:space:]]*;' /tmp/core-imports.txt | rg -q .; then
    rg -v ':[0-9]+:[[:space:]]*;' /tmp/core-imports.txt >&2
    echo "core: host capability reference found" >&2
    exit 1
  fi
fi

# Real compiler guard: compile a core probe and assert no import section.
SOIL_LANG=${SOIL_LANG:-/Users/ajmwagar/src/singularity/soil/target/debug/soil-lang}
SOURCE="$(cat tests/component-probe.soil)"
"$SOIL_LANG" --prelude core/prelude.soil --call main compiler/core.soil compiler/main.soil \
  -- "$SOURCE" > /tmp/core-probe-bytes.txt
python3 scripts/bytes-to-wasm.py /tmp/core-probe-bytes.txt /tmp/core-probe.wasm >/dev/null
if wasm-tools print /tmp/core-probe.wasm | grep -q '^  (import'; then
  echo "core: compiled module contains imports" >&2
  exit 1
fi

echo "core: import-free"
