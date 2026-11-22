#!/bin/sh
# Run the LSP unit corpus under the Wasmtime runner.
set -eu
cd "$(dirname "$0")/.."
WASM=${SOIL_WASM:-/tmp/soil-rust-target/wasm32-wasip2/release/soil-wasm.wasm}
for name in \
  test/lsp-position-roundtrip \
  test/lsp-scan-ok test/lsp-scan-list test/lsp-scan-string \
  test/lsp-parse-ok test/lsp-parse-error \
  test/lsp-diagnostics-empty test/lsp-diagnostics-error \
  test/lsp-symbols test/lsp-definition test/lsp-comment-docs
do
  wasmtime run --dir . "$WASM" \
    --prelude core/prelude.soil --call "$name" \
    std/bindings/wasi-io.generated.soil \
    std/bindings/wasi-cli.generated.soil \
    std/wasi/stdio.soil \
    tools/lsp/protocol.soil tools/lsp/document.soil tools/lsp/main.soil \
    tests/lsp.soil
done
