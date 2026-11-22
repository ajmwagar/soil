# tend

Data-only project manifest and build tool.

```lisp
(tend
  (version 1)
  (members "core/*" "std/*" "tools/*")
  (command "fmt"
    (summary "Format Soil sources")
    (run "tools/tend/fmt.soil" "main")))
```

`tools/tend/run.soil` is the execution seam. It loads the target source,
resolves the named function, and applies the command args.

Run under Wasmtime:

```sh
wasmtime run --dir . target/wasm32-wasip2/release/soil-wasm.wasm \
  --prelude core/prelude.soil \
  --call main \
  std/bindings/wasi-io.generated.soil \
  std/bindings/wasi-cli.generated.soil \
  std/wasi/stdio.soil std/wasi/fs.soil \
  tools/tend/manifest.soil tools/tend/dispatch.soil tools/tend/run.soil \
  tools/tend/main.soil --arg fmt
```
