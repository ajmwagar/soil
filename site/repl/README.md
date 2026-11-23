# Browser REPL evaluator

`soil_repl_wasm_bg.wasm` is the Soil evaluator from the Rust language crate,
compiled for `wasm32-unknown-unknown` and wrapped with `wasm-bindgen`. It
evaluates reader forms, `defun`, `let`, `cond`, arithmetic, lists, JSON, and
the host-free prelude. `site/repl.js` boots it with `core/prelude.soil` inside
a dedicated worker per panel (`site/repl-worker.js`). The page terminates a
worker after three seconds so a slow edit cannot freeze the UI.

It is an artifact, not a Rust checkout: this repository stays Soil source. The
wrapper is the `Session::new(prelude)` / `Session::eval(source)` pair around
`Runtime` and `FakeHost`. The browser build stubs `Instant` because
`wasm32-unknown-unknown` has no monotonic clock; REPL stats report zero elapsed
milliseconds.

`site/build.sh` copies the wasm, the generated JS, and the current
`core/prelude.soil` into `site/dist/repl/`.
