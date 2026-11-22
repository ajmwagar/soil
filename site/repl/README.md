# Browser REPL evaluator

`soil_repl_wasm_bg.wasm` is the Soil evaluator from the Rust language crate,
compiled for `wasm32-unknown-unknown` and wrapped with `wasm-bindgen`. It
evaluates reader forms, `defun`, `let`, `cond`, arithmetic, lists, JSON, and
the host-free prelude. `site/repl.js` boots it with `core/prelude.soil`.

It is an artifact, not a Rust checkout: this repository stays Soil source. The
wrapper is the `Session::new(prelude)` / `Session::eval(source)` pair around
`Runtime` and `FakeHost`.

`site/build.sh` copies the wasm, the generated JS, and the current
`core/prelude.soil` into `site/dist/repl/`.
