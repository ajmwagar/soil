#!/bin/sh
# Thin invocation wrapper. The policy check itself is tools/tend/check-core.
set -eu
cd "$(dirname "$0")/.."
WASM=${SOIL_WASM:-/tmp/soil-rust-target/wasm32-wasip2/release/soil-wasm.wasm}
exec wasmtime run --dir . "$WASM" \
  --prelude core/prelude.soil --call main \
  std/bindings/wasi-io.generated.soil \
  std/bindings/wasi-cli.generated.soil \
  std/wasi/stdio.soil std/wasi/fs.soil \
  tools/tend/check-core/main.soil
