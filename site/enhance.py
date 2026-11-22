#!/usr/bin/env python3
"""Inject per-page canonical, description, and social metadata."""
import html
import pathlib
import re

ROOT = pathlib.Path("site/dist")
BASE = "https://soil-lang.org"
IMAGE = f"{BASE}/og.png"


def clean_text(fragment):
    text = re.sub(r"<[^>]+>", " ", fragment)
    text = html.unescape(text)
    return re.sub(r"\s+", " ", text).strip()


def description_for(text, title):
    for pattern in (
        r'<p class="lede"[^>]*>(.*?)</p>',
        r'<p class="docs-lede"[^>]*>(.*?)</p>',
        r'<p class="body"[^>]*>(.*?)</p>',
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
        f'<meta property="og:title" content="{html.escape(title, quote=True)}">',
        f'<meta property="og:description" content="{html.escape(description, quote=True)}">',
        f'<meta property="og:url" content="{html.escape(canonical, quote=True)}">',
        f'<meta property="og:image" content="{IMAGE}">',
        '<meta name="twitter:card" content="summary_large_image">',
        f'<meta name="twitter:title" content="{html.escape(title, quote=True)}">',
        f'<meta name="twitter:description" content="{html.escape(description, quote=True)}">',
        f'<meta name="twitter:image" content="{IMAGE}">',
    ])
    text = text.replace("</head>", tags + "\n</head>", 1)
    file.write_text(text)
print("wrote per-page metadata")
