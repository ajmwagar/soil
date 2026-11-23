# Soil compiler bootstrap

- `core.soil` — the Soil-written numeric-core compiler (WAT and binary backends)
- `main.soil` — the bootstrap CLI entry point; compiles the first `argv` string
- `runtime.soil` — the first Soil-written object runtime: memory, heap global,
  cons cells, car/cdr

## Build the bootstrap runner

The runner lives in [`ajmwagar/soil-rust`](https://github.com/ajmwagar/soil-rust):

```sh
git clone https://github.com/ajmwagar/soil-rust
cargo build --manifest-path soil-rust/Cargo.toml --bin soil-run
```

Or download a release binary.

## Compile the runtime smoke module

From this repository:

```sh
SOIL_RUN=${SOIL_RUN:-../soil-rust/target/debug/soil-run}

SOURCE="$(cat tests/runtime-smoke.soil)"
"$SOIL_RUN" \
  --prelude core/prelude.soil \
  --call main \
  compiler/core.soil compiler/main.soil \
  --arg "$SOURCE" > /tmp/runtime-bytes.txt

python3 scripts/bytes-to-wasm.py /tmp/runtime-bytes.txt /tmp/runtime.wasm
```

The compiler prints a Lisp list of bytes. `scripts/bytes-to-wasm.py` turns that
list into the actual Wasm binary.

## Run it in Wasmtime

Install Wasmtime with the official installer:

```sh
# Linux and macOS
curl https://wasmtime.dev/install.sh -sSf | bash

# Windows: use the MSI from
# https://github.com/bytecodealliance/wasmtime/releases/latest
wasmtime -V
```

Validate and run:

```sh
wasm-tools validate /tmp/runtime.wasm
wasmtime run --invoke rt/test-cons /tmp/runtime.wasm
# 1
wasmtime run --invoke rt/test-cdr /tmp/runtime.wasm
# 2
```

Wasmtime currently prints an experimental-feature warning for `--invoke` when
the function returns a value; the returned number is still the result.

## Self-tests

```sh
"$SOIL_RUN" \
  --prelude core/prelude.soil \
  --call main \
  compiler/core.soil tests/selfhost.soil tests/selfhost-main.soil
# true
```

## Direct WIT imports

The compiler now emits flat i64 WIT imports:

```lisp
(import "wasi:cli/stdin" "get-stdin" () i64)

(defun main ()
  (wasi:cli/stdin.get-stdin))
```

This emits a real Wasm import section:

```wat
(import "wasi:cli/stdin" "get-stdin" (func (result i64)))
```

Current limit: parameters and result are i64-only. Strings, lists, resources,
and canonical ABI lowering are the next slice.

## Macros

The Soil compiler expands target macros before lowering:

```lisp
(defmacro unless (c a b)
  `(if (not ,c) ,a ,b))

(defun choose (x)
  (unless (= x 0) 10 20))
```

`defmacro` forms are installed into the interpreter's macro table, then forms
are recursively expanded. Quoted forms are left alone. This is compile-time
expansion; macros are not part of the emitted runtime.

## Component target

`scripts/componentize.sh` wraps a core module with WIT metadata:

```sh
scripts/componentize.sh tests/wit/component-probe.wit \
  core.wasm component.wasm
```

The probe compiles a Soil function to a core export and lifts it into a real
component:

```lisp
(defun run () 42)
```

```sh
wasmtime run --invoke 'run()' /tmp/component-probe.wasm
# 42
```

This is the component-model path. Strings, lists, and resources still require
canonical-ABI lowering (`cabi_realloc`, memory export, post-return functions).

### First canonical-ABI import

`tests/component-cli.soil` emits a component that imports:

```wit
wasi:cli/environment.get-arguments() -> list<string>
```

and reads the length of the first returned string.

```sh
scripts/componentize-soil.sh
# 19 (the runner's first argv entry)
```

This required:

- `cm32p2_memory`
- `cabi_realloc`
- `i32` memory/address operations
- canonical return-area loading
- `i32.const`, `i32.load`, `i32.add`, `i64.extend_i32_u` in the WAT target

The binary emitter still targets the numeric core; the component target is the
WAT path for now.

### String literals

Source strings are emitted as data segments. `string-ptr` and `string-len`
produce the two halves of the canonical ABI string representation, so they can
be passed to an imported function without hand-writing stores:

```lisp
(import "test:strings" "log" "log" (i32 i32) unit)

(defun main ()
  (log (string-ptr "hello") (string-len "hello")))
```

```sh
scripts/test-component-string-literal.sh
```

### Canonical ABI probes

```sh
scripts/test-component-string.sh   # string export: "hi"
scripts/test-component-env.sh      # get-environment list count
scripts/test-component-clock.sh    # wall-clock record seconds
scripts/test-component-record.sh   # record export {x:7,y:9}
scripts/componentize-soil.sh       # get-arguments first-string length
```

The component path now covers:

```text
i32 memory and address operations
string exports
list<string> imports
record imports/exports
cabi_realloc with 8-byte alignment
return-area loading
```

Resources, nested records, options/results, and `soil:records`/`soil:vfs`/
`soil:inference` are still ahead.

```sh
scripts/test-component-list.sh     # list<u32> export: [10, 20, 30]
```
```sh
scripts/test-component-record-query.sh  # import: query-spec (strings, list<record>,
                                        # list<string>, option<string>) -> u32
