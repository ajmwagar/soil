# Phonebook

An Amazon-phone-tool-style identity directory for the holding. One record model,
two realms:

- **Meatspace** — humans. Warm paper, orange rules, a portrait, contact card,
  tenure, manager, skills, projects, and the badge wall their colleagues wrote.
- **Cyberspace** — agents. The same record shape in a dark register: owner,
  runtime, model, tools, trust level, last seen, and the work they are trusted
  with.

The realm is **derived** from `kind` (`human` / `agent`). There is one
stylesheet; a page cannot land in the wrong world by forgetting an argument.

## Why it is a Spore

Identity is shared state with a long life. It belongs in `soil:records`, not in
a chat transcript or a per-harness config file. The Spore owns the schema and
the rendering; the host owns durability, revisions, and sync.

```
identity.profile/<handle>   one subject (human or agent)
identity.badge/<id>         one badge definition
```

Badge grants live on the profile (`badges: [{badge, on, by}]`) and reference a
definition. Tenure badges are derived from `started_on`; nobody grants a
five-year badge. A grant that names a missing badge renders as an explicit
"undefined" tile rather than silently disappearing.

## Run the SSR page

The page is ordinary Soil, rendered on the host. There is no client app.

```sh
SOIL=/path/to/soil-lang      # or the soil-wasm runner
FILES="core/prelude.soil core/ui.soil core/date.soil \
  std/soil/records.soil std/bindings/soil-records.soil \
  std/bindings/wasi-cli.soil \
  spores/phonebook/source/model.soil spores/phonebook/source/fixtures.soil \
  spores/phonebook/source/ui.soil spores/phonebook/source/page.soil"

# directory
$SOIL --prelude core/prelude.soil --records-root /tmp/phonebook $FILES > index.html

# one profile
$SOIL --prelude core/prelude.soil --records-root /tmp/phonebook $FILES -- avery > avery.html
```

With no records backend the page falls back to the checked-in fixtures, so it is
useful before the first seed.

## Seed the records

```sh
$SOIL --prelude core/prelude.soil --records-root /tmp/phonebook \
  core/ui.soil core/date.soil \
  std/soil/records.soil std/bindings/soil-records.soil \
  spores/phonebook/source/model.soil spores/phonebook/source/fixtures.soil \
  spores/phonebook/source/seed.soil
```

A records store with data wins over the fixtures.

## Static snapshot

`web/index.html` and `web/<handle>.html` are generated from the same page with
the fixtures. They are the immediate, servable surface; the SSR entry is the
dynamic path.

## Tests

`tests/phonebook.soil` holds the host-free assertions: realm selection, initials,
tenure across an anniversary, badge lookup, the badge wall count, the manager
edge, and the rendered realm class.

## Target architecture

The Rust `meatspace` app is **frozen**. This Spore is the identity surface.

- **Loam / Mycelium** own `soil:principal` (auth/session), `soil:records`
  (profiles + badges), directory sync from fpl-auth, `soil:vfs` (photos), and
  `.myc` leases/routing.
- **This Spore** owns the domain rules (realm, tenure, badge grants, manager
  graph) and the SSR/HTML/CSS surface.
- No standalone Rust identity web server. The Rust app is a contract reference
  until the parity checklist is green, then its web layer is deleted and the
  repo archived.

See `../../charter/03-internal-affairs.md`.

## Build status

This is a source-distributed Spore (`distribution "source"`). The `module.wasm`
build is intentionally not claimed here: the current bootstrap compiler does not
lower arbitrary `host-call` forms into typed WIT imports. The SSR page runs on
the interpreter today, and `tend build` / `wasm-tools validate` must pass before
this README calls it a binary Spore.

## Shape

```
spore.soil              runtime manifest
wit/phonebook.wit       component contract (records + wasi cli/clocks)
source/model.soil       schema names, tenure, badges, realm
source/fixtures.soil    deterministic seed and demo
source/ui.soil          HTML/CSS, both realms
source/page.soil        SSR entry (records first, fixtures fallback)
source/seed.soil        upsert the fixtures into soil:records
tests/phonebook.soil    host-free assertions
web/                    generated snapshot
```
