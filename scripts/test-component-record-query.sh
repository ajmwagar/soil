#!/bin/sh
# Canonical ABI: lower soil:records/query's query-spec as an import.
#
# Proves the parameter lowering (strings, list<record>, list<string>,
# option<string> cursor) by building the core module, embedding it against the
# WIT world, and letting `wasm-tools component new` validate the adaptation.
set -eu
cd "$(dirname "$0")/.."
SRC=tests/component-records-query.soil
OUT=/tmp/records-query
SOURCE=$(cat "$SRC")

./scripts/soil-run.sh core/prelude.soil "$SOURCE" \
  compiler/core.soil compiler/component-main.soil > "$OUT.wat"

# 12 flattened spec parameters plus the indirect result<page, error> pointer.
grep -q 'records-query (param i32)' "$OUT.wat"
[ "$(tr ' ' '\n' < "$OUT.wat" | grep -c '^(param$')" -ge 13 ]
! grep -q '(result' "$OUT.wat"

wasm-tools component embed tests/wit/records-query.wit --world probe "$OUT.wat" -o "$OUT-embed.wasm"
wasm-tools component new "$OUT-embed.wasm" -o "$OUT.wasm"
wasm-tools validate "$OUT.wasm"

wasm-tools component wit "$OUT.wasm" | grep -q 'import test:records-query/records@0.1.0'

printf 'record-query: ok (12 spec params + result area, option cursor, component validates)\n'
