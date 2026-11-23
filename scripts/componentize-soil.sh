#!/bin/sh
# Soil -> component WAT -> Wasmtime component.
set -eu
cd "$(dirname "$0")/.."
SOURCE=$(cat tests/component-cli.soil)
./scripts/soil-run.sh core/prelude.soil "$SOURCE" compiler/core.soil compiler/component-main.soil > /tmp/soil-component.wat
wasm-tools component embed tests/wit/wasi-stdio --world probe /tmp/soil-component.wat \
  -o /tmp/soil-component-embed.wasm
wasm-tools component new /tmp/soil-component-embed.wasm -o /tmp/soil-component.wasm
wasm-tools validate /tmp/soil-component.wasm
wasmtime run --invoke 'run()' /tmp/soil-component.wasm -- one two three
