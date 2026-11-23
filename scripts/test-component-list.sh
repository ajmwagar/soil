#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
SOURCE=$(cat tests/component-list.soil)
./scripts/soil-run.sh core/prelude.soil "$SOURCE" compiler/core.soil compiler/component-main.soil > /tmp/soil-list.wat
wasm-tools component embed tests/wit/list-probe.wit --world probe /tmp/soil-list.wat \
  -o /tmp/soil-list-embed.wasm
wasm-tools component new /tmp/soil-list-embed.wasm -o /tmp/soil-list.wasm
wasm-tools validate /tmp/soil-list.wasm
wasmtime run --invoke 'run()' /tmp/soil-list.wasm
