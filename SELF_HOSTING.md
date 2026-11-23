# Self-hosting status

What is self-hosted here is the **compiler**, not the machine. Soil source is
read, lowered and emitted to a Wasm module by a compiler written in Soil. The
Wasm VM underneath stays Rust — that is deliberate and not part of this gap.

```text
compiler/core.soil   1349 lines   reader→WAT, WAT→binary, WIT emitter, canonical ABI
compiler/runtime.soil  ~200 lines  the object runtime the compiled program links against
compiler/*-main.soil  5 lines ea   entry points (component WAT, WIT, binary)
```

The canonical repository contains **no Rust**. Everything measured below was run
on the bootstrapped runtime (`~/.local/share/soil/current/bin/soil-wasm.wasm`),
not on the native interpreter:

```sh
scripts/soil-run.sh core/prelude.soil "$SOURCE" compiler/core.soil compiler/main.soil
```

## Stage table

| stage | what it means | status | evidence |
| --- | --- | --- | --- |
| 0 | WAT compiler written in Soil | **done** | `compiler/compile-wat` |
| 1 | binary emitter in Soil, byte-matches the Rust reference on the corpus | **done** | `crates/soil-lang/tests/selfhost.rs` |
| 2 | object model the compiler needs to represent itself | **partial** | `compiler/runtime.soil` |
| 3 | compile the compiler, run the result, compare on the corpus | **not started** | fails at `missing index nil?` |
| 4 | fixed point: `Rust(compiler) == Soil(compiler)`, then re-bootstrap | **not started** | needs 3 |

## What works today

Toolchain, all on the bootstrapped runtime:

```text
203/203 core tests
13/13 component and WIT gates
23/23 object-runtime probes (compiler/runtime.soil + tests/selfhost-objects.soil)
```

The object model is kind-tagged:

```text
cons      +0 kind=0, +8 car,  +16 cdr          (24 bytes)
string    +0 kind=1, +8 length, +16 element 0  (16 + 8n, one i64 per char)
symbol    +0 kind=2, +8 length, +16 element 0
bytes     +0 kind=3, +8 length, +16 capacity, +24 data (u8)
env       assoc list of (name . value); env-bind prepends, so shadowing works
```

Byte buffers required `i32.load8_u` / `i32.store8` / `i32.add` in the **binary**
emitter; the WAT target already had them.

The test gate compiles `compiler/runtime.soil` **and** the probes together, so
what is tested is what ships:

```sh
scripts/test-selfhost-objects.sh
# ok rt/test-env-lookup = 42 … selfhost-objects: ok
```

## The gap

Asking the compiler to compile itself:

```sh
SOURCE=$(cat compiler/runtime.soil compiler/core.soil)
./scripts/soil-run.sh core/prelude.soil "$SOURCE" compiler/core.soil compiler/main.soil
# missing index nil?
```

The binary backend lowers a call it does not define to **a call by name**. So
compiling the compiler into one module needs every name the compiler itself
calls. Enumerated from `compiler/core.soil` (155 functions defined; the scan has
local-variable noise, the list below is the real primitive floor):

| implemented in `compiler/runtime.soil` | missing |
| --- | --- |
| `nil?` `first` `rest` `length` `append` `reverse` `cons` `car` `cdr` `field` | `map` `min` `max` `atom` `symbol?` `keyword?` `str` `read` `eval` `macroexpand` `println` `json-encode` `string->bytes` `string-chars` `string-join` |

Ordered by difficulty, not alphabet:

1. **Mechanical** — `map`, `min`, `max`, `atom`, `symbol?`, `keyword?`. Each is
   a few lines in Soil over the existing object model.
2. **Text and numbers** — `str`, `json-encode`, `string->bytes`,
   `string-chars`, `string-join`, `println`. `str` is the first real one: the
   compiler builds its output as text, so number and list formatting has to
   exist in Soil, and `json-encode` is what escapes it.
3. **The meta-level** — `read`, `eval`, `macroexpand`. This is the real work.
   The compiler expands macros at compile time by *evaluating* `defmacro` forms
   in its host; a self-compiled compiler needs that inside the compiled program,
   plus a reader, plus the environment semantics to go with it.

Nothing in the gap is a lowering problem. Lowering is done: records, lists,
options, parameters, both directions, verified by running the components.

## What "stage 3 done" will look like

```sh
# compiler + runtime compiled by the compiler, then run under wasmtime
scripts/selfhost-compile.sh          # emits compiler.wasm
wasmtime run --invoke compiler/main compiler.wasm <input.soil> > out.wasm
# out.wasm must byte-match the reference output for every corpus input
```

Then stage 4 adds the fixed point: use the compiler to compile the compiler,
and compare stage 2 and stage 3 outputs.

## Non-goals

- Replacing Wasmtime with a Soil VM.
- Bootstrapping without a Rust first stage: something has to run the first
  `soil-wasm.wasm`. Stage 0 is that something, and it is allowed to stay Rust.

## See also

- [docs/self-hosting.md](docs/self-hosting.md) — stage history and the
  differential-test rationale
- [docs/object-model.md](docs/object-model.md) — object layout and env semantics
- [docs/mutation-and-fuzzing.md](docs/mutation-and-fuzzing.md) — the testing
  work that comes after the compiler stops moving daily
