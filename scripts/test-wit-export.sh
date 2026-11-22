#!/bin/sh
# Compile export declarations to WIT and compare the expected contract.
set -eu
cd "$(dirname "$0")/.."
SOIL_LANG=${SOIL_LANG:-/Users/ajmwagar/src/singularity/soil/target/debug/soil-lang}
SOURCE=$(cat tests/wit-export.soil)
"$SOIL_LANG" --prelude core/prelude.soil --call main \
  compiler/core.soil compiler/wit-main.soil -- "$SOURCE" > /tmp/soil-export.wit
grep -F 'package acme:counter@0.1.0;' /tmp/soil-export.wit
grep -F 'export add: func(a: s32, b: s32) -> s32;' /tmp/soil-export.wit
grep -F 'export greet: func(name: string) -> string;' /tmp/soil-export.wit

wasm-tools component embed /tmp/soil-export.wit --world counter --dummy   -o /tmp/soil-export-dummy.wasm
wasm-tools component new /tmp/soil-export-dummy.wasm   -o /tmp/soil-export-component.wasm
wasm-tools component wit /tmp/soil-export-component.wasm > /tmp/soil-export-roundtrip.wit
grep -F 'export add: func(a: s32, b: s32) -> s32;' /tmp/soil-export-roundtrip.wit
