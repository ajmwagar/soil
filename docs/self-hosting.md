# Self-hosting the numeric-core compiler

Status: **bootstrap stage 0**, 2026-11-22.

The Rust compiler remains the reference. The first Soil compiler lives in
`examples/compiler/core.soil` and emits WebAssembly text (WAT) for the numeric
core.

```lisp
(compiler/compile-source "(defun add (a b) (+ a b))")
```

```wat
(module
  (func $add (export "add") (param $a i64) (param $b i64) (result i64)
    local.get $a
    local.get $b
    i64.add
  )
)
```

## Why start with WAT

WAT lets us prove the compiler pipeline without owning:

- Wasm binary section encoding
- LEB128
- opcode tables
- the object representation

It is still a compiler written in Soil, and its output runs under Wasmtime.

## Bootstrap stages

### Stage 0 — WAT compiler in Soil

- read forms with `(read source)`
- lower `defun`, `if`, arithmetic, comparisons, and calls
- emit text
- differential-test against the Rust compiler's shapes and semantics

Current limits:

- no `let`
- no `defconst` in the emitted module
- no function dependency closure
- no binary output

### Stage 1 — binary emitter in Soil

Replace text with bytes:

- LEB128
- type/function/export/code sections
- content-addressed VFS artifacts

This proves the compiler can produce the same artifact format as `soil-c`.

### Stage 2 — the object model

A compiler that can compile itself needs:

- strings
- lists or arrays
- byte buffers
- symbols
- a way to write bytes

That is the first real expansion of the numeric core. Without it, the compiler
can compile numeric functions but cannot be compiled itself.

### Stage 3 — compile the compiler

Use the Rust reference compiler to compile the Soil compiler to Wasm, then run
that Wasm compiler under the existing `soil-lang-wasm` runner.

### Stage 4 — fixed point

Compile the compiler with itself. Compare:

```text
Rust compiler(compiler)  ==  Soil compiler(compiler)
```

Then bootstrap again from the output. The fixed point is the real self-hosting
milestone.

## Guardrail

The Wasmtime **runtime** stays in Rust. Self-hosting applies to the compiler:
Soil source → Wasm artifact. It does not mean writing a Wasm VM in Soil.
