#!/bin/sh
# Self-hosted object runtime: compile compiler/runtime.soil's fixture to Wasm
# with the Soil compiler on the bootstrapped runtime, then run each probe.
#
# No Rust in the loop: soil-run.sh runs the compiler, bytes-to-wasm.py assembles
# the module the compiler emitted, wasmtime runs it.
set -eu
cd "$(dirname "$0")/.."
OUT=${TMPDIR:-/tmp}/selfhost-objects
SOURCE=$(cat compiler/runtime.soil tests/selfhost-objects.soil)

./scripts/soil-run.sh core/prelude.soil "$SOURCE" \
  compiler/core.soil compiler/main.soil > "$OUT-bytes.txt"
python3 scripts/bytes-to-wasm.py "$OUT-bytes.txt" "$OUT.wasm"
wasm-tools validate "$OUT.wasm"

check() {
  name=$1
  expected=$2
  actual=$(wasmtime run --invoke "$name" "$OUT.wasm" 2>/dev/null | tail -n 1)
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
check rt/test-cons-kind 0
check rt/test-string-kind 1
check rt/test-symbol-kind 2
check rt/test-symbol-equal 1
check rt/test-symbol-length 4
check rt/test-env-lookup 42
check rt/test-env-shadow 43
check rt/test-env-missing 0
check rt/test-bytes-kind 3
check rt/test-bytes-capacity 4
check rt/test-bytes-push-length 2
check rt/test-bytes-get 66
check rt/test-bytes-set-get 122

printf 'selfhost-objects: ok\n'
