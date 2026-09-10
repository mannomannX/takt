#!/usr/bin/env python3
"""Bringt die Codebloecke der Referenz in die kanonische Form (grammar/format.md).

    python grammar/format_examples.py            schreibt plan/definition.md um
    python grammar/format_examples.py --check    meldet nur, welche Bloecke abweichen

Die Bloecke sind dieselben wie in extract_snippets.py (Ausschlussliste dort);
jeder wird als Schnipsel durch den Formatter geschickt und mit seiner
Markdown-Einrueckung zurueckgeschrieben. Danach corpus-try/ref neu erzeugen:
python grammar/extract_snippets.py
"""
from __future__ import annotations

import io
import re
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import extract_snippets as ex  # noqa: E402

BLOCK_RE = re.compile(r"^(### ([\d.]+) [^\n]*|## (\d+)\.[^\n]*|```[a-z]*\n(.*?)```)", re.S | re.M)


def blocks(src):
    """(Start, Ende, Einrueckung, Text ohne Einrueckung) je formatierbarem Block."""
    section, counter, out = "0", {}, []
    for m in BLOCK_RE.finditer(src):
        if m.group(2):
            section = m.group(2)
            continue
        if m.group(3):
            section = m.group(3)
            continue
        counter[section] = counter.get(section, 0) + 1
        n = counter[section]
        if ex.EXCLUDE.get((section, n)) or section.startswith("9."):
            continue
        lines = m.group(4).split("\n")
        nonblank = [l for l in lines if l.strip()]
        indent = min(len(l) - len(l.lstrip(" ")) for l in nonblank) if nonblank else 0
        text = "\n".join(l[indent:] if l.strip() else "" for l in lines).rstrip("\n") + "\n"
        out.append((m.start(4), m.end(4), indent, text))
    return out


def main(argv):
    check = "--check" in argv
    src = io.open(ex.REF, encoding="utf-8").read()
    found = blocks(src)
    with tempfile.TemporaryDirectory(prefix="takt-ref-") as work:
        paths = []
        for i, (_, _, _, text) in enumerate(found):
            p = Path(work) / f"{i:03d}.takt"
            io.open(p, "w", encoding="utf-8", newline="\n").write(text)
            paths.append(str(p))
        r = subprocess.run(["cargo", "run", "-q", "-p", "takt-syntax", "--example", "fmt", "--", "--snippet"] + paths,
                           capture_output=True, text=True, encoding="utf-8", cwd=ex.ROOT)
        if r.returncode != 0:
            print(r.stderr or r.stdout)
            return 1
        formatted = [io.open(p, encoding="utf-8").read() for p in paths]
    changed = 0
    out = []
    pos = 0
    for (start, end, indent, text), new in zip(found, formatted):
        if new != text:
            changed += 1
        body = "".join((" " * indent + l if l else "") + "\n" for l in new.splitlines())
        out.append(src[pos:start])
        out.append(body)
        pos = end
    out.append(src[pos:])
    result = "".join(out)
    if check:
        print(f"{len(found)} Bloecke, {changed} nicht kanonisch")
        return 1 if changed else 0
    if result != src:
        io.open(ex.REF, "w", encoding="utf-8", newline="\n").write(result)
    print(f"{len(found)} Bloecke, {changed} umgeschrieben")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
