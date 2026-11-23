#!/bin/sh
# Build the minimal component probe and invoke its WIT export under Wasmtime.
set -eu
cd "$(dirname "$0")/.."
SOURCE=$(cat tests/component-probe.soil)
./scripts/soil-run.sh core/prelude.soil "$SOURCE" compiler/core.soil compiler/main.soil > /tmp/component-probe-bytes.txt
python3 scripts/bytes-to-wasm.py /tmp/component-probe-bytes.txt /tmp/component-probe-core.wasm
scripts/componentize.sh tests/wit/component-probe.wit \
  /tmp/component-probe-core.wasm /tmp/component-probe.wasm
wasmtime run --invoke 'run()' /tmp/component-probe.wasm
