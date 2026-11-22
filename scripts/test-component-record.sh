#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
SOIL_LANG=${SOIL_LANG:-/Users/ajmwagar/src/singularity/soil/target/debug/soil-lang}
SOURCE=$(cat tests/component-record.soil)
"$SOIL_LANG" --prelude core/prelude.soil --call main compiler/core.soil compiler/component-main.soil \
  -- "$SOURCE" > /tmp/soil-record.wat
wasm-tools component embed tests/wit/record-probe.wit --world probe /tmp/soil-record.wat \
  -o /tmp/soil-record-embed.wasm
wasm-tools component new /tmp/soil-record-embed.wasm -o /tmp/soil-record.wasm
wasm-tools validate /tmp/soil-record.wasm
wasmtime run --invoke 'run()' /tmp/soil-record.wasm
