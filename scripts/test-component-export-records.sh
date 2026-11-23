#!/bin/sh
# Canonical ABI export: records and parameters, out and in.
#
# Runs on the bootstrapped Soil runtime (scripts/soil-run.sh), so this is
# evidence about Soil's own toolchain, not about the native interpreter.
set -eu
cd "$(dirname "$0")/.."
SRC=tests/component-export-records.soil
OUT=/tmp/component-export-records
SOURCE=$(cat "$SRC")

./scripts/soil-run.sh core/prelude.soil "$SOURCE" \
  compiler/core.soil compiler/component-main.soil > "$OUT.wat"
./scripts/soil-run.sh core/prelude.soil "$SOURCE" \
  compiler/core.soil compiler/wit-main.soil > "$OUT.wit"

grep -F 'interface lights {' "$OUT.wit" >/dev/null
grep -F 'lights: func() -> list<light-state>;' "$OUT.wit" >/dev/null
grep -F 'colour: option<colour>,' "$OUT.wit" >/dev/null

wasm-tools component embed "$OUT.wit" --world hue "$OUT.wat" -o "$OUT-embed.wasm"
wasm-tools component new "$OUT-embed.wasm" -o "$OUT.wasm"
wasm-tools validate "$OUT.wasm"

wasm-tools component wit "$OUT.wasm" | grep -F 'export fungos:hue/lights@0.1.0;' >/dev/null
[ "$(strings "$OUT.wasm" | grep -c soil_lang)" -eq 0 ]

RESULT=$(wasmtime run --invoke 'lights()' "$OUT.wasm")
printf '%s\n' "$RESULT"
printf '%s' "$RESULT" | grep -F 'id: "hall"' >/dev/null
printf '%s' "$RESULT" | grep -F 'brightness: 0.5' >/dev/null
printf '%s' "$RESULT" | grep -F 'colour: some({x: 0.3, y: 0.4})' >/dev/null
printf '%s' "$RESULT" | grep -F 'brightness: 0}' >/dev/null

TARGET=$(wasmtime run --invoke 'set-light("hall", true)' "$OUT.wasm")
printf '%s\n' "$TARGET"
printf '%s' "$TARGET" | grep -F 'id: "hall"' >/dev/null
printf '%s' "$TARGET" | grep -F 'on: true' >/dev/null

printf 'component-export-records: ok (list<record> out, parameters in, option field)\n'
