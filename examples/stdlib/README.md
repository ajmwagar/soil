# Soil stdlib examples

Small, host-free programs for the libraries in `stdlib/`.

```sh
# UI components: one self-contained HTML page
$SOIL_LANG \
  stdlib/prelude.soil \
  stdlib/ui.soil \
  examples/stdlib/ui_components.soil \
  > /tmp/soil-components.html

# Date, URL, and JSON
$SOIL_LANG \
  stdlib/prelude.soil \
  stdlib/date.soil \
  stdlib/url.soil \
  stdlib/json.soil \
  examples/stdlib/data.soil

# Event vocabulary without a bus
$SOIL_LANG \
  stdlib/prelude.soil \
  stdlib/events.soil \
  examples/stdlib/events.soil
```

The UI page is static SSR. `data.soil` prints values. `events.soil` encodes a
subscription and a body without opening a bus.
