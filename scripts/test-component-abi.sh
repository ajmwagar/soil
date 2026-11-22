#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
SOIL_LANG=${SOIL_LANG:-/Users/ajmwagar/src/singularity/soil/target/debug/soil-lang}
run_probe() {
  WIT=$1; WORLD=$2; SRC=$3; OUT=$4
  SOURCE=$(cat "$SRC")
  "$SOIL_LANG" --prelude core/prelude.soil --call main compiler/core.soil compiler/component-main.soil \
    -- "$SOURCE" > "/tmp/$OUT.wat"
  wasm-tools component embed "$WIT" --world "$WORLD" "/tmp/$OUT.wat" -o "/tmp/$OUT-embed.wasm"
  wasm-tools component new "/tmp/$OUT-embed.wasm" -o "/tmp/$OUT.wasm"
  wasm-tools validate "/tmp/$OUT.wasm"
  printf '%s: ' "$OUT"
  wasmtime run --invoke 'run()' "/tmp/$OUT.wasm"
}
run_probe tests/wit/option-probe.wit probe tests/component-option.soil option
run_probe tests/wit/result-probe.wit probe tests/component-result.soil result
run_probe tests/wit/variant-probe.wit probe tests/component-variant.soil variant
