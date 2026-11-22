#!/usr/bin/env node
// Check generated site links and assets. No dependencies.
//
//   node site/check-links.mjs site/dist
//   node site/check-links.mjs --base http://soil-lang.org
//
// Local mode checks every href/src against files in the dist tree. Live mode
// discovers pages from sitemap.xml and fetches every local target.

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
const origin = base ? new URL(base) : null;

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

function localTarget(page, raw) {
  const clean = raw.split("#")[0].split("?")[0];
  if (!clean) return null;
  const baseDir = path.posix.dirname(page.replaceAll("\\", "/"));
  const joined = clean.startsWith("/") ? `dist${clean}` : path.posix.join(baseDir, clean);
  const normalized = path.posix.normalize(joined).replace(/^dist/, "");
  const withIndex = normalized.endsWith("/") ? `${normalized}index.html` : normalized;
  return withIndex || "/index.html";
}

function liveTarget(pageUrl, raw) {
  if (!isLocal(raw)) return null;
  const url = new URL(raw, pageUrl);
  return url.origin === origin.origin ? url.href : null;
}

function collect(html, page, localTargets, liveUrls) {
  const refs = html.matchAll(/\b(?:href|src)="([^"]+)"/g);
  for (const [, raw] of refs) {
    if (!isLocal(raw)) continue;
    if (origin) {
      const target = liveTarget(page, raw);
      if (target) liveUrls.add(target);
    } else {
      const target = localTarget(page, raw);
      if (target) localTargets.add(target);
    }
  }
}

const problems = [];
const localTargets = new Set();
const liveUrls = new Set();
let pageCount = 0;

if (!origin && !fs.existsSync(root)) {
  console.error(`no such directory: ${root}`);
  process.exit(2);
}

if (!origin && fs.existsSync(root)) {
  for (const file of walk(root)) {
    const page = `/${path.relative(root, file).replaceAll("\\", "/")}`;
    const html = fs.readFileSync(file, "utf8");
    pageCount += 1;
    collect(html, page, localTargets, liveUrls);
  }
  for (const target of localTargets) {
    const disk = path.join(root, target);
    if (!fs.existsSync(disk) || !fs.statSync(disk).isFile()) {
      problems.push(`missing ${target}`);
    }
  }
}

if (origin) {
  let sitemap = "";
  try {
    const response = await fetch(new URL("/sitemap.xml", origin));
    if (!response.ok) throw new Error(String(response.status));
    sitemap = await response.text();
  } catch (error) {
    problems.push(`${origin.origin}/sitemap.xml -> ${error.message}`);
  }
  const pages = [...sitemap.matchAll(/<loc>(.*?)<\/loc>/g)].map((match) => {
    const loc = new URL(match[1]);
    return new URL(loc.pathname + loc.search, origin).href;
  });
  for (const page of pages) {
    let response;
    try {
      response = await fetch(page);
    } catch (error) {
      problems.push(`${page} -> ${error.message}`);
      continue;
    }
    if (!response.ok) {
      problems.push(`${page} -> ${response.status}`);
      continue;
    }
    pageCount += 1;
    collect(await response.text(), page, localTargets, liveUrls);
  }

  const queue = [...liveUrls].sort();
  const workers = Array.from({ length: 6 }, async () => {
    while (queue.length) {
      const url = queue.shift();
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
  for (const problem of [...new Set(problems)]) console.error(problem);
  console.error(`${new Set(problems).size} broken link${new Set(problems).size === 1 ? "" : "s"}`);
  process.exit(1);
}
console.log(`checked ${pageCount} pages, ${origin ? liveUrls.size : localTargets.size} targets${origin ? ` at ${origin.origin}` : ""}`);
