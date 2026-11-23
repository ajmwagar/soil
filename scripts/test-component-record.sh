#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
SOURCE=$(cat tests/component-record.soil)
./scripts/soil-run.sh core/prelude.soil "$SOURCE" compiler/core.soil compiler/component-main.soil > /tmp/soil-record.wat
wasm-tools component embed tests/wit/record-probe.wit --world probe /tmp/soil-record.wat \
  -o /tmp/soil-record-embed.wasm
wasm-tools component new /tmp/soil-record-embed.wasm -o /tmp/soil-record.wasm
wasm-tools validate /tmp/soil-record.wasm
wasmtime run --invoke 'run()' /tmp/soil-record.wasm
