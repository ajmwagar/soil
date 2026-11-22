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
