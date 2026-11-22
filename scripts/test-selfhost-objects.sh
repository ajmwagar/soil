#!/bin/sh
# Selfhost compile/run smoke for multi-form bodies and objects.
set -eu
cd "$(dirname "$0")/.."
SOIL_C=${SOIL_C:-"$HOME/.local/share/soil/bin/soil-c"}
OUT=${TMPDIR:-/tmp}/selfhost-objects.wasm

"$SOIL_C" --selfhost --soil-root "$PWD" \
  tests/selfhost-objects.soil "$OUT"
wasm-tools validate "$OUT"

check() {
  name=$1
  expected=$2
  actual=$(wasmtime run --invoke "$name" "$OUT" 2>/dev/null | tail -n 1)
  if [ "$actual" != "$expected" ]; then
    echo "FAIL $name: expected $expected, got $actual" >&2
    exit 1
  fi
  echo "ok $name = $actual"
}

check rt/test-multi 41
check rt/test-list 10
check rt/test-record 9
check rt/test-string 66
check rt/test-string-eq 1
