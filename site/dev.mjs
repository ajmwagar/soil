#!/usr/bin/env node
// Live-reloading dev server for soil-lang.org.
//
//   SOIL_LANG=/path/to/soil-lang node site/dev.mjs
//
// Builds with site/build.sh, watches Soil sources, and reloads open pages after
// a successful build. No npm dependencies. No Rust in this checkout.

import { spawn } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, "..");
const dist = path.join(here, "dist");
const bin = process.env.SOIL_LANG || "soil-lang";
const port = Number(process.env.PORT || 8000);
const clients = new Set();
let timer = null;
let building = false;
let pending = false;

const watchDirs = ["site", "core", "std", "examples/ui"]
  .map((dir) => path.join(root, dir))
  .filter((dir) => fs.existsSync(dir));
const generated = new Set([
  path.join(root, "site", "examples-code.soil"),
]);

function broadcast(event, data = "") {
  for (const client of clients) {
    client.write(`event: ${event}\ndata: ${JSON.stringify(data)}\n\n`);
  }
}

function build() {
  if (building) {
    pending = true;
    return;
  }
  building = true;
  let stdout = "";
  let stderr = "";
  const child = spawn("./site/build.sh", {
    cwd: root,
    env: { ...process.env, SOIL_LANG: bin },
  });

  child.stdout.on("data", (chunk) => {
    stdout += chunk;
    process.stdout.write(chunk);
  });
  child.stderr.on("data", (chunk) => {
    stderr += chunk;
    process.stderr.write(chunk);
  });
  child.on("close", (code) => {
    building = false;
    if (code === 0) {
      broadcast("reload");
    } else {
      const error = `${stdout}${stderr}`.trim() || "site build failed";
      broadcast("build-error", error);
    }
    if (pending) {
      pending = false;
      build();
    }
  });
}

function scheduleBuild() {
  clearTimeout(timer);
  timer = setTimeout(build, 180);
}

for (const dir of watchDirs) {
  fs.watch(dir, { recursive: true }, (_event, filename) => {
    if (!filename) {
      scheduleBuild();
      return;
    }
    const changed = path.resolve(dir, filename);
    if (changed === dist || changed.startsWith(`${dist}${path.sep}`)) return;
    if (generated.has(changed)) return;
    if (filename.endsWith(".soil") || filename.endsWith(".md")) scheduleBuild();
  });
}

const types = new Map([
  [".html", "text/html; charset=utf-8"],
  [".css", "text/css; charset=utf-8"],
  [".js", "text/javascript; charset=utf-8"],
  [".mjs", "text/javascript; charset=utf-8"],
  [".md", "text/markdown; charset=utf-8"],
  [".json", "application/json; charset=utf-8"],
  [".xml", "application/xml; charset=utf-8"],
  [".txt", "text/plain; charset=utf-8"],
  [".wasm", "application/wasm"],
]);

const livereload = `<script>
(() => {
  const source = new EventSource("/__reload");
  source.addEventListener("reload", () => location.reload());
  source.addEventListener("build-error", (event) => {
    let node = document.getElementById("__soil_build_error");
    if (!node) {
      node = document.createElement("pre");
      node.id = "__soil_build_error";
      Object.assign(node.style, {
        position: "fixed", inset: "16px", zIndex: "99999", margin: "0",
        padding: "18px", overflow: "auto", color: "#ffd7d7",
        background: "rgba(38, 10, 10, .96)", border: "1px solid #ff7a7a",
        borderRadius: "12px", font: "13px/1.5 ui-monospace, monospace",
      });
      document.body.append(node);
    }
    node.textContent = JSON.parse(event.data);
  });
})();
</script>`;

const server = http.createServer((request, response) => {
  const url = new URL(request.url || "/", "http://127.0.0.1");

  if (url.pathname === "/__reload") {
    response.writeHead(200, {
      "content-type": "text/event-stream",
      "cache-control": "no-store",
      connection: "keep-alive",
    });
    response.write(": connected\n\n");
    clients.add(response);
    request.on("close", () => clients.delete(response));
    return;
  }

  let relative = decodeURIComponent(url.pathname).replace(/^\/+/, "");
  if (!relative || relative.endsWith("/")) relative += "index.html";
  let file = path.resolve(dist, relative);
  if (!file.startsWith(dist)) {
    response.writeHead(403).end("forbidden");
    return;
  }
  if (!fs.existsSync(file) || !fs.statSync(file).isFile()) {
    response.writeHead(404, { "content-type": "text/plain; charset=utf-8" });
    response.end("not found");
    return;
  }

  const ext = path.extname(file);
  response.setHeader("cache-control", "no-store");
  response.setHeader("content-type", types.get(ext) || "application/octet-stream");
  if (ext === ".html") {
    let html = fs.readFileSync(file, "utf8");
    const at = html.lastIndexOf("</body>");
    html = at === -1 ? html + livereload : html.slice(0, at) + livereload + html.slice(at);
    response.end(html);
    return;
  }
  response.end(fs.readFileSync(file));
});

build();
server.listen(port, "127.0.0.1", () => {
  console.log(`soil-lang.org dev: http://127.0.0.1:${port}/`);
  console.log(`watching: ${watchDirs.map((dir) => path.relative(root, dir)).join(", ")}`);
});
