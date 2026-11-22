# Soil

**A Lisp for WebAssembly.**

[![license](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![language](https://img.shields.io/badge/language-Soil-c8f169.svg)](compiler/core.soil)
[![runtime](https://img.shields.io/badge/runtime-Loam-6f8cff.svg)](https://github.com/mycelium-linux/loam)

```lisp
(defun view (state)
  (el "main" nil
    (list
      (el "h1" nil (list (txt "Hello from Soil")))
      (el "button" (list (attr :data-op "inc"))
        (list (txt "+"))))))
```

Soil is the language, compiler, standard library, and examples. It compiles to
WebAssembly. You may use any Wasm runtime you like. However Soil is designed to run in [Loam](https://github.com/mycelium-linux/loam).

## What is here

| Path | What it is |
|---|---|
| [`compiler/core.soil`](compiler/core.soil) | The Soil-to-Wasm compiler |
| [`compiler/runtime.soil`](compiler/runtime.soil) | The first Soil object runtime |
| [`core/`](core) | Prelude and host-free modules: date, URL, JSON, UI, highlight, docs, CLI |
| [`std/`](std) | Modules that call `soil:*` WIT: events, records, time, perf |
| [`examples/`](examples) | Core, std, and UI programs |
| [`tools/tend/`](tools/tend) | The Soil build tool: manifest, lint, format, docs |
| [`tools/lsp/`](tools/lsp) | The Soil language server |
| [`site/`](site) | The soil-lang.org landing page and getting-started guide |
| [`tests/`](tests) | Core tests and the migrated Loam-host corpus |
| [`docs/`](docs) | Language, compiler, and object-model notes |

## Core and std

`core/` has no `soil:*` imports. `std/` is the layer that calls `soil:*` WIT.

### Core

| Module | What it does |
|---|---|
| [`prelude.soil`](core/prelude.soil) | Lists, JSON, records, and the small helpers every program uses |
| [`date.soil`](core/date.soil) | UTC formatting, parsing, day boundaries, and relative labels |
| [`url.soil`](core/url.soil) | Percent encoding, query strings, joining, and validation |
| [`json.soil`](core/json.soil) | Object helpers, deep merge, path set, and pretty printing |
| [`ui.soil`](core/ui.soil) | Forms, escaping, HTML rendering, and the component library |
| [`cli.soil`](core/cli.soil) | Command specs, argv parsing, help, and man pages |
| [`highlight.soil`](core/highlight.soil) | A small Soil tokenizer for code blocks |
| [`doc.soil`](core/doc.soil) | Documentation extraction and rendering |

### Std

| Module | What it does |
|---|---|
| [`wasi/time.soil`](std/wasi/time.soil) | Wall and monotonic time over `wasi:clocks` |
| [`wasi/perf.soil`](std/wasi/perf.soil) | Counters and timing helpers |
| [`soil/events.soil`](std/soil/events.soil) | Unibus subscriptions, bounded polling, publishing, and dedupe |
| [`soil/records.soil`](std/soil/records.soil) | Typed records and write helpers over `soil:records` |
| [`soil/index.soil`](std/soil/index.soil) | Workspace browse/search over `soil:index` |
| [`soil/vfs.soil`](std/soil/vfs.soil) | Read-only VFS helpers over `soil:vfs` |

## Documentation

You can learn more about Soil at [https://soil-lang.org](https://soil-lang.org)

## Examples

- [`examples/core/data.soil`](examples/core/data.soil) — date, URL, and JSON
- [`examples/std/events.soil`](examples/std/events.soil) — subscription and body encoding
- [`examples/core/ui_components.soil`](examples/core/ui_components.soil) — a page built from components
- [`examples/ui/counter.soil`](examples/ui/counter.soil) — the smallest complete UI
- [`examples/ui/`](examples/ui) — counter, todo, calculator, tic-tac-toe, tabs, form

## Running the output

Soil emits WebAssembly. You can run it with
[Wasmtime](https://wasmtime.dev/):

```sh
# Linux and macOS
curl https://wasmtime.dev/install.sh -sSf | bash

# confirm it is on PATH
wasmtime -V
```

Windows: download the MSI from the
[Wasmtime releases](https://github.com/bytecodealliance/wasmtime/releases/latest)
and install it.

Then run a module:

```sh
wasmtime run hello.wasm
```

Or call an exported function:

```sh
wasmtime run --invoke main hello.wasm
```

See the [compiler notes](compiler/README.md#run-it-in-wasmtime) for the current
bootstrap output and validation steps.

## Bootstrap

The Rust bootstrap compiler lives in
[`ajmwagar/soil-rust`](https://github.com/ajmwagar/soil-rust). It exists only
until Soil can compile itself.

```text
Rust bootstrap compiler
  → runs compiler/core.soil
  → emits Wasm
  → Soil compiler
  → emits Wasm
```

## Current state

- compiler: WAT and binary Wasm backends
- compiler host: runs inside the `wasm32-wasip2` interpreter
- object runtime: linear memory, heap global, bump allocator, cons cells
- not yet: tagged values, strings, byte buffers, full self-compilation, fixed point

## License

Dual-licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

## Tests

The test harness is Soil plus a shell driver; it does not use Python:

```sh
SOIL_WASM=/path/to/soil-wasm.wasm scripts/run-core-tests.sh
```

Each test runs in its own Wasmtime process.

## Local install

```sh
scripts/install.sh \
  --soil-run /path/to/soil-run \
  --soil-c /path/to/soil-c \
  --soil-wasm /path/to/soil-wasm.wasm
```

This installs under `~/.local/share/soil/versions/<sha>` and links
`~/.local/bin/soil` and `~/.local/bin/tend`. It does not replace the legacy
`~/.local/share/soil/bin` runtime.
