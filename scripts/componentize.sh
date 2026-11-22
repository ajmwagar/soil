#!/bin/sh
# Compile a core Wasm module and wrap it as a component with wasm-tools.
#
#   scripts/componentize.sh tests/wit/component-probe.wit core.wasm out.wasm
set -eu

WIT=${1:?usage: componentize.sh WIT CORE OUT}
CORE=${2:?usage: componentize.sh WIT CORE OUT}
OUT=${3:?usage: componentize.sh WIT CORE OUT}
EMBEDDED=$(mktemp "${TMPDIR:-/tmp}/soil-embed.XXXXXX.wasm")
trap 'rm -f "$EMBEDDED"' EXIT

wasm-tools component embed "$WIT" "$CORE" -o "$EMBEDDED"
wasm-tools component new "$EMBEDDED" -o "$OUT"
wasm-tools validate "$OUT"
