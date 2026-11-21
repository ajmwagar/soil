# Andes and docs-kit

**Andes** ([`ajmwagar/andes`](https://github.com/ajmwagar/andes)) is the
canonical FPL documentation theme. It is a Vite docs runtime plus a prose
stylesheet that non-Vite renderers can link. It stays in its own repository; do
not vendor it into Soil.

`docs-kit` is the same engine under its original name. Andes is the
continuation and adds single-audience builds and `theme.css`. The two should not
be maintained in parallel: treat docs-kit as superseded and archive or redirect
it to Andes.

## How Soil uses it

Soil does not import Andes at build time. It emits documentation artifacts:

- `docs/*.md` — generated from `;;;` doc comments in the standard library
- `llms.txt` and `llms-full.txt` — agent-facing indexes
- `docs.json` — an Andes navigation manifest
- static HTML pages — a zero-JS readable fallback

Andes can render the Markdown by pointing its manifest at the Soil checkout.
`file` paths in `docs.json` are relative to `repoRoot`, so an Andes site with
`repoRoot: "/path/to/soil"` reads the generated pages without copying them.

Tend will need git dependencies to mount Andes directly. Until then the
generated Markdown and manifest are the integration boundary: no CSS is copied
out of Andes, and no Soil source moves into Andes.
