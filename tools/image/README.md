# image

In-memory image server over JSON lines.

```sh
wasmtime run --dir . target/wasm32-wasip2/release/soil-wasm.wasm \
  --prelude core/prelude.soil \
  --call main \
  std/bindings/wasi-io.generated.soil \
  std/bindings/wasi-cli.generated.soil \
  std/wasi/stdio.soil \
  core/image.soil \
  tools/image/main.soil
```

This is one live interpreter per process. It supports:

- attach / detach
- eval and load definitions
- status
- list
- ping

Snapshot/restore writes canonical Soil `(image ...)` text and reloads tracked source entries. It does not yet persist to a VFS or snapshot eval-only definitions. That is the next image slice.
