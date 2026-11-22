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
import os from "node:os";
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
const docsFiles = new Set([
  path.join(root, "site", "docs-copy.soil"),
  path.join(root, "site", "docs.soil"),
  path.join(root, "site", "docs.sh"),
]);
const docsRoots = [path.join(root, "core"), path.join(root, "std")];
let docsVersion = 0;
let docsBuiltVersion = -1;

function touchesDocs(file) {
  if (docsFiles.has(file)) return true;
  return docsRoots.some((dir) => file === dir || file.startsWith(`${dir}${path.sep}`));
}

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
  const withDocs = docsVersion !== docsBuiltVersion;
  const buildDocsVersion = docsVersion;
  const child = spawn("./site/build.sh", {
    cwd: root,
    env: {
      ...process.env,
      SOIL_LANG: bin,
      SOIL_DOCS: withDocs ? "1" : "0",
    },
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
      if (withDocs) docsBuiltVersion = buildDocsVersion;
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
    if (filename.endsWith(".soil") || filename.endsWith(".md")) {
      if (touchesDocs(changed)) docsVersion += 1;
      scheduleBuild();
    }
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
  [".svg", "image/svg+xml"],
  [".png", "image/png"],
  [".jpg", "image/jpeg"],
  [".jpeg", "image/jpeg"],
  [".webp", "image/webp"],
  [".ico", "image/x-icon"],
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


const exampleNames = new Set(["counter", "todo", "calculator", "tictactoe", "tabs", "form"]);

function readBody(request) {
  return new Promise((resolve, reject) => {
    let body = "";
    request.setEncoding("utf8");
    request.on("data", (chunk) => {
      body += chunk;
      if (body.length > 65536) {
        reject(new Error("request body too large"));
        request.destroy();
      }
    });
    request.on("end", () => resolve(body));
    request.on("error", reject);
  });
}

function sendJson(response, status, value) {
  response.writeHead(status, {
    "content-type": "application/json; charset=utf-8",
    "cache-control": "no-store",
  });
  response.end(JSON.stringify(value));
}

async function runExample(request, response) {
  let data;
  try {
    data = JSON.parse(await readBody(request));
  } catch (error) {
    sendJson(response, 400, { error: String(error.message || error) });
    return;
  }

  const name = String(data.name || "");
  if (!exampleNames.has(name)) {
    sendJson(response, 404, { error: "unknown example" });
    return;
  }

  const state = typeof data.state === "string" ? data.state : JSON.stringify(data.state);
  const action = data.action && typeof data.action === "object" ? data.action : { op: "noop" };
  const actionJson = JSON.stringify(action);
  const stateLiteral = JSON.stringify(state);
  const actionLiteral = JSON.stringify(actionJson);
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "soil-example-"));
  const wrapper = path.join(tmp, "main.soil");
  fs.writeFileSync(wrapper, `(defun main ()
  (let ((state (json-decode ${stateLiteral})))
    (let ((next (update state ${actionLiteral})))
      (println (json-encode (list (list :state next) (list :html (render-html (view next)))))))))
`);

  let stdout = "";
  let stderr = "";
  const child = spawn(
    bin,
    [
      "--call", "main",
      "--prelude", "core/prelude.soil",
      "core/ui.soil",
      "examples/ui/prelude.soil",
      `examples/ui/${name}.soil`,
      wrapper,
    ],
    { cwd: root, env: { ...process.env }, timeout: 3000 },
  );
  child.stdout.on("data", (chunk) => { stdout += chunk; });
  child.stderr.on("data", (chunk) => { stderr += chunk; });
  child.on("close", (code) => {
    fs.rmSync(tmp, { recursive: true, force: true });
    if (code !== 0 || stderr.trim()) {
      sendJson(response, 500, { error: stderr.trim() || "example failed" });
      return;
    }
    try {
      sendJson(response, 200, JSON.parse(stdout.trim()));
    } catch (error) {
      sendJson(response, 500, { error: "example returned invalid JSON" });
    }
  });
}

const server = http.createServer((request, response) => {
  const url = new URL(request.url || "/", "http://127.0.0.1");

  if (url.pathname === "/__example" && request.method === "POST") {
    runExample(request, response);
    return;
  }

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
