# repl

The interactive Soil REPL loop.

```sh
wasmtime run --dir . target/wasm32-wasip2/release/soil-wasm.wasm \
  --prelude core/prelude.soil \
  --call main \
  std/bindings/wasi-io.generated.soil \
  std/bindings/wasi-cli.generated.soil \
  core/repl.soil \
  tools/repl/main.soil
```

The state machine lives in `core/repl.soil`. This file supplies stdin/stdout and
the `load` action. `load` is a core primitive because the REPL must install
`defun` forms; `(eval (read source))` only evaluates expressions.
