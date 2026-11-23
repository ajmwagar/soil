#!/bin/sh
# Compile export declarations that use named types to WIT.
#
# Records cannot be declared in a world, so a source with `(record ...)` emits
# an interface and the world exports it. The gate is that the emitted WIT
# round-trips through wasm-tools, with the interface, the records and the
# methods intact -- that is the contract a Soil-backed Hue lights surface needs.
set -eu
cd "$(dirname "$0")/.."
SOURCE=$(cat tests/wit-export-records.soil)
OUT=/tmp/soil-export-records

./scripts/soil-run.sh core/prelude.soil "$SOURCE" \
  compiler/core.soil compiler/wit-main.soil > "$OUT.wit"

grep -F 'package fungos:hue@0.1.0;' "$OUT.wit"
grep -F 'interface lights {' "$OUT.wit"
grep -F 'record light-state {' "$OUT.wit"
grep -F 'lights: func() -> list<light-state>;' "$OUT.wit"
grep -F 'set-light: func(id: string, on: bool) -> result<light-state, string>;' "$OUT.wit"
grep -F 'export lights;' "$OUT.wit"

# The emitted text has to be valid WIT, not just plausible text.
wasm-tools component wit "$OUT.wit" > "$OUT-roundtrip.wit"
grep -F 'record colour {' "$OUT-roundtrip.wit"
grep -F 'record light-state {' "$OUT-roundtrip.wit"

printf 'wit-export-records: ok (interface + records + world export)\n'
