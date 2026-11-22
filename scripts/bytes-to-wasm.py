#!/usr/bin/env python3
"""Convert the Lisp byte list printed by the Soil compiler to a .wasm file."""

from pathlib import Path
import re
import sys

if len(sys.argv) != 3:
    raise SystemExit("usage: bytes-to-wasm.py INPUT.txt OUTPUT.wasm")

text = Path(sys.argv[1]).read_text().strip()
if text.startswith("(") and text.endswith(")"):
    text = text[1:-1]

values = [int(value, 10) for value in re.findall(r"-?\d+", text)]
if not values:
    raise SystemExit("no bytes found in compiler output")
if any(value < 0 or value > 255 for value in values):
    raise SystemExit("byte outside 0..255")

Path(sys.argv[2]).write_bytes(bytes(values))
print(f"wrote {len(values)} bytes to {sys.argv[2]}")
