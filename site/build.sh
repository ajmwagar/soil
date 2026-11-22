#!/usr/bin/env sh
# Build soil-lang.org and the inline examples from the Soil source in this repo.
# Rust is not part of this checkout: provide a `soil-lang` binary from the
# bootstrap compiler (or any compatible host) with SOIL_LANG.
set -eu
cd "$(dirname "$0")/.."

BIN=${SOIL_LANG:-}
if [ -z "$BIN" ] && command -v soil-lang >/dev/null 2>&1; then
  BIN=$(command -v soil-lang)
fi
if [ -z "$BIN" ] || [ ! -x "$BIN" ]; then
  echo "set SOIL_LANG to a compatible soil-lang binary" >&2
  exit 1
fi

run_soil() {
  output=$(mktemp)
  "$BIN" "$@" > "$output"
  python3 - "$output" <<'PY'
import sys
text = open(sys.argv[1]).read()
sys.stdout.write(text[:-4] if text.endswith("nil\n") else text)
PY
  rm -f "$output"
}

mkdir -p site/dist/examples
python3 site/gen-examples.py
cp site/loam.svg site/dist/loam.svg
cp site/site.js site/dist/site.js
cp site/example.js site/dist/examples/example.js
mkdir -p site/dist/repl
cp site/repl/soil_repl_wasm.js site/repl/soil_repl_wasm_bg.wasm site/dist/repl/
cat core/prelude.soil core/url.soil core/json.soil core/date.soil core/ui.soil core/highlight.soil > site/dist/repl/prelude.soil
cp site/repl.js site/dist/repl.js
cp site/docs.js site/dist/docs.js
cp site/strata.svg site/dist/strata.svg
cp site/favicon.svg site/dist/favicon.svg
cp site/og.png site/dist/og.png

run_soil \
  core/prelude.soil \
  core/ui.soil \
  core/highlight.soil \
  site/copy.soil \
  site/examples-code.soil \
  site/soil-lang.soil \
  site/build.soil \
  > site/dist/index.html

run_soil \
  core/prelude.soil \
  core/ui.soil \
  core/highlight.soil \
  site/copy.soil \
  site/soil-lang.soil \
  site/getting-started.soil \
  > site/dist/getting-started.html

for slug in counter todo calculator tictactoe tabs form; do
  run_soil \
    core/prelude.soil \
    core/ui.soil \
    core/highlight.soil \
    examples/ui/prelude.soil \
    "examples/ui/$slug.soil" \
    "site/pages/$slug.soil" \
    > "site/dist/examples/$slug.html"
done

if [ "${SOIL_DOCS:-1}" = "1" ]; then
  site/docs.sh
fi
python3 site/enhance.py
printf "soil-lang.org\n" > site/dist/CNAME
touch site/dist/.nojekyll
echo "wrote site/dist/"
