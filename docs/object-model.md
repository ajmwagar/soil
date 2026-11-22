# Object model for compiled Soil

Status: **design gate**, 2026-11-22.

The numeric core is `i64`-only. The Soil compiler already runs in a Wasm
interpreter, but to compile itself directly to Wasm it needs values beyond
numbers:

- integers and booleans
- strings
- symbols
- lists
- byte buffers
- nil
- an output path

This is the object model decision. It is a bigger fork than adding another
operator.

## Recommendation: host-imported runtime first

Use one Wasm value type for user objects:

```text
value = i64
  integer       immediate
  object        handle into the host runtime table
```

The compiler emits imports for object operations. The host runtime implements
them with the canonical `soil_lang::sexp::Value`.

This keeps the first bootstrap tractable:

- no garbage collector in generated Wasm
- no linear-memory layout yet
- no allocator
- the Rust runtime remains the object authority
- the compiler is still Soil

The trade-off is explicit: a module built this way is not standalone. It needs
the Soil runtime to execute. That is acceptable while the runtime stays Rust.

## ABI sketch

```wit
package soil:runtime@0.1.0;

interface value {
  type value = s64;

  string-const: func(id: u32) -> value;
  symbol-const: func(id: u32) -> value;
  string-concat: func(left: value, right: value) -> value;
  string-bytes: func(value: value) -> value;
  length: func(value: value) -> value;
  first: func(value: value) -> value;
  rest: func(value: value) -> value;
  cons: func(head: value, tail: value) -> value;
  equal: func(left: value, right: value) -> value;
  read: func(source: value) -> value;
  write: func(value: value) -> unit;
}
```

The exact import names can be flatter (`soil.rt.string_concat`) for Wasm.

The host keeps a table:

```text
handle -> Value
```

Handles are opaque to compiled code. The compiler never invents one; it gets
them from imports and passes them back.

## Compiler changes

The core AST needs a second value class:

```text
Type = I64 | Value
```

Every expression gets a result type. Builtins are typed:

```text
str          (Value...) -> Value
string->bytes Value -> Value
length       Value -> I64
first/rest   Value -> Value
cons         Value, Value -> Value
read         Value -> Value
nil?/atom?   Value -> I64
```

The Wasm backend emits:

- a function type whose params/results are `i64`
- imports from `soil:runtime`
- calls for object builtins
- the existing numeric instructions for arithmetic/control flow

## Later: standalone object model

Once the compiler bootstraps against host imports, replace the imports with a
Wasm-native object runtime:

1. linear memory
2. tagged values or a small object header
3. bump allocator
4. strings, cons cells, arrays, symbols
5. mark/sweep or reference counting

That is where standalone modules and the fixed point become possible.

Wasm GC is a possible alternative, but it raises toolchain and encoder
questions. Do not make it the first bootstrap dependency.

## Bootstrap sequence

The current Rust `soil-c` seed is saved outside the repository:

```text
~/.local/share/soil/bootstrap/soil-c-rust-3f58bee9e33a
sha256 3f58bee9e33aecf79991a2e2bdfa1f81926d625aec9825d56427dacb857f53b5
source 96333190cc5e6105cf382406e0b3b79f63879c37
```

Order:

1. define the runtime ABI
2. add `Type` and object builtins to the core
3. compile the compiler with host imports
4. run it under Wasmtime with the Soil runtime linked
5. compare output with the saved Rust compiler
6. replace imports with the standalone object runtime
7. fixed point

## Non-goals for the first slice

- no GC
- no linear-memory strings
- no closures or higher-order calls
- no standalone compiler module
- no fixed point yet
