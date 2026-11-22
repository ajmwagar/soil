# soil-lang.org

Design brief: [`DESIGN.md`](DESIGN.md).

The Soil landing page and getting-started guide, written as `.soil` views.

All wording lives in `site/copy.soil`. `site/soil-lang.soil` owns the landing
structure and CSS; `site/getting-started.soil` owns the guide. The inline
examples live in `examples/ui/`.

Build the static site with a compatible `soil-lang` binary from the bootstrap
compiler or another host:

```sh
SOIL_LANG=/path/to/soil-lang ./site/build.sh
# site/dist/index.html
# site/dist/getting-started.html
# site/dist/examples/*.html
```

Or build and serve locally:

```sh
SOIL_LANG=/path/to/soil-lang ./site/serve.sh
# open http://127.0.0.1:8000/
# after the first build: SKIP_BUILD=1 ./site/serve.sh
```

For live reload while editing Soil:

```sh
SOIL_LANG=/path/to/soil-lang ./site/dev.sh
# open http://127.0.0.1:8000/
```

The dev server watches `site/`, `core/`, `std/`, and `examples/ui/`, rebuilds
after changes, and reloads open pages.

The generated API pages come from the `;;;` comments in `core/` and `std/`. The page
catalog — order, titles, summaries — lives in `site/docs-copy.soil`.

## Deploy

GitHub Pages builds `.github/workflows/pages.yml` on every push to `master`.
The workflow checks out `ajmwagar/soil-rust`, builds `soil-run`, and uses it as
`SOIL_LANG`:

```sh
SOIL_PRELUDE=core/prelude.soil SOIL_DOCS=1 SOIL_LANG=soil-run ./site/build.sh
```

`site/build.sh` writes `CNAME` (`soil-lang.org`) and `.nojekyll` into
`site/dist/`. Pages serves the uploaded artifact; `html_url` is
`https://soil-lang.org/` once DNS points at GitHub Pages.

Link checks are part of the Pages workflow:

```sh
node site/check-links.mjs site/dist
node site/check-links.mjs --base http://soil-lang.org
```

## Example runtime

Examples prefer the local `/__example` endpoint, which runs the real Soil
`update` and `view`. GitHub Pages is static, so `site/example.js` keeps one
small browser fallback per example. These are resilience code, not a second
implementation to extend: when the wasm evaluator can drive `update`/`view`,
remove the fallbacks rather than adding behavior to both paths.
