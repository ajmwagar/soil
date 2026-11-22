#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Smoke-test a soil-lsp process over real LSP framing.

Usage:
    tools/lsp/smoke.py -- <server command...>

For example, after building the package:
    tools/lsp/smoke.py -- soil command run soil-lsp
"""

from __future__ import annotations

import json
import subprocess
import sys
import threading


def frame(message: dict) -> bytes:
    body = json.dumps(message, separators=(",", ":")).encode()
    return f"Content-Length: {len(body)}\r\n\r\n".encode() + body


def read_message(stream) -> dict | None:
    length = None
    while True:
        line = stream.readline()
        if not line:
            return None
        if line in (b"\r\n", b"\n"):
            break
        name, _, value = line.decode().partition(":")
        if name.lower() == "content-length":
            length = int(value.strip())
    if length is None:
        raise RuntimeError("missing Content-Length")
    return json.loads(stream.read(length))


def main() -> int:
    argv = sys.argv[1:]
    if argv and argv[0] == "--":
        argv = argv[1:]
    if not argv:
        print(__doc__, file=sys.stderr)
        return 2
    process = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE)
    assert process.stdin and process.stdout
    messages = [
        {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}},
        {"jsonrpc": "2.0", "method": "initialized", "params": {}},
        {"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {
            "textDocument": {"uri": "file:///tmp/a.soil", "languageId": "soil",
                             "version": 1,
                             "text": ";;; Adds one.\n(defun inc (x) (+ x 1))\n(defun use () (inc 1))\n"}}},
        {"jsonrpc": "2.0", "id": 2, "method": "textDocument/documentSymbol",
         "params": {"textDocument": {"uri": "file:///tmp/a.soil"}}},
        {"jsonrpc": "2.0", "id": 3, "method": "textDocument/definition",
         "params": {"textDocument": {"uri": "file:///tmp/a.soil"},
                    "position": {"line": 2, "character": 15}}},
        {"jsonrpc": "2.0", "id": 4, "method": "shutdown", "params": {}},
        {"jsonrpc": "2.0", "method": "exit", "params": {}},
    ]
    for message in messages:
        process.stdin.write(frame(message))
        process.stdin.flush()

    responses: dict[int, dict] = {}
    diagnostics: list[dict] = []

    def read_until_exit() -> None:
        while True:
            message = read_message(process.stdout)
            if message is None:
                return
            if "id" in message:
                responses[message["id"]] = message
            elif message.get("method") == "textDocument/publishDiagnostics":
                diagnostics.append(message["params"])

    reader = threading.Thread(target=read_until_exit, daemon=True)
    reader.start()
    process.stdin.close()
    code = process.wait(timeout=20)
    reader.join(timeout=2)
    stderr = process.stderr.read().decode() if process.stderr else ""
    if code != 0:
        print(f"server exited {code}\n{stderr}", file=sys.stderr)
        return 1
    if len(responses) != 4:
        print(f"missing responses: {responses}\n{stderr}", file=sys.stderr)
        return 1
    capabilities = responses[1]["result"]["capabilities"]
    assert capabilities["definitionProvider"] is True
    symbols = responses[2]["result"]
    assert [s["name"] for s in symbols] == ["inc", "use"], symbols
    location = responses[3]["result"]
    assert location["uri"] == "file:///tmp/a.soil", location
    assert location["range"]["start"] == {"line": 1, "character": 0}, location
    assert diagnostics and diagnostics[-1]["diagnostics"] == [], diagnostics
    print("soil-lsp smoke: ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
