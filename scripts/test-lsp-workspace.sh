#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
WASM=${SOIL_WASM:-/tmp/soil-rust-target/wasm32-wasip2/release/soil-wasm.wasm}
for name in test/lsp-workspace-symbols test/lsp-workspace-definition; do
  wasmtime run --dir . "$WASM" --prelude core/prelude.soil --call "$name" \
    std/wasi/stdio.soil \
    std/soil/index.soil std/soil/vfs.soil \
    tools/lsp/protocol.soil tools/lsp/document.soil tools/lsp/workspace.soil \
    tests/lsp-workspace.soil
done
