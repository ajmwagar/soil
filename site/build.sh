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

mkdir -p site/dist/examples
python3 site/gen-examples.py
cp site/loam.svg site/dist/loam.svg
cp site/strata.svg site/dist/strata.svg

"$BIN" \
  core/prelude.soil \
  core/ui.soil \
  core/highlight.soil \
  site/copy.soil \
  site/examples-code.soil \
  site/soil-lang.soil \
  site/build.soil \
  > site/dist/index.html

"$BIN" \
  core/prelude.soil \
  core/ui.soil \
  core/highlight.soil \
  site/copy.soil \
  site/soil-lang.soil \
  site/getting-started.soil \
  > site/dist/getting-started.html

for slug in counter todo calculator tictactoe tabs form; do
  "$BIN" \
    core/prelude.soil \
    core/ui.soil \
    core/highlight.soil \
    examples/ui/prelude.soil \
    "examples/ui/$slug.soil" \
    "site/pages/$slug.soil" \
    > "site/dist/examples/$slug.html"
done

site/docs.sh
echo "wrote site/dist/"
