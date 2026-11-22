# Soil stdlib examples

Small, host-free examples for the libraries added in `crates/soil-lang/stdlib`.

```sh
# UI components: writes one self-contained HTML page
SOIL_LANG=${SOIL_LANG:-target/debug/soil-lang}
$SOIL_LANG \
  crates/soil-lang/stdlib/prelude.soil \
  crates/soil-lang/stdlib/ui.soil \
  examples/stdlib/ui_components.soil \
  > /tmp/soil-components.html
open /tmp/soil-components.html

# Date, URL, and JSON
$SOIL_LANG \
  crates/soil-lang/stdlib/prelude.soil \
  crates/soil-lang/stdlib/date.soil \
  crates/soil-lang/stdlib/url.soil \
  crates/soil-lang/stdlib/json.soil \
  examples/stdlib/data.soil

# Event vocabulary without a bus
$SOIL_LANG \
  crates/soil-lang/stdlib/prelude.soil \
  crates/soil-lang/stdlib/events.soil \
  examples/stdlib/events.soil

# Live Unibus event (needs a hub config)
$SOIL_LANG \
  --unibus-config ~/.config/unibus/client.json \
  crates/soil-lang/stdlib/prelude.soil \
  crates/soil-lang/bindings/soil-unibus.soil \
  crates/soil-lang/stdlib/events.soil \
  examples/stdlib/unibus_events.soil
```

The UI page is static SSR. `data.soil` prints values. `events.soil` only
encodes a subscription and a body; `unibus_events.soil` is the live path.
