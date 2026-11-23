#!/usr/bin/env sh
# Generate the API docs from Soil `;;;` doc comments.
#
# The page catalog (order, titles, summaries) lives in docs-copy.soil. The
# entries on each page come from the `;;;` comments in the core/std source.
# Extraction runs in Soil. The evaluator is recursive and this machine's stack
# is small, so the wrapper feeds it fixed-size source windows and joins the
# entries; projection to Markdown, llms.txt, and HTML stays in docs.soil.
set -eu
cd "$(dirname "$0")/.."

BIN=${SOIL_LANG:-}
if [ -z "$BIN" ]; then
  if command -v soil-lang >/dev/null 2>&1; then
    BIN=$(command -v soil-lang)
  fi
fi
if [ ! -x "$BIN" ]; then
  echo "set SOIL_LANG to a compatible soil-lang binary" >&2
  exit 1
fi

mkdir -p site/dist/docs
BIN="$BIN" python3 - <<'PY'
import json
import os
import subprocess
import sys
import tempfile

root = os.getcwd()
bin_path = os.environ["BIN"]
prelude = [
    "core/prelude.soil",
    "core/ui.soil",
    "core/highlight.soil",
    "core/doc.soil",
    "site/docs-copy.soil",
    "site/doc-examples.soil",
]

def top_level_units(text):
    lines = text.splitlines()
    units = []
    index = 0
    while index < len(lines):
        pending = []
        while index < len(lines):
            stripped = lines[index].strip()
            if stripped == "" or stripped.startswith(";"):
                pending.append(lines[index])
                index += 1
                continue
            break
        if index >= len(lines):
            if pending:
                units.append("\n".join(pending))
            break
        start = index
        depth = 0
        in_string = False
        escaped = False
        while index < len(lines):
            line = lines[index]
            comment = False
            for character in line:
                if comment:
                    break
                if in_string:
                    if escaped:
                        escaped = False
                    elif character == "\\":
                        escaped = True
                    elif character == "\"":
                        in_string = False
                else:
                    if character == ";":
                        comment = True
                    elif character == "\"":
                        in_string = True
                    elif character == "(":
                        depth += 1
                    elif character == ")":
                        depth -= 1
            index += 1
            if depth <= 0:
                break
        units.append("\n".join(pending + lines[start:index]))
    return units

def chunks(text, limit=18):
    # Never split a top-level form: a truncated source block is worse than a
    # slightly larger evaluator chunk.
    out = []
    current = []
    count = 0
    for unit in top_level_units(text):
        size = unit.count("\n") + 1
        if current and count + size > limit:
            out.append("\n".join(current))
            current = []
            count = 0
        current.append(unit)
        count += size
    if current:
        out.append("\n".join(current))
    return out

def run_soil(extra):
    command = [bin_path] + prelude + extra
    proc = subprocess.run(command, cwd=root, capture_output=True, text=True)
    if proc.returncode != 0:
        sys.stderr.write(proc.stderr)
        raise SystemExit(proc.returncode)
    value = proc.stdout.strip()
    if not value:
        return None
    return json.loads(json.loads(value))

def catalog():
    with tempfile.TemporaryDirectory() as tmp:
        app = os.path.join(tmp, "catalog.soil")
        with open(app, "w") as handle:
            handle.write("(defun main () (json-encode docs-copy))\n")
        return run_soil([app])

docs_copy = catalog()
sources = [
    (page["slug"], page["title"], page["summary"])
    for page in docs_copy["pages"]
]

def extract(chunk):
    with tempfile.TemporaryDirectory() as tmp:
        source = os.path.join(tmp, "source.soil")
        app = os.path.join(tmp, "app.soil")
        with open(source, "w") as handle:
            handle.write("(defun soil-doc-chunk () (json-decode ")
            handle.write(json.dumps(json.dumps(chunk)))
            handle.write("))\n")
        with open(app, "w") as handle:
            handle.write("(defun main () (json-encode (doc-extract-public (soil-doc-chunk))))\n")
        return run_soil([source, app]) or []

pages = []
def source_path(slug):
    for directory in ("core", "std"):
        for dirpath, _, filenames in os.walk(os.path.join(root, directory)):
            path = os.path.join(dirpath, slug + ".soil")
            if slug + ".soil" in filenames and os.path.exists(path):
                return path
    raise SystemExit("no source for %s" % slug)

for slug, title, summary in sources:
    with open(source_path(slug)) as handle:
        source = handle.read()
    entries = []
    for chunk in chunks(source):
        entries.extend(extract(chunk))
    pages.append({
        "slug": slug,
        "title": title,
        "summary": summary,
        "entries": entries,
    })

with tempfile.TemporaryDirectory() as tmp:
    source = os.path.join(tmp, "pages.soil")
    with open(source, "w") as handle:
        handle.write("(defun soil-doc-pages () (json-decode ")
        handle.write(json.dumps(json.dumps(pages)))
        handle.write("))\n")
    files = run_soil([source, "site/docs.soil"])
    if not isinstance(files, list):
        raise SystemExit("docs.soil did not return a file list")
    for item in files:
        path = os.path.join(root, "site/dist", item["path"])
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w") as handle:
            handle.write(item["text"])

# Andes manifest. Andes lives in its own repository; tend can consume it as a
# git dependency and point `repoRoot` at this checkout.
manifest = {
    "name": docs_copy["name"],
    "description": docs_copy["description"],
    "navigation": [{
        "group": docs_copy["nav-group"],
        "audience": docs_copy["audience"],
        "pages": [
            {
                "title": "Overview",
                "slug": "api",
                "file": "site/dist/docs/index.md",
                "description": "Generated API documentation.",
            }
        ] + [
            {
                "title": title,
                "slug": slug,
                "file": "site/dist/docs/%s.md" % slug,
                "description": summary,
            }
            for slug, title, summary in sources
        ],
    }],
}
with open(os.path.join(root, "site/dist/docs.json"), "w") as handle:
    json.dump(manifest, handle, indent=2)
    handle.write("\n")

# Routes crawlers expect. The llms.txt index is generated in Soil above.
base = "https://soil-lang.org"
urls = ["/", "/getting-started.html", "/lisp-101.html", "/demos.html", "/docs/"] + [
    "/docs/%s.html" % slug for slug, _, _ in sources
]
sitemap = [
    '<?xml version="1.0" encoding="UTF-8"?>',
    '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">',
]
for url in urls:
    sitemap.append("  <url><loc>%s%s</loc></url>" % (base, url))
sitemap.append("</urlset>")
with open(os.path.join(root, "site/dist/sitemap.xml"), "w") as handle:
    handle.write("\n".join(sitemap) + "\n")
with open(os.path.join(root, "site/dist/robots.txt"), "w") as handle:
    handle.write("User-agent: *\nAllow: /\n\n")
    handle.write("Sitemap: %s/sitemap.xml\n" % base)
    handle.write("# LLM index: %s/llms.txt\n" % base)

print("wrote site/dist/docs/, llms.txt, llms-full.txt, sitemap.xml, robots.txt")
PY
