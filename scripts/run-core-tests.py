#!/usr/bin/env python3
"""Run the host-free Soil tests with an existing interpreter.

No Cargo, no generated bindings, no Loam. Set SOIL_LANG or pass --soil.
Host capability tests in tests/loam/ are intentionally excluded; they need a
WIT host (Wasmtime + generated bindings) or Loam.
"""

from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

CORE = [
    "core/prelude.soil",
    "core/cli.soil",
    "core/date.soil",
    "core/doc.soil",
    "core/highlight.soil",
    "core/json.soil",
    "core/ui.soil",
    "core/url.soil",
    "core/repl.soil",
    "core/repl-session.soil",
    "core/image.soil",
]
STD = [
    "std/wasi/time.soil",
    "std/soil/records.soil",
    "std/wasi/perf.soil",
    "std/soil/events.soil",
]
FIXTURES = [
    "boot/boot.soil",
    "examples/legacy/frigate_automation.soil",
    "examples/home/home.soil",
    "examples/home/ui.soil",
    "examples/home/dashboard.soil",
    "examples/hue/hue.soil",
    "examples/hue/hue_cli.soil",
    "examples/playback/playback.soil",
    "examples/playback/playback_cli.soil",
]
TESTS = [
    *[f"tests/loam/{name}.soil" for name in [
        "boot", "cli", "date", "defconst", "doc", "events", "frigate",
        "home", "hue", "json", "macros", "playback", "prelude", "tco",
        "ui", "url",
    ]],
    "tests/tend.soil",
    "tests/witgen.soil",
    "tests/lsp.soil",
    "tests/repl.soil",
    "tests/repl-session.soil",
    "tests/image.soil",
    "tests/selfhost.soil",
]


def load_files(test_file: str) -> list[str]:
    files = CORE + STD + sorted(
        str(path.relative_to(ROOT)) for path in (ROOT / "std/bindings").glob("*.soil")
    ) + FIXTURES + [test_file]
    if test_file == "tests/tend.soil":
        files += sorted(
            str(path.relative_to(ROOT)) for path in (ROOT / "tools/tend").glob("*.soil")
        )
    if test_file == "tests/lsp.soil":
        files += sorted(
            str(path.relative_to(ROOT)) for path in (ROOT / "tools/lsp").glob("*.soil")
        )
    if test_file == "tests/selfhost.soil":
        files += ["compiler/core.soil"]
    if test_file == "tests/witgen.soil":
        files += ["tools/witgen/main.soil"]
    return files


def test_names(source: str) -> list[str]:
    return re.findall(r"^\(defun\s+(test/[^\s()]+)", source, re.MULTILINE)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--soil", default=os.environ.get("SOIL_LANG", "soil-lang"))
    args = parser.parse_args()

    passed = failed = skipped = 0
    for test_file in TESTS:
        path = ROOT / test_file
        names = test_names(path.read_text())
        if not names:
            print(f"SKIP {test_file}: no test/... defun")
            skipped += 1
            continue
        files = load_files(test_file)
        for name in names:
            # The old local interpreter may not have read-document yet. The
            # scanner and position tests still run.
            if test_file == "tests/lsp.soil" and name in {
                "test/lsp-parse-ok", "test/lsp-parse-error",
                "test/lsp-diagnostics-empty", "test/lsp-diagnostics-error",
                "test/lsp-symbols", "test/lsp-definition", "test/lsp-comment-docs",
            }:
                print(f"SKIP {test_file}::{name}: needs read-document")
                skipped += 1
                continue
            result = subprocess.run(
                [args.soil, "--prelude", "core/prelude.soil", "--call", name, *files],
                cwd=ROOT, text=True, capture_output=True,
            )
            output = result.stdout.strip()
            if "unknown form or function `load`" in result.stderr:
                print(f"SKIP {test_file}::{name}: interpreter has no load")
                skipped += 1
            elif result.returncode != 0:
                print(f"ERR  {test_file}::{name}\n{result.stderr.strip()}")
                failed += 1
            elif output in {"", "false", "nil"}:
                print(f"FAIL {test_file}::{name}: {output or '<nil>'}")
                failed += 1
            else:
                passed += 1

    print(f"\n{passed} passed, {failed} failed, {skipped} skipped")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
