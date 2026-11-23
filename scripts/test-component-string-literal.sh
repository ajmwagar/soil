#!/bin/sh
# Compile a source string literal into a data segment and read its canonical
# ABI pointer and length back through a component export.
set -eu
cd "$(dirname "$0")/.."
SOIL_RUN=${SOIL_RUN:-../soil-rust/target/debug/soil-run}
SOURCE=$(cat tests/component-string-literal.soil)
"$SOIL_RUN" --prelude core/prelude.soil --call main compiler/core.soil compiler/component-main.soil \
  -- "$SOURCE" > /tmp/soil-string-literal.wat
# The bootstrap runner prints the nil return from `main`; the compiler already
# wrote the WAT to stdout.
if [ "$(tail -n 1 /tmp/soil-string-literal.wat)" = "nil" ]; then
  sed -i.bak '$d' /tmp/soil-string-literal.wat
fi
grep -q '(data (i32.const 65536) "hi")' /tmp/soil-string-literal.wat
wasm-tools component embed tests/wit/string-literal.wit --world probe \
  /tmp/soil-string-literal.wat -o /tmp/soil-string-literal-embed.wasm
wasm-tools component new /tmp/soil-string-literal-embed.wasm \
  -o /tmp/soil-string-literal.wasm
wasm-tools validate /tmp/soil-string-literal.wasm
[ "$(wasmtime run --invoke 'ptr()' /tmp/soil-string-literal.wasm)" = "65536" ]
[ "$(wasmtime run --invoke 'len()' /tmp/soil-string-literal.wasm)" = "2" ]
echo ok
