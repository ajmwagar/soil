#!/bin/sh
# Build the minimal component probe and invoke its WIT export under Wasmtime.
set -eu
cd "$(dirname "$0")/.."
SOIL_LANG=${SOIL_LANG:-/Users/ajmwagar/src/singularity/soil/target/debug/soil-lang}
SOURCE=$(cat tests/component-probe.soil)
"$SOIL_LANG" --prelude core/prelude.soil --call main compiler/core.soil compiler/main.soil \
  -- "$SOURCE" > /tmp/component-probe-bytes.txt
python3 scripts/bytes-to-wasm.py /tmp/component-probe-bytes.txt /tmp/component-probe-core.wasm
scripts/componentize.sh tests/wit/component-probe.wit \
  /tmp/component-probe-core.wasm /tmp/component-probe.wasm
wasmtime run --invoke 'run()' /tmp/component-probe.wasm
