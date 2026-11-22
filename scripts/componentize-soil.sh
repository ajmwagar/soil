#!/bin/sh
# Soil -> component WAT -> Wasmtime component.
set -eu
cd "$(dirname "$0")/.."
SOIL_LANG=${SOIL_LANG:-/Users/ajmwagar/src/singularity/soil/target/debug/soil-lang}
SOURCE=$(cat tests/component-cli.soil)
"$SOIL_LANG" --prelude core/prelude.soil --call main compiler/core.soil compiler/component-main.soil \
  -- "$SOURCE" > /tmp/soil-component.wat
wasm-tools component embed tests/wit/wasi-stdio --world probe /tmp/soil-component.wat \
  -o /tmp/soil-component-embed.wasm
wasm-tools component new /tmp/soil-component-embed.wasm -o /tmp/soil-component.wasm
wasm-tools validate /tmp/soil-component.wasm
wasmtime run --invoke 'run()' /tmp/soil-component.wasm -- one two three
