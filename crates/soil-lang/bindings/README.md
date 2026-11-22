# Generated bindings

Generated from Soil's WIT with:

```sh
cargo run -p soil-lang --bin soil-witgen -- \
  ../../wit/soil-records.wit --out bindings/soil-records.soil
```

The `.soil` files are S-expression stubs over `host-call`; the host owns grants
and dispatch. Regenerate from `wit/` rather than editing them by hand.
