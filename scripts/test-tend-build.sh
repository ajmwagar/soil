#!/usr/bin/env bash
# Gate for `tend build`: one Spore that must build and validate, one that must
# be refused, and a determinism check on the sorted source order.
#
#   SOIL_WASM=/path/to/soil-wasm.wasm scripts/test-tend-build.sh
set -uo pipefail

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
WASM=${SOIL_WASM:-"$ROOT/../soil-rust/target/wasm32-wasip2/release/soil-wasm.wasm"}

if [ ! -f "$WASM" ]; then
  echo "test-tend-build: missing soil-wasm.wasm; set SOIL_WASM" >&2
  exit 2
fi
if ! command -v wasm-tools >/dev/null 2>&1; then
  echo "test-tend-build: wasm-tools not found; skipping componentization checks" >&2
  exit 2
fi

failures=0
fail() {
  echo "FAIL $1: $2" >&2
  failures=$((failures + 1))
}

run_build() {
  local dir=$1
  (
    cd "$dir" || exit 1
    wasmtime run --dir . --dir "$ROOT" "$WASM" \
      --prelude "$ROOT/core/prelude.soil" --call main \
      "$ROOT/std/bindings/wasi-io.generated.soil" \
      "$ROOT/std/bindings/wasi-cli.generated.soil" \
      "$ROOT/std/wasi/stdio.soil" "$ROOT/std/wasi/fs.soil" \
      "$ROOT/compiler/core.soil" \
      "$ROOT/tools/tend/manifest.soil" "$ROOT/tools/tend/dispatch.soil" \
      "$ROOT/tools/tend/run.soil" "$ROOT/tools/tend/init.soil" \
      "$ROOT/tools/tend/build.soil" "$ROOT/tools/tend/main.soil" \
      --arg build
  )
}

tmp=$(mktemp -d "${TMPDIR:-/tmp}/tend-build.XXXXXX")
trap 'rm -rf "$tmp"' EXIT HUP INT TERM

# 1. A Spore that must build: members glob, sorted, then componentized.
cp -R "$ROOT/tests/fixtures/spore-hello" "$tmp/hello"
out=$(run_build "$tmp/hello" 2>&1)
echo "$out" | grep -q '^(ok ' || fail "hello" "build did not report ok: $out"
[ "$(echo "$out" | grep -c '^(ok ')" = "1" ] || fail "hello" "build printed $(
  echo "$out" | grep -c '^(ok '
) result lines; exactly one owner"
[ -f "$tmp/hello/build/module.wat" ] || fail "hello" "no build/module.wat"
[ "$(cat "$tmp/hello/build/world.txt" 2>/dev/null)" = "hello" ] ||
  fail "hello" "world.txt is not 'hello'"

first=$(cat "$tmp/hello/build/module.wat" 2>/dev/null)
run_build "$tmp/hello" >/dev/null 2>&1
second=$(cat "$tmp/hello/build/module.wat" 2>/dev/null)
[ "$first" = "$second" ] || fail "hello" "module.wat is not deterministic across runs"

(
  cd "$tmp/hello" || exit 1
  wasm-tools component embed wit --world hello build/module.wat -o build/embed.wasm &&
    wasm-tools component new build/embed.wasm -o module.wasm &&
    wasm-tools validate module.wasm &&
    wasm-tools component wit module.wasm | grep -q 'export add: func(a: s32, b: s32) -> s32'
) || fail "hello" "component does not validate or lost its export"

# 2. A Spore that must be refused, with no artifacts left behind.
cp -R "$ROOT/tests/fixtures/spore-hostcall" "$tmp/hostcall"
out=$(run_build "$tmp/hostcall" 2>&1)
echo "$out" | grep -q 'host-call -> typed WIT imports is not lowered yet' ||
  fail "hostcall" "refusal message changed: $out"
[ -d "$tmp/hostcall/build" ] && fail "hostcall" "refusal still wrote build/ artifacts"

# 3. Invalid WIT package is rejected before artifacts are written.
cp -R "$ROOT/tests/fixtures/spore-hello" "$tmp/badpkg"
sed -i.bak 's/local:hello@0.1.0/dev.local:hello@0.1.0/' "$tmp/badpkg/src/main.soil"
out=$(run_build "$tmp/badpkg" 2>&1)
echo "$out" | grep -q 'invalid WIT namespace' ||
  fail "badpkg" "invalid package was not rejected: $out"
[ -f "$tmp/badpkg/build/module.wat" ] && fail "badpkg" "invalid package still wrote module.wat"

# 4. A missing world is an error, not a silent skip.
cp -R "$ROOT/tests/fixtures/spore-hello" "$tmp/noworld"
sed -i.bak '/^(world /d' "$tmp/noworld/src/main.soil"
out=$(run_build "$tmp/noworld" 2>&1)
echo "$out" | grep -q 'no (world' || fail "noworld" "missing world was not rejected: $out"

if [ "$failures" -ne 0 ]; then
  echo "test-tend-build: $failures failure(s)" >&2
  exit 1
fi
echo "test-tend-build: ok"
