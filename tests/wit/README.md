# WIT fixtures

These are generator inputs, not host contracts.

```sh
wasm-tools component wit tests/wit/wasi-stdio -j > /tmp/wit.json
soil-run --prelude core/prelude.soil --call main tools/witgen/main.soil \
  --arg "$(cat /tmp/wit.json)" --arg "wasi:cli@0.2.4"
```

`wasi-stdio/` is a small `wasi:cli` + `wasi:io` subset used to exercise
resources, handles, and canonical interface keys.
