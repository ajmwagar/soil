#!/usr/bin/env sh
# Build the static site and serve site/dist locally.
# Set SKIP_BUILD=1 to serve the last build, PORT=9000 to change the port.
set -eu
cd "$(dirname "$0")/.."

if [ "${SKIP_BUILD:-0}" != "1" ]; then
  ./site/build.sh
fi

PORT=${PORT:-8000}
cd site/dist
exec python3 -m http.server "$PORT"
