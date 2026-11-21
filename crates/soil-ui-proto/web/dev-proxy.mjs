// SPDX-License-Identifier: MIT OR Apache-2.0
// Dev proxy: serve index.html and ui.js from disk, proxy everything else to
// the Rust host. This keeps JS/CSS edits live without rebuilding the Rust
// binary just to re-embed two static files.
//
//   node crates/soil-ui-proto/web/dev-proxy.mjs
//   open http://127.0.0.1:7879/

import http from "node:http";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const upstream = process.env.SOIL_UI_UPSTREAM ?? "http://127.0.0.1:7878";
const port = Number(process.env.SOIL_UI_PORT ?? 7879);

function asset(name, type, response) {
  const body = fs.readFileSync(path.join(here, name));
  response.writeHead(200, {
    "content-type": type,
    "content-length": body.length,
    "cache-control": "no-store",
  });
  response.end(body);
}

const server = http.createServer(async (request, response) => {
  const pathname = new URL(request.url, upstream).pathname;
  if (pathname === "/") return asset("index.html", "text/html; charset=utf-8", response);
  if (pathname === "/ui.js") return asset("ui.js", "text/javascript; charset=utf-8", response);

  // Static example pages used by the inline workbench. `/examples` without a
  // file is the API listing, so only intercept paths with a trailing file.
  if (pathname.startsWith("/examples/")) {
    const file = path.join(here, "../../../sites/soil-lang/dist", pathname);
    if (fs.existsSync(file) && fs.statSync(file).isFile()) {
      response.writeHead(200, {
        "content-type": "text/html; charset=utf-8",
        "cache-control": "no-store",
      });
      response.end(fs.readFileSync(file));
      return;
    }
  }

  const chunks = [];
  for await (const chunk of request) chunks.push(chunk);
  const body = chunks.length ? Buffer.concat(chunks) : undefined;
  const target = new URL(request.url, upstream);
  const fetched = await fetch(target, {
    method: request.method,
    headers: {
      "content-type": request.headers["content-type"] ?? "application/octet-stream",
    },
    body: request.method === "GET" || request.method === "HEAD" ? undefined : body,
  });
  response.writeHead(fetched.status, {
    "content-type": fetched.headers.get("content-type") ?? "application/octet-stream",
    "cache-control": "no-store",
  });
  response.end(Buffer.from(await fetched.arrayBuffer()));
});

server.listen(port, "127.0.0.1", () => {
  console.log(`soil-ui dev proxy: http://127.0.0.1:${port}/ -> ${upstream}`);
});
