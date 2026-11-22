#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
SOIL_LANG=${SOIL_LANG:-/Users/ajmwagar/src/singularity/soil/target/debug/soil-lang}
SOURCE=$(cat tests/component-clock.soil)
"$SOIL_LANG" --prelude core/prelude.soil --call main compiler/core.soil compiler/component-main.soil \
  -- "$SOURCE" > /tmp/soil-clock.wat
wasm-tools component embed tests/wit/wasi-clocks --world probe /tmp/soil-clock.wat \
  -o /tmp/soil-clock-embed.wasm
wasm-tools component new /tmp/soil-clock-embed.wasm -o /tmp/soil-clock.wasm
wasm-tools validate /tmp/soil-clock.wasm
wasmtime run --invoke 'run()' /tmp/soil-clock.wasm
