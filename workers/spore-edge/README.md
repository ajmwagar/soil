# spore-edge (spike)

Run a Soil Spore component in a Cloudflare Worker by transpiling the component
with `jco` and importing the resulting core wasm through Wrangler's
`CompiledWasm` rule.

This is a spike, not a host. It deliberately implements only enough WASI to
load a Spore and call its exports.

## Try it

```sh
cd workers/spore-edge
node build.mjs ~/src/fungOS/spores-scribe/spores/tui/module.wasm tui
wrangler dev --local --port 8799
```

Then:

```sh
curl http://127.0.0.1:8799/health
curl 'http://127.0.0.1:8799/render?cols=80&rows=24'
```

The response is the ANSI frame the TUI Spore normally renders in a terminal.

## What worked

- `wasm32-wasip2` components do not load in workerd directly. A Worker consumes
  core wasm modules, not the component model.
- `jco transpile` lowers the component to JS + core wasm.
- With `--tla-compat`, the worker awaits jco's `$init` inside `fetch`.
- The generated core modules must be imported through Wrangler's
  `CompiledWasm` rule. Leaving jco's `fetchCompile`/`WebAssembly.compile` loader
  in place makes the first request hang in local workerd until it is cancelled;
  later requests then work. `build.mjs` patches that loader to import the
  precompiled modules instead.
- The TUI Spore's `render`, `start`, and `frame` exports run unchanged.

## Limits

- No Cloudflare-native component model. Every Spore goes through jco.
- The `wasi:*` stubs satisfy the component boundary but do not implement real
  terminal, clock, or stream I/O. A Spore that calls them needs a real shim.
- Anything importing `soil:*` (records, canvas, appdata, vfs, audio, unibus) has
  no Worker host here. Such a Spore needs explicit Worker implementations for
  those interfaces, or a remote host behind an HTTP boundary.
- No durable state, no filesystem, no Spore supervision/lifecycle.
- Generated `spore/` is build output and is not committed.

## Next

1. Replace the `wasi:*` stubs with a real Worker WASI layer where needed.
2. Map `soil:records/query` onto D1 or an HTTP records host.
3. Add a Spore-to-Worker packaging step so a Spore can be deployed without
   checking in generated JS.
