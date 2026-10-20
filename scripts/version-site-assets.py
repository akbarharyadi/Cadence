"""Adds ?v=<content hash> to the website's asset links so Cloudflare and browsers fetch changed files.

Run after changing anything in site/ (images, style.css, main.js): python scripts/version-site-assets.py
"""
import hashlib
import pathlib
import re

site = pathlib.Path(__file__).resolve().parent.parent / "site"
html = site / "index.html"
text = html.read_text(encoding="utf-8")


def versioned(match: re.Match) -> str:
    path = match.group(2)
    file = site / path.lstrip("/")
    if not file.is_file():
        return match.group(0)
    digest = hashlib.sha256(file.read_bytes()).hexdigest()[:8]
    return f'{match.group(1)}{path}?v={digest}"'


# src="/assets/x.jpg", href="/style.css", content="https://.../assets/home.jpg" (with or without an old ?v=).
text = re.sub(r'((?:src|href|content)="(?:https://cadence\.akbarharyadi\.com)?)(/[\w./-]+\.(?:jpg|png|svg|css|js))(?:\?v=\w+)?"', versioned, text)
html.write_text(text, encoding="utf-8", newline="\n")
print("\n".join(sorted(set(re.findall(r'/[\w./-]+\?v=\w+', text)))))
