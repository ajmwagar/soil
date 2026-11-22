# soil-lang.org

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
