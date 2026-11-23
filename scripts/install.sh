#!/bin/sh
# Versioned local install for the Soil toolchain.
#
#   scripts/install.sh [--prefix DIR] [--bin-dir DIR] \
#     [--soil-run PATH] [--soil-c PATH] [--soil-wasm PATH]
#
# The old ~/.local/share/soil/bin is intentionally left alone. Theseus and
# other consumers can keep using the legacy native runtime while the new
# toolchain is installed under versions/<sha> and exposed through current/.
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
PREFIX=${SOIL_PREFIX:-"$HOME/.local/share/soil"}
BIN_DIR=${SOIL_BIN_DIR:-"$HOME/.local/bin"}
VERSION=${SOIL_VERSION:-}
SOIL_RUN_ARG=${SOIL_RUN:-}
SOIL_C_ARG=${SOIL_C:-}
SOIL_WASM_ARG=${SOIL_WASM:-}
LINK=1

while [ "$#" -gt 0 ]; do
  case "$1" in
    --prefix) PREFIX=$2; shift 2 ;;
    --bin-dir) BIN_DIR=$2; shift 2 ;;
    --version) VERSION=$2; shift 2 ;;
    --soil-run) SOIL_RUN_ARG=$2; shift 2 ;;
    --soil-c) SOIL_C_ARG=$2; shift 2 ;;
    --soil-wasm) SOIL_WASM_ARG=$2; shift 2 ;;
    --no-link) LINK=0; shift ;;
    -h|--help)
      sed -n '2,18p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *) echo "install: unknown option $1" >&2; exit 2 ;;
  esac
done

if [ -z "$VERSION" ]; then
  VERSION=$(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || date +%Y%m%d%H%M%S)
fi

DEST="$PREFIX/versions/$VERSION"
rm -rf "$DEST"
mkdir -p "$DEST/bin" "$DEST/share"

cp -R "$ROOT/core" "$ROOT/std" "$ROOT/compiler" "$ROOT/tools" "$DEST/share/"
cp "$ROOT/tend.soil" "$DEST/share/"
cp "$ROOT/README.md" "$DEST/share/" 2>/dev/null || true

find_bin() {
  name=$1
  shift
  if [ -n "$1" ] && [ -x "$1" ]; then printf '%s\n' "$1"; return 0; fi
  for candidate in \
    "$ROOT/target/debug/$name" \
    "$ROOT/target/release/$name" \
    "$ROOT/../soil-rust/target/debug/$name" \
    "$ROOT/../soil-rust/target/release/$name"; do
    if [ -x "$candidate" ]; then printf '%s\n' "$candidate"; return 0; fi
  done
  command -v "$name" 2>/dev/null || true
}

SOIL_RUN_PATH=$(find_bin soil-run "$SOIL_RUN_ARG")
SOIL_C_PATH=$(find_bin soil-c "$SOIL_C_ARG")
if [ -n "$SOIL_RUN_PATH" ]; then cp "$SOIL_RUN_PATH" "$DEST/bin/soil-run"; fi
if [ -n "$SOIL_C_PATH" ]; then cp "$SOIL_C_PATH" "$DEST/bin/soil-c"; fi

SOIL_WASM_PATH=$SOIL_WASM_ARG
if [ -z "$SOIL_WASM_PATH" ]; then
  for candidate in \
    "$ROOT/target/wasm32-wasip2/release/soil-wasm.wasm" \
    "$ROOT/../soil-rust/target/wasm32-wasip2/release/soil-wasm.wasm"; do
    if [ -f "$candidate" ]; then SOIL_WASM_PATH=$candidate; break; fi
  done
fi
if [ -n "$SOIL_WASM_PATH" ] && [ -f "$SOIL_WASM_PATH" ]; then
  cp "$SOIL_WASM_PATH" "$DEST/bin/soil-wasm.wasm"
fi

cat > "$DEST/bin/soil" <<'SOIL'
#!/bin/sh
set -eu
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
SHARE="$ROOT/share"
cmd=${1:-}
if [ "$#" -gt 0 ]; then shift; fi

