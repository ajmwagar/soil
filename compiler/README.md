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
  --prelude stdlib/prelude.soil \
  --call main \
  compiler/core.soil compiler/main.soil \
  --arg "$SOURCE" > /tmp/runtime-bytes.txt

python3 scripts/bytes-to-wasm.py /tmp/runtime-bytes.txt /tmp/runtime.wasm
```

The compiler prints a Lisp list of bytes. `scripts/bytes-to-wasm.py` turns that
list into the actual Wasm binary.

## Run it in Wasmtime

Install Wasmtime:

```sh
curl -sSfL https://wasmtime.dev/install.sh | bash
export PATH="$HOME/.wasmtime/bin:$PATH"
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
  --prelude stdlib/prelude.soil \
  --call main \
  compiler/core.soil tests/selfhost.soil tests/selfhost-main.soil
# true
```
