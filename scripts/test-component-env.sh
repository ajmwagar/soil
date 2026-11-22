#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
SOIL_LANG=${SOIL_LANG:-/Users/ajmwagar/src/singularity/soil/target/debug/soil-lang}
SOURCE=$(cat tests/component-env.soil)
"$SOIL_LANG" --prelude core/prelude.soil --call main compiler/core.soil compiler/component-main.soil \
  -- "$SOURCE" > /tmp/soil-env.wat
wasm-tools component embed tests/wit/wasi-stdio --world probe /tmp/soil-env.wat \
  -o /tmp/soil-env-embed.wasm
wasm-tools component new /tmp/soil-env-embed.wasm -o /tmp/soil-env.wasm
wasm-tools validate /tmp/soil-env.wasm
wasmtime run --env SOIL_TEST_ENV=yes --invoke 'run()' /tmp/soil-env.wasm
