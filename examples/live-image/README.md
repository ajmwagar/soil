# Live image app

This app is loaded into one live Soil image. The process stays alive while
definitions are redefined.

Run the image server:

```sh
wasmtime run --dir . target/wasm32-wasip2/release/soil-wasm.wasm \
  --prelude core/prelude.soil \
  --call main \
  std/bindings/wasi-io.generated.soil \
  std/bindings/wasi-cli.generated.soil \
  std/wasi/stdio.soil std/wasi/fs.soil \
  core/image.soil tools/image/main.soil
```

Then send:

```json
{"op":"attach","session":"app"}
{"op":"load","session":"app","path":"examples/live-image/app.soil"}
{"op":"eval","session":"app","source":"(live-app/version)"}
{"op":"eval","session":"app","source":"(defun live-app/version () 2)"}
{"op":"eval","session":"app","source":"(live-app/version)"}
```

The first call returns `1`; the second returns `2` without restarting the image.

## SSR page

```json
{"op":"load","session":"app","path":"examples/live-image/page.soil"}
{"op":"eval","session":"app","source":"(page/render)"}
{"op":"eval","session":"app","source":"(defun page/title () \"Live Soil v2\")"}
{"op":"eval","session":"app","source":"(page/render)"}
```

The HTML changes while the image stays alive.
