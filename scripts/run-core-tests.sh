#!/usr/bin/env bash
# Run the host-free Soil tests with the new Wasmtime runner.
#
# Usage:
#   SOIL_WASM=/path/to/soil-wasm.wasm scripts/run-core-tests.sh
#
# Each test gets its own process, matching the old Python harness isolation.
set -u

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT"

WASM=${SOIL_WASM:-"$ROOT/../soil-rust/target/wasm32-wasip2/release/soil-wasm.wasm"}
if [ ! -f "$WASM" ]; then
  echo "run-core-tests: missing soil-wasm.wasm; set SOIL_WASM" >&2
  exit 2
fi

CORE=(
  core/prelude.soil
  core/cli.soil
  core/date.soil
  core/doc.soil
  core/highlight.soil
  core/json.soil
  core/ui.soil
  core/url.soil
  core/repl.soil
  core/repl-session.soil
  core/image.soil
)
STD=(
  std/wasi/time.soil
  std/soil/records.soil
  std/wasi/perf.soil
  std/soil/events.soil
  std/loam/edit.soil
)
FIXTURES=(
  boot/boot.soil
  examples/legacy/frigate_automation.soil
  examples/home/home.soil
  examples/home/ui.soil
  examples/home/dashboard.soil
  examples/hue/hue.soil
  examples/hue/hue_cli.soil
  examples/playback/playback.soil
  examples/playback/playback_cli.soil
)
TESTS=(
  tests/loam/boot.soil tests/loam/cli.soil tests/loam/date.soil
  tests/loam/defconst.soil tests/loam/doc.soil tests/loam/events.soil
  tests/loam/frigate.soil tests/loam/home.soil tests/loam/hue.soil
  tests/loam/json.soil tests/loam/macros.soil tests/loam/playback.soil
  tests/loam/prelude.soil tests/loam/tco.soil tests/loam/ui.soil
  tests/loam/url.soil
  tests/tend.soil
  tests/witgen.soil
  tests/lsp.soil
  tests/repl.soil
  tests/repl-session.soil
  tests/package.soil
  tests/package-resolution.soil
  tests/image.soil
  tests/selfhost.soil
  tests/edit.soil
  tests/safe-eval.soil
  tests/edit-lsp.soil
)

base_files() {
  local f
  for f in "${CORE[@]}" "${STD[@]}"; do printf '%s\n' "$f"; done
  for f in std/bindings/*.soil; do printf '%s\n' "$f"; done
  for f in "${FIXTURES[@]}"; do printf '%s\n' "$f"; done
}

test_files() {
  local test_file=$1
  base_files
  case "$test_file" in
    tests/tend.soil) for f in tools/tend/*.soil; do printf '%s\n' "$f"; done ;;
    tests/lsp.soil) for f in tools/lsp/*.soil; do printf '%s\n' "$f"; done ;;
    tests/selfhost.soil) printf '%s\n' compiler/core.soil ;;
    tests/witgen.soil) printf '%s\n' tools/witgen/main.soil ;;
    tests/edit-lsp.soil)
      printf '%s\n' tools/lsp/protocol.soil tools/lsp/document.soil tools/lsp/workspace.soil tools/lsp/analyze.soil
      printf '%s\n' std/loam/edit-lsp.soil
      ;;
  esac
  printf '%s\n' "$test_file"
}

passed=0
failed=0
skipped=0
for test_file in "${TESTS[@]}"; do
  if [ ! -f "$test_file" ]; then
    echo "SKIP $test_file: missing"
    skipped=$((skipped + 1))
    continue
  fi
  names=$(sed -n 's/^(defun[[:space:]][[:space:]]*\(test\/[^[:space:]()]*\).*/\1/p' "$test_file")
  if [ -z "$names" ]; then
    echo "SKIP $test_file: no test/... defun"
    skipped=$((skipped + 1))
    continue
  fi
  files=()
  while IFS= read -r f; do files+=("$f"); done < <(test_files "$test_file")
  for name in $names; do
    out=$(SOIL_TEST="$name" wasmtime run --dir . --env SOIL_TEST "$WASM" \
      --prelude core/prelude.soil --call main \
      "${files[@]}" tools/test/run-one.soil 2>/tmp/soil-test.err)
    status=$?
    output=$(printf '%s' "$out" | tr -d '\r')
    if [ "$status" -ne 0 ]; then
      echo "ERR  $test_file::$name"
      sed -n '1,8p' /tmp/soil-test.err
      failed=$((failed + 1))
    elif [ -z "$output" ] || [ "$output" = "false" ] || [ "$output" = "nil" ]; then
      echo "FAIL $test_file::$name: ${output:-<nil>}"
      failed=$((failed + 1))
    else
      passed=$((passed + 1))
    fi
  done
done

printf '\n%d passed, %d failed, %d skipped\n' "$passed" "$failed" "$skipped"
[ "$failed" -eq 0 ]
