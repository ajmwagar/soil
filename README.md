# Soil

**A Lisp-like language for performant applications and automation.**

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
WebAssembly. The Wasm runtime is [Loam](https://github.com/mycelium-linux/loam).

This repository is **100% Soil source**: no Rust crates, Cargo files, or
generated bindings live here.

## What is here

| Path | What it is |
|---|---|
| [`compiler/core.soil`](compiler/core.soil) | The Soil-to-Wasm compiler |
| [`compiler/runtime.soil`](compiler/runtime.soil) | The first Soil object runtime |
| [`stdlib/`](stdlib) | The language prelude and standard library |
| [`examples/`](examples) | UI and stdlib programs |
| [`site/`](site) | The soil-lang.org landing page and getting-started guide |
| [`tests/`](tests) | Soil test programs |
| [`docs/`](docs) | Language, compiler, and object-model notes |

## Standard library

| Module | What it does |
|---|---|
| [`prelude.soil`](stdlib/prelude.soil) | Lists, records, JSON, and the small helpers every program uses |
| [`date.soil`](stdlib/date.soil) | UTC formatting, parsing, day boundaries, and relative labels |
| [`url.soil`](stdlib/url.soil) | Percent encoding, query strings, joining, and validation |
| [`json.soil`](stdlib/json.soil) | Object helpers, deep merge, path set, and pretty printing |
| [`events.soil`](stdlib/events.soil) | Unibus subscriptions, bounded polling, publishing, and dedupe |
| [`ui.soil`](stdlib/ui.soil) | Forms, escaping, HTML rendering, and the component library |
| [`records.soil`](stdlib/records.soil) | Typed records over the host records capability |
| [`cli.soil`](stdlib/cli.soil) | Command specs, argv parsing, help, and man pages |
| [`highlight.soil`](stdlib/highlight.soil) | A small Soil tokenizer for code blocks |
| [`doc.soil`](stdlib/doc.soil) | Documentation extraction and rendering |
| [`perf.soil`](stdlib/perf.soil) | Counters and timing helpers |

## Example site

The actual landing page and getting-started guide are Soil views:

- [`site/soil-lang.soil`](site/soil-lang.soil) — landing page structure and CSS
- [`site/getting-started.soil`](site/getting-started.soil) — build, run, examples
- [`site/pages/`](site/pages) — the inline UI demos
- [`site/copy.soil`](site/copy.soil) — all editable site text

Render it with a compatible bootstrap compiler or host:

```sh
SOIL_LANG=/path/to/soil-lang ./site/build.sh
# site/dist/index.html
# site/dist/getting-started.html
# site/dist/examples/*.html
```

## Examples

- [`examples/stdlib/data.soil`](examples/stdlib/data.soil) — date, URL, and JSON
- [`examples/stdlib/events.soil`](examples/stdlib/events.soil) — subscription and body encoding
- [`examples/stdlib/ui_components.soil`](examples/stdlib/ui_components.soil) — a page built from components
- [`examples/ui/counter.soil`](examples/ui/counter.soil) — the smallest complete UI
- [`examples/ui/`](examples/ui) — counter, todo, calculator, tic-tac-toe, tabs, form

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
