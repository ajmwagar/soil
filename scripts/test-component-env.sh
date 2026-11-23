#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
SOURCE=$(cat tests/component-env.soil)
./scripts/soil-run.sh core/prelude.soil "$SOURCE" compiler/core.soil compiler/component-main.soil > /tmp/soil-env.wat
wasm-tools component embed tests/wit/wasi-stdio --world probe /tmp/soil-env.wat \
  -o /tmp/soil-env-embed.wasm
wasm-tools component new /tmp/soil-env-embed.wasm -o /tmp/soil-env.wasm
wasm-tools validate /tmp/soil-env.wasm
wasmtime run --env SOIL_TEST_ENV=yes --invoke 'run()' /tmp/soil-env.wasm
