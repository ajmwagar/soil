#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
SOIL_RUN=${SOIL_RUN:-../soil-rust/target/debug/soil-run}
"$SOIL_RUN" --prelude core/prelude.soil --call main tests/package.soil
