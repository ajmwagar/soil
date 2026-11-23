#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
SOURCE=$(cat tests/component-clock.soil)
./scripts/soil-run.sh core/prelude.soil "$SOURCE" compiler/core.soil compiler/component-main.soil > /tmp/soil-clock.wat
wasm-tools component embed tests/wit/wasi-clocks --world probe /tmp/soil-clock.wat \
  -o /tmp/soil-clock-embed.wasm
wasm-tools component new /tmp/soil-clock-embed.wasm -o /tmp/soil-clock.wasm
wasm-tools validate /tmp/soil-clock.wasm
wasmtime run --invoke 'run()' /tmp/soil-clock.wasm
