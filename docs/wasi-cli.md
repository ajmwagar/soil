# WASI CLI for Soil commands

A Soil command needs the same inputs any other CLI has: argv, an allowlisted
environment, stdin/stdout/stderr, and an exit code. The standard interface is
`wasi:cli@0.2.4`; Soil does not invent a parallel `soil:cli`.

## What is implemented

- **Native argv:** `soil-lang FILE... -- ARG...` fills the `argv` form. The
  same list is returned by `cli-args` in
  [`bindings/wasi-cli.soil`](../crates/soil-lang/bindings/wasi-cli.soil).
- **Host side:** `soil-command` already links `wasmtime_wasi::p2`, sets
  `wasi.args`, and passes only `--env` values plus explicit stdin/stdout/stderr
  pipes. No host change was needed for argv.
- **Component world:** [`wit/soil-command/command.wit`](../wit/soil-command/command.wit) is the
  command shape. It imports `wasi:cli/environment`, `wasi:cli/exit`, and
  `soil:records`; it exports `wasi:cli/run`.
- **Vendored subset:** [`wit/soil-command/deps/wasi-cli`](../wit/soil-command/deps/wasi-cli)
  carries only the interfaces used above. `soil-witgen wit/soil-command`
  parses the package; `bindings/wasi-cli.generated.soil` is the generated
  reference. The full `wasi:cli` world also pulls
  filesystem, sockets, clocks, random, and io; those stay behind Soil grants.

## What remains

The current `soil-c` backend emits the numeric core and the small Lisp core,
not a component that runs the interpreter/stdlib and exports `wasi:cli/run`.
Until that guest emitter lands, a Soil command runs through the native
interpreter; the WIT world is the contract the emitter targets.

## Long-term command shape

```text
argv ──► wasi:cli/environment.get-arguments
env  ──► wasi:cli/environment.get-environment (allowlist only)
stdin/stdout/stderr ──► wasi:cli/stdio + wasi:io/streams
exit ──► wasi:cli/exit
data ──► soil:records (book + collection), not wasi:filesystem
```

A command that declares `soil:records` gets the records host; a command that
does not cannot reach it. `wasi:filesystem` is intentionally absent from the
world: paths are a host authority, and records/VFS are the durable interfaces.

The next implementation step is a `wasi:cli/run` guest that calls `main` with
`cli-args`; `mb` then runs as a normal `soil-command` package without a shell
shim.