case "$cmd" in
  run)
    exec "$ROOT/bin/soil-run" "$@"
    ;;
  c)
    exec "$ROOT/bin/soil-c" "$@"
    ;;
  wasm)
    exec wasmtime run --dir . "$ROOT/bin/soil-wasm.wasm" "$@"
    ;;
  repl)
    exec wasmtime run --dir . --dir "$SHARE" "$ROOT/bin/soil-wasm.wasm" \
      --prelude "$SHARE/core/prelude.soil" --call main \
      "$SHARE/std/bindings/wasi-io.generated.soil" \
      "$SHARE/std/bindings/wasi-cli.generated.soil" \
      "$SHARE/core/repl.soil" "$SHARE/tools/repl/main.soil" "$@"
    ;;
  wit)
    if [ "$#" -lt 1 ]; then
      echo "usage: soil wit SOURCE.soil" >&2
      exit 2
    fi
    source=$(cat "$1")
    if [ -x "$ROOT/bin/soil-run" ]; then
      "$ROOT/bin/soil-run" --prelude "$SHARE/core/prelude.soil" \
        --call main "$SHARE/compiler/core.soil" \
        "$SHARE/compiler/wit-main.soil" --arg "$source" | sed '$d'
      exit 0
    fi
    wasmtime run --dir . --dir "$SHARE" "$ROOT/bin/soil-wasm.wasm" \
      --prelude "$SHARE/core/prelude.soil" --call main \
      "$SHARE/compiler/core.soil" "$SHARE/compiler/wit-main.soil" \
      --arg "$source" | sed '$d'
    exit 0
    ;;
  tend)
    exec "$ROOT/bin/tend" "$@"
    ;;
  *)
    echo "usage: soil <run|c|wasm|repl|wit|tend> ..." >&2
    exit 2
    ;;
esac
SOIL
chmod +x "$DEST/bin/soil"

cat > "$DEST/bin/tend" <<'TEND'
#!/bin/sh
set -eu
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
SHARE="$ROOT/share"
WASM="$ROOT/bin/soil-wasm.wasm"
if [ ! -f "$WASM" ]; then
  echo "tend: missing $WASM; install with --soil-wasm" >&2
  exit 1
fi
tmp=$(mktemp "${TMPDIR:-/tmp}/soil-tend.XXXXXX")
trap 'rm -f "$tmp"' EXIT HUP INT TERM
for arg in "$@"; do printf '%s\n' "$arg" >> "$tmp"; done
set -- --prelude "$SHARE/core/prelude.soil" --call main \
  "$SHARE/std/bindings/wasi-io.generated.soil" \
  "$SHARE/std/bindings/wasi-cli.generated.soil" \
  "$SHARE/std/wasi/stdio.soil" "$SHARE/std/wasi/fs.soil" \
  "$SHARE/compiler/core.soil" \
  "$SHARE/tools/tend/manifest.soil" "$SHARE/tools/tend/dispatch.soil" \
  "$SHARE/tools/tend/run.soil" "$SHARE/tools/tend/init.soil" \
  "$SHARE/tools/tend/build.soil" "$SHARE/tools/tend/main.soil"
while IFS= read -r arg; do set -- "$@" --arg "$arg"; done < "$tmp"

# Soil emits the core module; the canonical ABI is wasm-tools' job, which the
# sandbox cannot run (no process capability). The guest owns the result line;
# this side only finishes the component, and clears stale artifacts first so a
# refused build cannot be mistaken for a successful one.
rm -f build/module.wat build/world.txt build/embed.wasm
set +e
wasmtime run --dir . --dir "$SHARE" "$WASM" "$@"
STATUS=$?
set -e
[ "$STATUS" -eq 0 ] || exit "$STATUS"
[ -f build/module.wat ] || exit 0
WORLD=$(cat build/world.txt 2>/dev/null || true)
if [ -z "$WORLD" ]; then
  printf '(err "build produced a module with no world")\n' >&2
  exit 1
fi
if ! command -v wasm-tools >/dev/null 2>&1; then
  printf '(err "wasm-tools not found: build/module.wat left for a host that has it")\n' >&2
  exit 1
fi
wasm-tools component embed wit --world "$WORLD" build/module.wat -o build/embed.wasm
wasm-tools component new build/embed.wasm -o module.wasm
TEND
chmod +x "$DEST/bin/tend"

ln -sfn "$DEST" "$PREFIX/current"

if [ "$LINK" -eq 1 ]; then
  mkdir -p "$BIN_DIR"
  cat > "$BIN_DIR/soil" <<WRAPPER
#!/bin/sh
exec "$DEST/bin/soil" "\$@"
WRAPPER
  cat > "$BIN_DIR/tend" <<WRAPPER
#!/bin/sh
exec "$DEST/bin/tend" "\$@"
WRAPPER
  chmod +x "$BIN_DIR/soil" "$BIN_DIR/tend"
fi

printf 'installed Soil %s -> %s\n' "$VERSION" "$DEST"
printf '  soil: %s\n' "$BIN_DIR/soil"
printf '  tend: %s\n' "$BIN_DIR/tend"
