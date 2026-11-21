# marbles

A read-only Marbles board rendered by Soil. The host supplies one flat snapshot
— the array `marbles list --json` returns — and `ui.soil` decides every pixel.
There is no client JavaScript and no Canvas.

This is the first spore UI moved to the HTML/CSS path. It is deliberately a
projection: the Rust/SQLite or `soil:records` store stays authoritative.

## Build

```sh
./examples/marbles/build.sh
# examples/marbles/dist/index.html
```

Cargo is frozen: the script uses `.bin/soil-lang` or `target/debug/soil-lang`.
The fixture in `snapshot.json` is synthetic but uses the real `marbles
list --json` field shape. To use a live project, replace it with:

```sh
marbles -C /path/to/repo list --json > snapshot.json
./examples/marbles/build.sh
```

## Live host

The same view can run through the dev host with the snapshot as a prelude:

```sh
./examples/marbles/build.sh   # writes .state.soil for the host too

.bin/soil-ui-proto \
  --source examples/marbles/ui.soil \
  --prelude examples/marbles/.state.soil \
  --port 7890

SOIL_UI_UPSTREAM=http://127.0.0.1:7890 SOIL_UI_PORT=7891 \
  node crates/soil-ui-proto/web/dev-proxy.mjs

open 'http://127.0.0.1:7891/?bare=1'
```

The static build is pure SSR; the live host adds the existing Soil reconciler
so source edits hot-reload. Actions are intentionally no-ops for now.

## What it shows

- summary counts: ready, in flight, review, blocked, closed
- ready queue, in-flight, review, and blocked lanes
- issue type, priority, labels, claim, blockers, parent
- recently closed and the SSR contract in a side rail

The next slice is the write path: `claim`, `review`, and `close` become forms
that post intents back to the host while the read path stays a pure function.
