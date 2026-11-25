#!/usr/bin/env -S uv run --script
# /// script
# dependencies = ["markdown"]
# ///
# SPDX-License-Identifier: EUPL-1.2 · Copyright (c) 2026 example.org
"""Erzeugt die PDFs der Anleitung und der Matterhorn-Übersicht (DE/EN) aus Markdown.

Aufruf im Projektordner:  uv run docs/build-pdf.py            (alle)
                          uv run docs/build-pdf.py DATEI.md   (nur diese)
Benötigt: uv und Chromium (oder Google Chrome) im PATH, Schrift Carlito empfohlen.
"""
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

import markdown

ROOT = Path(__file__).resolve().parent.parent
DOCS = [
    ("INSTRUCTION.md", "INSTRUCTION_de.pdf", "de", "Mimi – Anleitung"),
    ("INSTRUCTION.en.md", "INSTRUCTION.en.pdf", "en", "Mimi – User Guide"),
    ("MATTERHORN.md", "MATTERHORN_de.pdf", "de", "Mimi und das Matterhorn-Protokoll"),
    ("MATTERHORN.en.md", "MATTERHORN.en.pdf", "en", "Mimi and the Matterhorn Protocol"),
]
# Sprachumschalter „Deutsch · English“ gehört nicht ins PDF
SWITCH = re.compile(r"(?m)^(\*\*Deutsch\*\* · \[English\]|\[Deutsch\]\(.*?\) · \*\*English\*\*).*\n")

CSS = """
@page { size: A4; margin: 20mm 18mm;
  @bottom-center { content: counter(page) " / " counter(pages); font-size: 9pt; color: #5b6470; } }
body { font-family: Carlito, "Noto Sans", "Liberation Sans", sans-serif; font-size: 11pt; line-height: 1.45; color: #1b1f24; }
h1 { font-size: 22pt; color: #1f5fbf; margin: 0 0 8pt; }
h2 { font-size: 15pt; color: #1f5fbf; border-bottom: 1px solid #dde1e6; padding-bottom: 3pt; margin-top: 18pt; break-after: avoid; }
h3 { font-size: 12pt; margin-top: 12pt; break-after: avoid; }
table { border-collapse: collapse; width: 100%; margin: 6pt 0 10pt; font-size: 10pt; }
tr { break-inside: avoid; }
thead { display: table-header-group; }
th, td { border: 1px solid #dde1e6; padding: 4pt 6pt; text-align: left; vertical-align: top; }
th { background: #eef1f5; }
code { font-family: "DejaVu Sans Mono", monospace; font-size: 9.5pt; background: #f1f3f5; padding: 0 2pt; }
blockquote { margin: 8pt 0; padding: 6pt 10pt; background: #fff4d6; border-left: 3pt solid #8a5a00; }
blockquote p { margin: 0; }
hr { border: 0; border-top: 1px solid #dde1e6; margin: 14pt 0; }
a { color: #1f5fbf; text-decoration: none; }
li { margin: 2pt 0; }
img { max-width: 100%; margin: 4pt 0 8pt; }
"""


def browser() -> str:
    for name in ("chromium", "chromium-browser", "google-chrome-stable", "google-chrome"):
        if path := shutil.which(name):
            return path
    sys.exit("Chromium bzw. Google Chrome nicht gefunden.")


def main() -> None:
    exe = browser()
    docs = DOCS
    if len(sys.argv) > 1:
        # einzelne Dateien: uv run docs/build-pdf.py DATEI.md … → DATEI.pdf, Titel = erste Überschrift
        docs = []
        for arg in sys.argv[1:]:
            src = Path(arg).resolve().relative_to(ROOT)
            first = (ROOT / src).read_text(encoding="utf-8").splitlines()[0].lstrip("# ").strip()
            lang = "en" if ".en." in src.name else "de"
            docs.append((str(src), str(src.with_suffix(".pdf")), lang, first))
    with tempfile.TemporaryDirectory() as tmp:
        for src, out, lang, title in docs:
            text = SWITCH.sub("", (ROOT / src).read_text(encoding="utf-8"))
            body = markdown.markdown(text, extensions=["tables", "toc", "sane_lists"])
            html = (
                f'<!doctype html><html lang="{lang}"><head><meta charset="utf-8">'
                f'<base href="{ROOT.as_uri()}/"><title>{title}</title>'
                f"<style>{CSS}</style></head><body>{body}</body></html>"
            )
            page = Path(tmp) / f"{lang}.html"
            page.write_text(html, encoding="utf-8")
            subprocess.run(
                [exe, "--headless", "--disable-gpu", "--no-pdf-header-footer",
                 "--allow-file-access-from-files", f"--print-to-pdf={ROOT / out}", page.as_uri()],
                check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
            )
            print(f"{out} erzeugt")


if __name__ == "__main__":
    main()
