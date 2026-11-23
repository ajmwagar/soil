# Object model for compiled Soil

Status: **kind-tagged objects, symbols, and environment tables**, 2026-11-23.

Soil owns the object model. Loam is the Rust/Wasm engine: it instantiates
modules, provides memory limits and host capabilities, and runs the code. Loam
does not own strings, cons cells, or the allocator.

## Decision

Linear memory plus i64 values, written in Soil and compiled by the Soil compiler.

```text
value = i64
  integer        immediate
  object handle  address in linear memory
```

Object layout, with a kind word so a handle can be inspected instead of assumed:

```text
cons      +0 kind=0, +8 car,  +16 cdr          (24 bytes)
string    +0 kind=1, +8 length, +16 element 0  (16 + 8n)
symbol    +0 kind=2, +8 length, +16 element 0  (16 + 8n)
```

One i64 per character keeps the binary emitter free of byte loads. A character
is its code, so string and symbol comparison is the same loop (`rt/text-equal?`),
and a symbol compares equal to the string that spells it — lookup does not
require interning to be correct.

## Environment

An environment is an association list of `(name . value)` pairs:

```lisp
(rt/env-new)
(rt/env-bind env name value)
(rt/env-lookup env name)
```

`env-bind` prepends, so the newest binding shadows older ones, which is the
semantics a lexical environment needs.

## Provenance

`compiler/runtime.soil` is the runtime. `tests/selfhost-objects.soil` holds only
probes, and `scripts/test-selfhost-objects.sh` concatenates the two before
compiling, so the tested runtime is the shipped one.

The gate runs with no Rust in the loop:

```text
soil-run.sh (bootstrapped soil-wasm.wasm) -> bytes-to-wasm.py -> wasmtime
```

```sh
scripts/test-selfhost-objects.sh
# ok rt/test-env-lookup = 42
# ok rt/test-env-shadow = 43
# selfhost-objects: ok
```

## Still ahead

- Byte-accurate strings: one i64 per character is fine for names, too wasteful
  for compiler output; needs `i32.load8_u`/`i32.store8` in the binary emitter.
- Symbols as a distinct type in the reader (today a literal is a string).
- Hash tables for large environments; the assoc list is O(n).