```

The query-spec is `soil:records/query`'s parameter. It flattens to 12 core
values in declaration order:

```text
collection (ptr,len)  prefix (ptr,len)  filters (ptr,count)  fields (ptr,count)
cursor (disc,ptr,len: none = 0,0,0)     limit (u32)
```

`wasm-tools component new` accepting that core signature is the proof: the
adapter and our layout agree. This runs on the bootstrapped Wasm runtime because
the compiler needs `defmacro`, which the native interpreter does not have.

```sh
scripts/test-component-resource.sh  # pollable resource handle + method
scripts/test-component-abi.sh       # option, result, variant exports
```

Resource handles are i32 table indices at the core ABI boundary. The host owns
the table; the component adapter lowers/borrows/drops them.

The canonical ABI now covers:

```text
strings
lists
records
option
result
variants
resource handles
```

Still ahead: variants carrying strings/lists in all positions, flags, nested
records, and the real `soil:records` / `soil:vfs` / `soil:inference` worlds.


## Core import guard

`tend check-core` walks `core/`, parses each `.soil` file, and rejects import
forms or `wasi:`/`soil:` symbols. The check is Soil; the shell wrapper only
invokes the Wasmtime runner.

## Export declarations and WIT

A `defun` remains the implementation. An `export` form only declares the
WIT/component surface:

```lisp
(package "acme:counter@0.1.0")
(world "counter")

(defun add (a b)
  (+ a b))

(export add
  (a s32)
  (b s32)
  -> s32)
```

Emit the WIT description:

```sh
SOURCE="$(cat tests/wit-export.soil)"
"$SOIL_RUN" --prelude core/prelude.soil --call main \
  compiler/core.soil compiler/wit-main.soil --arg "$SOURCE"
```

```wit
package acme:counter@0.1.0;

world counter {
  export add: func(a: s32, b: s32) -> s32;
}
```

The export can appear before or after the `defun`. Rename with a string:

```lisp
(export "add" add-impl
  (a s32)
  (b s32)
  -> s32)
```

Current type syntax: scalar names, `(list T)`, `(option T)`, `(result T E)`,
`(tuple T ...)`, and named `record` declarations.

Records cannot be declared in a WIT world, so a source that declares any emits
them in an interface and the world exports that interface:

```soil
(package "fungos:hue@0.1.0")
(world "hue")
(interface "lights")

(record colour (x f32) (y f32))
(record light-state (id string) (on bool) (colour colour))

(export lights -> (list light-state))
```

```wit
package fungos:hue@0.1.0;

interface lights {
  record colour { x: f32, y: f32 }
  record light-state { id: string, on: bool, colour: colour }
  lights: func() -> list<light-state>;
}

world hue {
  export lights;
}
```

```sh
scripts/test-wit-export-records.sh
# wit-export-records: ok (interface + records + world export)
```

This proves the *contract*.

## Canonical ABI export: `list<record>`

`defun-export` lowers a record-list literal to the core function the component
model expects. A `list<record>` result flattens to `(ptr, len)` — two core
values — so the guest allocates a return area, writes the pair there and returns
its pointer:

```soil
(defun-export lights -> (list light-state)
  (list
    (light-state "hall" "Hall" #t 0.5 (colour 0.3 0.4))
    (light-state "desk" "Desk" #f 0.0 (colour 0.1 0.2))))
```

For the `light-state` above, the element stride is 32 and the field offsets are
the canonical ones:

```text
 0  id          (ptr, len)
 8  name        (ptr, len)
16  on          (u8)
20  brightness  (f32)
24  colour.x    (f32)
28  colour.y    (f32)
```

The export is named for the interface it belongs to —
`fungos:hue/lights@0.1.0#lights` — because that is the core export name the
component encoder looks up.

```sh
scripts/test-component-export-records.sh
# [{id: "hall", name: "Hall", on: true, brightness: 0.5, colour: {x: 0.3, y: 0.4}}, ...]
# component-export-records: ok (list<record> exported from Soil)
```

The gate runs the component, so it checks the layout, not just that the encoder
accepted the signature.

### Parameters

Parameters flatten into core params — a `string` is a `(ptr, len)` pair and the
host lowers it into the guest's memory, so a record can reference it directly
instead of copying:

```soil
(record target (id string) (on bool))

(defun-export set-light (id string) (on bool) -> target
  (target id on))
```

```sh
wasmtime run --invoke 'set-light("hall", true)' lights.wasm
# {id: "hall", on: true}
```

Limits of this slice: values are literals or direct parameters — nothing that is
read at runtime yet. String *literals* live in a static area at 64 KiB; the
record/list a function returns is `cabi_realloc`ed. No post-return. `option`,
`result`, variants and resources are still ahead.

## Selfhost compile

`soil-c` can now run the Soil-written compiler instead of the Rust lowering
path:

```sh
soil-c --selfhost \
  --soil-root /path/to/soil \
  input.soil output.wasm
```

The prelude defaults to `<soil-root>/core/prelude.soil`. The compiler sources
are:

```text
<soil-root>/compiler/core.soil
<soil-root>/compiler/main.soil
```

Use this for compiler work. The default non-selfhost mode remains the small Rust
bootstrap only.

Example:

```sh
soil-c --selfhost --soil-root . tests/runtime-smoke.soil /tmp/runtime.wasm
wasm-tools validate /tmp/runtime.wasm
wasmtime run --invoke rt/test-cons /tmp/runtime.wasm
# 1
```

## Object smoke

The selfhost compiler supports multi-form bodies, list construction, records,
and string objects through the runtime helpers:

```sh
SOIL_C=/path/to/soil-c scripts/test-selfhost-objects.sh
```

It covers:

```text
multi-form function body
list literal
record literal + field lookup
string literal
string equality
```

Run with:

```sh
SOIL_C=/tmp/soil-rust-target/debug/soil-c scripts/test-selfhost-objects.sh
```
