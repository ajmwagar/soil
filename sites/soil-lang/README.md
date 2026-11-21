# soil-lang.org

The first landing page for Soil, written as a single `.soil` view.

Preview it full-screen through the dev proxy:

```sh
# 1. Rust host (no rebuild needed for the site source)
target/debug/soil-ui-proto --source sites/soil-lang/soil-lang.soil --port 7882

# 2. Serve the current web assets from disk and proxy the API
SOIL_UI_UPSTREAM=http://127.0.0.1:7882 SOIL_UI_PORT=7883 \
  node crates/soil-ui-proto/web/dev-proxy.mjs

# 3. Open the bare preview (no editor panel)
open 'http://127.0.0.1:7883/?bare=1'
```

The page is a static landscape: nav, hero, why, learn, spores, community, footer.
It has no actions and no state. The same forms can later be exported to plain
HTML by walking the tree the host already returns from `/view`.
