# marbles

A read-only Marbles report surface rendered by Soil. The host supplies one
snapshot assembled from the reports the CLI already emits:

- `marbles burndown --json`
- `marbles portfolio --json`
- `marbles goal list --json` and `marbles goal show <id> --json`
- `marbles plan gantt <plan> --json` when `MARBLES_PLAN` is set
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

## Refresh from the records store

`mb` is the records-backed command, so this path does not use a fixture:

```sh
MARBLES_DIR=/path/to/repo ./examples/marbles/refresh.sh
./examples/marbles/build.sh
```

`refresh.sh` runs the four reports from `MARBLES_DIR` (default `.`) and writes
the resolved scope into `snapshot.json`:

- project scope when a `.marbles/project.toml` is found;
- store scope with `MARBLES_SCOPE=store`.

`mb list`, `mb goal list`, and `mb burndown` are project-scoped by the shim's
project discovery. `mb portfolio` is store-wide; the UI selects the project
rollup named by `scope.slug`. Set `MARBLES_PLAN=<name>` to include the plan
Gantt; without it the plan section is omitted. See
[`docs/marbles-scope.md`](../../docs/marbles-scope.md) for the long-term
contract and explicit `--project`/`--folder`/`--goal` flags.

## Soil CLI (read path)

`examples/marbles/mb.soil` is the start of the Soil `mb` replacement. It reads
the same records book as the Rust Spore and uses the host's `scan-query`
projection, so `list --project` does not decode every issue in the book.

```sh
.bin/soil-lang   --records-root "$HOME/.local/share/fungos/records"   --book personal/avery/marbles   crates/soil-lang/stdlib/prelude.soil   crates/soil-lang/stdlib/cli.soil   crates/soil-lang/bindings/soil-records.soil   crates/soil-lang/bindings/soil-time.soil   crates/soil-lang/bindings/wasi-cli.soil   examples/marbles/mb.soil -- list --project marbles --limit 5

# ready and show also work
... examples/marbles/mb.soil -- ready --project marbles
... examples/marbles/mb.soil -- show marbles-an4z
```

The read path is the REPL-friendly part: load the same files and call
`(mb-ready result)` or `(mb-list result)` from a Soil session. Writes, claim
CAS, events, and the full command tree are the next port slice.

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
- **Goals** — descendant progress plus the `goal show` tree.
- **Plan** — Gantt rows, estimates, critical path, concurrency.
- **Queue** — ready/in-flight/review/blocked lanes and recently closed.

The next slice is the write path: `claim`, `review`, and `close` become forms
that post intents back to the host while the read path stays a pure function.
