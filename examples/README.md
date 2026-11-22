# Soil examples

Small programs for `core/`, `std/`, and the UI library.

```sh
# UI components: one self-contained HTML page
$SOIL_LANG \
  core/prelude.soil \
  core/ui.soil \
  examples/core/ui_components.soil \
  > /tmp/soil-components.html

# Date, URL, and JSON
$SOIL_LANG \
  core/prelude.soil \
  core/date.soil \
  core/url.soil \
  core/json.soil \
  examples/core/data.soil

# Event vocabulary without a bus
$SOIL_LANG \
  core/prelude.soil \
  std/soil/events.soil \
  examples/std/events.soil
```

The UI page is static SSR. `data.soil` prints values. `events.soil` encodes a
subscription and a body without opening a bus.
