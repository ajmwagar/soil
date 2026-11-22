# Soil

Soil is the Lisp-like language, compiler, and object runtime.

This repository is intentionally **100% Soil source**:

- `compiler/core.soil` — the Soil-to-Wasm compiler
- `compiler/runtime.soil` — the first Soil object runtime
- `stdlib/prelude.soil` — the language prelude
- `tests/` — Soil test programs
- `docs/` — language, compiler, and object-model design

## Bootstrap

The Rust bootstrap compiler lives in
[`ajmwagar/soil-rust`](https://github.com/ajmwagar/soil-rust). It exists only to
bootstrap this repository until the Soil compiler can compile itself.

```text
Rust bootstrap compiler
  → runs compiler/core.soil
  → emits Wasm
  → Soil compiler
  → emits Wasm
```

The Wasm engine is supplied separately by [Loam](https://github.com/mycelium-linux/loam).
Soil does not assume a specific engine; any Wasm host that provides the required
capabilities can run its output.

## Current state

- compiler: WAT and binary Wasm backends
- compiler host: runs inside the `wasm32-wasip2` interpreter
- object runtime: linear memory, heap global, bump allocator, cons cells
- not yet: tagged values, strings, byte buffers, full self-compilation, fixed point

## No Rust here

No Rust crates, Cargo files, or generated bindings live in this repository.
Those belong to the bootstrap compiler or to Loam.
