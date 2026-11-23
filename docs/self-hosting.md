# Self-hosting the numeric-core compiler

Status: **bootstrap stages 0, 1, and the Wasm-hosted compiler**, 2026-11-22.

The Rust compiler remains the reference. The Soil compiler lives in
`compiler/core.soil` and has two backends:

- `compiler/compile-wat` emits WebAssembly text
- `compiler/compile-wasm` emits the binary module, byte-for-byte matching the
  Rust numeric-core compiler on the current corpus

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

- no function dependency closure
- no host imports or object model
- no self-compilation yet

### Stage 1 — binary emitter in Soil — done

`compiler/compile-wasm` now emits:

- LEB128 signed/unsigned integers
- type/function/export/code sections
- the same instruction stream as the Rust reference compiler

The differential test `crates/soil-lang/tests/selfhost.rs` compares the Soil
compiler's bytes with `soil_lang::compile` for addition, `if`, `let`, `cond`,
calls, `defconst`, `and`, `or`, and `not`.

### Wasm-hosted compiler — done

`compiler` embeds the Soil compiler and runs it on the
`wasm32-wasip2` interpreter:

```sh
wasmtime run --dir . soil-compiler-wasm.wasm input.soil > output.wasm
```

The compiler logic is Soil. The interpreter is the Rust-written `soil-lang`
core compiled to Wasm. This is not the fixed point yet, but it proves the
compiler can run in Wasm and emit a runnable module.

### Stage 2 — the object model — started

`compiler/runtime.soil` now implements the first Soil-native object
runtime: linear memory, a mutable heap global, bump allocation, and cons cells.
It is compiled by the Soil compiler and runs under Loam/Wasmtime. See
[Object model](object-model.md).

### Stage 2 — full object model

A compiler that can compile itself needs:

- strings
- lists or arrays
- byte buffers
- symbols
- a way to write bytes

That is the first real expansion of the numeric core. Without it, the compiler
can compile numeric functions but cannot be compiled itself.

### Stage 3 — measured blocker

Asking the compiler to compile itself today fails immediately:

```text
$ SOURCE=$(cat compiler/runtime.soil compiler/core.soil)
$ ./scripts/soil-run.sh core/prelude.soil "$SOURCE" compiler/core.soil compiler/main.soil
missing index nil?
```

The binary backend lowers a call it does not define to a call by name. Compiling
the compiler in one module therefore needs every name the compiler itself calls,
which is the "primitive floor" `boot/boot.soil` names — but now in Soil, in the
target module.

Enumerated statically from `compiler/core.soil` (defined = 155 functions; the
scan has local-variable noise, the primitives below are the real set):

```text
nil?  first  rest  length  append  reverse  map  min  max  atom
symbol?  keyword?  str  read  eval  macroexpand  println  json-encode
string->bytes  string-chars  string-join
```

`compiler/runtime.soil` now has the first slice (`rt/nil?`, `rt/first`,
`rt/rest`, `rt/length`, `rt/append`, `rt/reverse`, with probes). The hard end of
the list is the meta-level: `read`, `eval`, `macroexpand` — the compiler expands
macros at compile time by evaluating `defmacro` forms in its host, so a
self-compiled compiler needs macro expansion *inside the compiled program*, plus
a reader, plus `str`/`json-encode` for codegen text.

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

## Object smoke

The selfhost binary compiler now covers multiple body forms, list literals,
record literals, field lookup, and string objects:

```sh
SOIL_C=/tmp/soil-rust-target/debug/soil-c scripts/test-selfhost-objects.sh
```

This is still the numeric-core object model: i64 values, cons cells, and
length-plus-elements strings. It is enough to compile a small runtime, not yet
the full compiler.
