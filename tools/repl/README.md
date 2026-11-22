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

`core/repl-session.soil` adds one image, many clients. Each client has an
independent input buffer and outbox, while evaluation is serialized through the
image. A socket or web adapter only reads a line, calls `session/step`, and
drains that client's events.

Socket transport is intentionally not built on `wasi:sockets`. The host owns
the listener and grants; a Loam adapter should accept through `soil:streams`
once the generic runner exposes it.
