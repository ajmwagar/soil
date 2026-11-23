#!/usr/bin/env node
// Build one Soil Spore component into the JS + core wasm modules a Worker can
// load. The generated tree is intentionally not committed.
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const modulePath = process.argv[2];
const name = process.argv[3] ?? "spore";

if (!modulePath) {
  console.error("usage: node build.mjs /path/to/spore/module.wasm NAME");
  process.exit(2);
}

const out = path.join(here, "spore");
fs.rmSync(out, { recursive: true, force: true });
fs.mkdirSync(out, { recursive: true });

execFileSync(
  "jco",
  [
    "transpile",
    modulePath,
    "-o",
    out,
    "--name",
    name,
    "--no-typescript",
    "--no-wasi-shim",
    "--base64-cutoff",
    "0",
    "--tla-compat",
  ],
  { stdio: "inherit" },
);

const jsPath = path.join(out, `${name}.js`);
let js = fs.readFileSync(jsPath, "utf8");

// jco's default loader compiles core modules with WebAssembly.compile() or
// fetch() at module initialization. workerd does not settle that path before
// the first request. Import the same modules through Wrangler's CompiledWasm
// rule instead, so they arrive already compiled.
const core = fs
  .readdirSync(out)
  .filter((file) => file.endsWith(".wasm"))
  .sort();
const imports = core.map(
  (file, index) => `import SPORE_CORE_${index} from "./${file}";`,
);
for (const [index, file] of core.entries()) {
  js = js.replace(
    `const module${index} = fetchCompile(new URL('./${file}', import.meta.url));`,
    `const module${index} = SPORE_CORE_${index};`,
  );
}

// jco emits one bare import per WASI interface. The spike supplies one local
// shim module instead; anything that actually performs WASI I/O must replace
// the relevant stub with a real implementation.
const wasiSpecifiers = new Set();
js = js.replace(
  /^import\s*\{([^}]*)\}\s*from\s*"wasi:[^"]+";\s*$/gm,
  (_, specifiers) => {
    for (const specifier of specifiers.split(",")) {
      const trimmed = specifier.trim();
      if (trimmed) wasiSpecifiers.add(trimmed);
    }
    return "";
  },
);
if (/^import .* from "(?!\.)/m.test(js)) {
  throw new Error(
    "this Spore imports a non-WASI host interface; add a Worker shim before building",
  );
}

fs.copyFileSync(
  path.join(here, "stubs", "wasi-stubs.js"),
  path.join(out, "wasi-stubs.js"),
);
const wasiImport = wasiSpecifiers.size
  ? `import { ${[...wasiSpecifiers].join(", ")} } from "./wasi-stubs.js";\n`
  : "";
js = imports.join("\n") + "\n" + wasiImport + js;
fs.writeFileSync(jsPath, js);

console.log(`built ${path.join(out, `${name}.js`)}`);
