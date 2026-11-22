#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
SOIL_LANG=${SOIL_LANG:-/Users/ajmwagar/src/singularity/soil/target/debug/soil-lang}
SOURCE=$(cat tests/component-resource.soil)
"$SOIL_LANG" --prelude core/prelude.soil --call main compiler/core.soil compiler/component-main.soil \
  -- "$SOURCE" > /tmp/soil-resource.wat
wasm-tools component embed tests/wit/wasi-clocks --world resources /tmp/soil-resource.wat \
  -o /tmp/soil-resource-embed.wasm
wasm-tools component new /tmp/soil-resource-embed.wasm -o /tmp/soil-resource.wasm
wasm-tools validate /tmp/soil-resource.wasm
wasmtime run --invoke 'run()' /tmp/soil-resource.wasm
