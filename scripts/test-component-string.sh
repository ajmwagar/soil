#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
SOURCE=$(cat tests/component-string.soil)
./scripts/soil-run.sh core/prelude.soil "$SOURCE" compiler/core.soil compiler/component-main.soil > /tmp/soil-string.wat
wasm-tools component embed tests/wit/string-probe.wit --world probe /tmp/soil-string.wat \
  -o /tmp/soil-string-embed.wasm
wasm-tools component new /tmp/soil-string-embed.wasm -o /tmp/soil-string.wasm
wasm-tools validate /tmp/soil-string.wasm
wasmtime run --invoke 'run()' /tmp/soil-string.wasm
