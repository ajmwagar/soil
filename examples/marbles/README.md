# marbles

A read-only Marbles report surface rendered by Soil. The host supplies one
snapshot assembled from the reports the CLI already emits:

- `marbles burndown --json`
- `marbles portfolio --json`
- `marbles goal list --json`
- `marbles list --json`

`ui.soil` decides every pixel: burndown chart, portfolio rollup, goal progress,
and the ready queue. No client JavaScript, no Canvas, no Lua.

This is a projection, not a second tracker. The Rust/SQLite or `soil:records`
store stays authoritative.

## Build

```sh
./examples/marbles/build.sh
# examples/marbles/dist/index.html
```

Cargo is frozen: the script uses `.bin/soil-lang` or `target/debug/soil-lang`.
The fixture in `snapshot.json` is synthetic but uses the real report field
shapes.

## Refresh from a real store

```sh
MARBLES_BIN=/path/to/marbles MARBLES_DIR=/path/to/repo ./examples/marbles/refresh.sh
./examples/marbles/build.sh
```

`refresh.sh` reads the four reports for `MARBLES_PROJECT` (default `marbles`)
and rewrites `snapshot.json`.

## Live host

The same view runs through the dev host with the snapshot as a prelude:

```sh
./examples/marbles/build.sh   # writes .state.soil for the host too

.bin/soil-ui-proto \
  --source examples/marbles/ui.soil \
  --prelude examples/marbles/.state.soil \
  --port 7892

SOIL_UI_UPSTREAM=http://127.0.0.1:7892 SOIL_UI_PORT=7893 \
  node crates/soil-ui-proto/web/dev-proxy.mjs

open 'http://127.0.0.1:7893/?bare=1'
```

The static build is pure SSR; the live host adds the existing Soil reconciler
so source edits hot-reload. Actions are intentionally no-ops for now.

## Ported views

- **Burndown** — scope, done, remaining, burn/day, projected days, actual and
  ideal series.
- **Portfolio** — folder/project/goal rollup with progress bars, ready work,
  and staleness.
- **Goals** — descendant progress, not the container issue itself.
- **Queue** — ready/in-flight/review/blocked lanes and recently closed.

The next slice is the write path: `claim`, `review`, and `close` become forms
that post intents back to the host while the read path stays a pure function.
