#!/usr/bin/env python3
"""Inject per-page canonical, description, and social metadata."""
import html
import pathlib
import re

ROOT = pathlib.Path("site/dist")
BASE = "https://soil-lang.org"
IMAGE = f"{BASE}/og.png"
IMAGE_WIDTH = 1200
IMAGE_HEIGHT = 630
IMAGE_ALT = "Soil — a Lisp for WebAssembly."


def clean_text(fragment):
    text = re.sub(r"<[^>]+>", " ", fragment)
    text = html.unescape(text)
    return re.sub(r"\s+", " ", text).strip()


def description_for(text, title):
    for pattern in (
        r'<p class="lede"[^>]*>(.*?)</p>',
        r'<p class="docs-lede"[^>]*>(.*?)</p>',
        r'<p class="body"[^>]*>(.*?)</p>',
        r'<p class="book-lede"[^>]*>(.*?)</p>',
        r'<p class="demo-lede"[^>]*>(.*?)</p>',
        r'<p class="statement"[^>]*>(.*?)</p>',
    ):
        match = re.search(pattern, text, re.S)
        if match:
            value = clean_text(match.group(1))
            if value:
                return value[:180]
    return title


def page_path(relative):
    if relative == "index.html":
        return "/"
    if relative.endswith("/index.html"):
        return "/" + relative[: -len("index.html")]
    return "/" + relative


for file in sorted(ROOT.rglob("*.html")):
    text = file.read_text()
    if 'rel="canonical"' in text:
        continue
    relative = file.relative_to(ROOT).as_posix()
    path = page_path(relative)
    canonical = BASE + path
    title_match = re.search(r"<title>(.*?)</title>", text, re.S)
    title = clean_text(title_match.group(1)) if title_match else "Soil"
    description = description_for(text, title)
    tags = "".join([
        '<link rel="icon" href="/favicon.svg" type="image/svg+xml">',
        f'<link rel="canonical" href="{html.escape(canonical, quote=True)}">',
        f'<meta name="description" content="{html.escape(description, quote=True)}">',
        '<meta name="theme-color" content="#14120f">',
        '<meta property="og:type" content="website">',
        '<meta property="og:site_name" content="Soil">',
        f'<meta property="og:title" content="{html.escape(title, quote=True)}">',
        f'<meta property="og:description" content="{html.escape(description, quote=True)}">',
        f'<meta property="og:url" content="{html.escape(canonical, quote=True)}">',
        f'<meta property="og:image" content="{IMAGE}">',
        f'<meta property="og:image:secure_url" content="{IMAGE}">',
        '<meta property="og:image:type" content="image/png">',
        f'<meta property="og:image:width" content="{IMAGE_WIDTH}">',
        f'<meta property="og:image:height" content="{IMAGE_HEIGHT}">',
        f'<meta property="og:image:alt" content="{html.escape(IMAGE_ALT, quote=True)}">',
        '<meta name="twitter:card" content="summary_large_image">',
        f'<meta name="twitter:title" content="{html.escape(title, quote=True)}">',
        f'<meta name="twitter:description" content="{html.escape(description, quote=True)}">',
        f'<meta name="twitter:image" content="{IMAGE}">',
        f'<meta name="twitter:image:alt" content="{html.escape(IMAGE_ALT, quote=True)}">',
    ])
    text = text.replace("</head>", tags + "\n</head>", 1)
    file.write_text(text)
print("wrote per-page metadata")
