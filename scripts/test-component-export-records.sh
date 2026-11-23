#!/bin/sh
# Canonical ABI export: `lights: func() -> list<light-state>` from Soil.
#
# The result flattens to (ptr, len), so the guest returns a pointer to a return
# area holding those two values, and the record array behind it uses the
# canonical field layout. The gate builds the component AND runs it, so this
# checks the layout and not just that the encoder accepted it.
set -eu
cd "$(dirname "$0")/.."
SOIL_LANG=${SOIL_LANG:-$HOME/.local/share/soil/bin/soil-lang}
SRC=tests/component-export-records.soil
OUT=/tmp/component-export-records
SOURCE=$(cat "$SRC")

"$SOIL_LANG" --prelude core/prelude.soil --call main \
  compiler/core.soil compiler/component-main.soil -- "$SOURCE" > "$OUT.wat"
"$SOIL_LANG" --prelude core/prelude.soil --call main \
  compiler/core.soil compiler/wit-main.soil -- "$SOURCE" > "$OUT.wit"

grep -F 'interface lights {' "$OUT.wit" >/dev/null
grep -F 'lights: func() -> list<light-state>;' "$OUT.wit" >/dev/null

wasm-tools component embed "$OUT.wit" --world hue "$OUT.wat" -o "$OUT-embed.wasm"
wasm-tools component new "$OUT-embed.wasm" -o "$OUT.wasm"
wasm-tools validate "$OUT.wasm"

# The export exists under its interface-qualified core name, and the interpreter
# is nowhere in the component.
wasm-tools component wit "$OUT.wasm" | grep -F 'export fungos:hue/lights@0.1.0;' >/dev/null
[ "$(strings "$OUT.wasm" | grep -c soil_lang)" -eq 0 ]

RESULT=$(wasmtime run --invoke 'lights()' "$OUT.wasm")
printf '%s\n' "$RESULT"
printf '%s' "$RESULT" | grep -F 'id: "hall"' >/dev/null
printf '%s' "$RESULT" | grep -F 'on: true' >/dev/null
printf '%s' "$RESULT" | grep -F 'brightness: 0.5' >/dev/null
printf '%s' "$RESULT" | grep -F 'x: 0.3' >/dev/null
printf '%s' "$RESULT" | grep -F 'id: "desk"' >/dev/null

# Parameters: a string (ptr, len) and a bool, returning a record.
TARGET=$(wasmtime run --invoke 'set-light("hall", true)' "$OUT.wasm")
printf '%s\n' "$TARGET"
printf '%s' "$TARGET" | grep -F 'id: "hall"' >/dev/null
printf '%s' "$TARGET" | grep -F 'on: true' >/dev/null

printf 'component-export-records: ok (list<record> out, parameters in)\n'
