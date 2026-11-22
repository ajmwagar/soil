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

## UI

`ui.soil` is a complete HTML/CSS view over the same `vitals.view.v1` shape a
GPUI renderer reads. `page.soil` renders it to one document:

```sh
$SOIL_LANG --prelude core/prelude.soil \
  core/ui.soil examples/vitals/vitals.soil examples/vitals/ui.soil \
  examples/vitals/page.soil > /tmp/vitals.html
open /tmp/vitals.html
```

The rings, cards and sleep hypnogram are ordinary Soil forms; the host only
turns them into HTML. The rings are inline SVG arcs rather than CSS
`conic-gradient`, because Blitz renders conic gradients as a solid ring with no
progress. SVG arcs render correctly in both a browser and Blitz.

This is the second backend the view model needs before a GPUI adapter is worth
building.
