# Vitals

A Soil port of the host-free half of `spores/vitals/core`: the wearer-scoped
health fusion policy and the `vitals.summary.v1` / `vitals.view.v1` records.

The host still owns records, Unibus, time and pixels. This module owns:

- ring-primary / watch-fallback selection;
- one sleep session from one source, never averaged;
- per-device steps and the per-bucket step estimate;
- stale values as unknown, not zero;
- the view model a GPUI renderer draws.

## Load

```sh
$SOIL_LANG \
  --prelude core/prelude.soil \
  --call main \
  examples/vitals/vitals.soil your_program.soil
```

## Shape

```lisp
(let ((vitals (vitals/fuse signals "owner" now (vitals/policy))))
  (vitals/to-json vitals))   ; vitals.summary.v1
(let ((vitals (vitals/fuse signals "owner" now (vitals/policy))))
  (vitals/to-view vitals))   ; vitals.view.v1
```

`vitals/seed` and `vitals/seed-signals` are the deterministic dev fixture; they
match the Rust seed used by the Spore's tests.

## Tests

`tests/vitals.soil` ports the Rust core assertions. Run them with the normal
host-free runner:

```sh
SOIL_LANG=soil-lang python3 scripts/run-core-tests.py
```
