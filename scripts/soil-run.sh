#!/bin/sh
# Run Soil with the bootstrapped runtime by default.
#
# The bootstrapped `soil-wasm.wasm` is the reference: it is Soil's own runtime,
# so a gate that only passes under the native Rust interpreter is not evidence
# about Soil. Set SOIL_WASM to pin a specific build, or SOIL_LANG=1 to force the
# native interpreter while debugging.
#
# usage: soil-run.sh PRELUDE SOURCE FILE...
set -eu

PRELUDE=$1
SOURCE=$2
shift 2

WASM=${SOIL_WASM:-"$HOME/.local/share/soil/current/bin/soil-wasm.wasm"}

if [ "${SOIL_LANG:-}" != "1" ] && [ -f "$WASM" ]; then
  exec wasmtime run --dir . "$WASM" --prelude "$PRELUDE" --call main "$@" --arg "$SOURCE"
fi

SOIL_LANG_BIN=${SOIL_LANG_BIN:-"$HOME/.local/share/soil/bin/soil-lang"}
exec "$SOIL_LANG_BIN" --prelude "$PRELUDE" --call main "$@" -- "$SOURCE"
