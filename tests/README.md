# Soil tests

## Core and tool tests

These run without Loam:

- `runtime-smoke.soil` — compile and run the object-runtime smoke module
- `selfhost.soil` — compiler self-tests
- `repl.soil`, `image.soil` — core REPL/image protocol
- `tend.soil` — `tools/tend`
- `lsp.soil` — `tools/lsp`

## Loam-host corpus

`tests/loam/` contains the prior `.soil` test corpus. These tests exercise
host-backed `soil:*` capabilities (VFS, records, HTTP, Unibus, time) and the
legacy fixture modules. They are migrated here as source of truth, but they
still need a Loam host or a Wasmtime WIT harness to run.

Do not add new Rust test harness code for these. The target is:

```text
Wasmtime + WASI + generated Soil WIT bindings
  -> the same .soil test corpus
```
