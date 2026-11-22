#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
SOIL_LANG=${SOIL_LANG:-/Users/ajmwagar/src/singularity/soil/target/debug/soil-lang}
SOURCE=$(cat tests/component-list.soil)
"$SOIL_LANG" --prelude core/prelude.soil --call main compiler/core.soil compiler/component-main.soil \
  -- "$SOURCE" > /tmp/soil-list.wat
wasm-tools component embed tests/wit/list-probe.wit --world probe /tmp/soil-list.wat \
  -o /tmp/soil-list-embed.wasm
wasm-tools component new /tmp/soil-list-embed.wasm -o /tmp/soil-list.wasm
wasm-tools validate /tmp/soil-list.wasm
wasmtime run --invoke 'run()' /tmp/soil-list.wasm
