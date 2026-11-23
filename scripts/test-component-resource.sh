#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
SOURCE=$(cat tests/component-resource.soil)
./scripts/soil-run.sh core/prelude.soil "$SOURCE" compiler/core.soil compiler/component-main.soil > /tmp/soil-resource.wat
wasm-tools component embed tests/wit/wasi-clocks --world resources /tmp/soil-resource.wat \
  -o /tmp/soil-resource-embed.wasm
wasm-tools component new /tmp/soil-resource-embed.wasm -o /tmp/soil-resource.wasm
wasm-tools validate /tmp/soil-resource.wasm
wasmtime run --invoke 'run()' /tmp/soil-resource.wasm
