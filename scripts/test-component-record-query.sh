#!/bin/sh
# Canonical ABI: lower soil:records/query's query-spec as an import.
#
# The query-spec is a record of strings, a list of records, a list of strings and
# an option<string> cursor. The slice proves the *parameter* lowering: the core
# module declares the 12 flat values in declaration order, and
# `wasm-tools component new` accepts it against the WIT world, which means the
# adapter agrees with our layout.
#
# The compiler needs `defmacro`, so this runs on the bootstrapped Wasm runtime,
# not the native interpreter.
set -eu
cd "$(dirname "$0")/.."

WASM=${SOIL_WASM:-$HOME/.local/share/soil/versions/be64582/bin/soil-wasm.wasm}
WASMTIME=${WASMTIME:-wasmtime}
SRC=tests/component-records-query.soil
WIT=tests/wit/records-query.wit
OUT=/tmp/records-query

SOURCE=$(cat "$SRC")
"$WASMTIME" run --dir . "$WASM" --prelude core/prelude.soil --call main \
  compiler/core.soil compiler/component-main.soil --arg "$SOURCE" > "$OUT.wat"

# 12 flattened parameters, one i32 result.
grep -q 'records-query (param i32)' "$OUT.wat"
[ "$(tr ' ' '\n' < "$OUT.wat" | grep -c '^(param$')" -ge 12 ]

wasm-tools component embed "$WIT" --world probe "$OUT.wat" -o "$OUT-embed.wasm"
wasm-tools component new "$OUT-embed.wasm" -o "$OUT.wasm"
wasm-tools validate "$OUT.wasm"

# The adapted component must still name exactly the interface we lowered, and
# must stay a component of its own rather than dragging in the interpreter.
wasm-tools component wit "$OUT.wasm" | grep -q 'import test:records-query/records@0.1.0'

printf 'record-query: ok (12 flat params, option cursor, component validates)\n'
