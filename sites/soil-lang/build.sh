#!/usr/bin/env sh
# Build the static soil-lang.org page through the Soil stdlib.
set -eu
cd "$(dirname "$0")/../.."

BIN=${SOIL_LANG:-target/debug/soil-lang}
if [ ! -x "$BIN" ]; then
  cargo build -q -p soil-lang --bin soil-lang
fi

mkdir -p sites/soil-lang/dist
"$BIN" \
  crates/soil-lang/stdlib/prelude.soil \
  crates/soil-lang/stdlib/ui.soil \
  crates/soil-lang/stdlib/highlight.soil \
  sites/soil-lang/soil-lang.soil \
  sites/soil-lang/build.soil \
  > sites/soil-lang/dist/index.html

echo "wrote sites/soil-lang/dist/index.html"
