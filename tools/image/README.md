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

It does not yet persist or flush to a VFS. That is the next image slice.
