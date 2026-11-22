#!/usr/bin/env node
// Check generated site links and assets. No dependencies.
//
//   node site/check-links.mjs site/dist
//   node site/check-links.mjs --base http://soil-lang.org site/dist
//
// In local mode every root-relative/relative href and src must resolve to a
// file. In live mode the same paths are fetched from --base.

import fs from "node:fs";
import path from "node:path";

const args = process.argv.slice(2);
let base = null;
let rootArg = "site/dist";
for (let i = 0; i < args.length; i += 1) {
  if (args[i] === "--base") base = args[++i];
  else rootArg = args[i];
}
const root = path.resolve(rootArg);
if (!fs.existsSync(root)) {
  console.error(`no such directory: ${root}`);
  process.exit(2);
}

function walk(dir, out = []) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) walk(full, out);
    else if (entry.name.endsWith(".html")) out.push(full);
  }
  return out;
}

function isLocal(raw) {
  const value = raw.trim();
  return value &&
    !value.startsWith("#") &&
    !value.startsWith("//") &&
    !value.startsWith("data:") &&
    !value.startsWith("mailto:") &&
    !value.startsWith("tel:") &&
    !/^[a-z][a-z0-9+.-]*:/i.test(value);
}

function resolveTarget(page, raw) {
  const clean = raw.split("#")[0].split("?")[0];
  if (!clean) return null;
  const baseDir = path.posix.dirname(page.replaceAll("\\", "/"));
  const joined = clean.startsWith("/") ? `dist${clean}` : path.posix.join(baseDir, clean);
  const normalized = path.posix.normalize(joined).replace(/^dist/, "");
  const withIndex = normalized.endsWith("/") ? `${normalized}index.html` : normalized;
  return withIndex || "/index.html";
}

const problems = [];
const urls = new Set();
const pages = walk(root);
for (const file of pages) {
  const page = `/${path.relative(root, file).replaceAll("\\", "/")}`;
  const html = fs.readFileSync(file, "utf8");
  const refs = html.matchAll(/\b(?:href|src)="([^"]+)"/g);
  for (const [, raw] of refs) {
    if (!isLocal(raw)) continue;
    const target = resolveTarget(page, raw);
    if (!target) continue;
    urls.add(target);
    if (!base) {
      const disk = path.join(root, target);
      if (!fs.existsSync(disk) || !fs.statSync(disk).isFile()) {
        problems.push(`${page} -> ${raw} (missing ${target})`);
      }
    }
  }
}

if (base) {
  const origin = new URL(base);
  const queue = [...urls].sort();
  const workers = Array.from({ length: 6 }, async () => {
    while (queue.length) {
      const target = queue.shift();
      const url = new URL(target, origin).href;
      let response;
      try {
        response = await fetch(url, { redirect: "follow" });
      } catch (error) {
        problems.push(`${url} -> ${error.message}`);
        continue;
      }
      if (!response.ok) problems.push(`${url} -> ${response.status}`);
    }
  });
  await Promise.all(workers);
}

if (problems.length) {
  for (const problem of problems) console.error(problem);
  console.error(`${problems.length} broken link${problems.length === 1 ? "" : "s"}`);
  process.exit(1);
}
console.log(`checked ${pages.length} pages, ${urls.size} local targets${base ? ` at ${base}` : ""}`);
