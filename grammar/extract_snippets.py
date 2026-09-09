#!/usr/bin/env python3
"""Zieht alle Codebloecke aus plan/definition.md als Schnipsel in ein Verzeichnis.

    python grammar/extract_snippets.py [ZIEL]      Default: corpus-try/ref

Jeder Block wird auf seine kleinste Einrueckung zurueckgesetzt (Bloecke in
Markdown-Listen sind um 2 eingerueckt) und als <abschnitt>_<nr>.takt abgelegt.
Bloecke, die keine Takt-Syntax sind (EBNF, Semantik-Pseudocode, Rust-Skizzen,
Tabellen in ASCII), stehen in EXCLUDE und werden mit Grund uebersprungen; das
Manifest listet alle Bloecke mit Status.
"""
from __future__ import annotations

import io
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
REF = ROOT / "plan" / "definition.md"

# (Abschnitt, laufende Nummer im Abschnitt) -> Grund
EXCLUDE = {
    ("2.2", 1): "Schluesselwortliste",
    ("2.3", 1): "EBNF",
    ("3.11", 2): "Typregeln der dimensionierten Matrizen (Prosa in ASCII)",
    ("3.12", 1): "Signaturen ohne Rumpf (Syntaxschau; Korpus 08 und 09 decken sie ab)",
    ("5.11", 2): "Semantik der gescopten Instanzen (Pseudocode)",
    ("5.12", 2): "Semantik von resume (Pseudocode)",
    ("6.3", 2): "Desugaring-Ergebnis mit Pseudonotation -> [Fault ...]",
    ("7.5", 2): "Semantik geplanter Ausgaben (Pseudocode)",
    ("7.5", 3): "Latenzformel",
    ("8.6", 2): "Semantik der Stroeme (Pseudocode)",
    ("8.7", 2): "Operatorschema mit Platzhalter P",
    ("8.7", 4): "Dispatch (Pseudocode)",
    ("9.0", 1): "Notation der Semantik",
    ("11.1", 1): "Crate-Liste",
    ("11.2", 1): "Rust-Skizze",
    ("11.4", 1): "Bibliothekssignaturen in Kurzschreibweise",
    ("12.1", 1): "Runtime-Schleife (Pseudocode)",
    ("13.5", 2): "Verdikt-Halbverband (Prosa in ASCII)",
    ("13.8", 1): "Kommandozeile",
}


def main(argv):
    out = Path(argv[0]) if argv else ROOT / "corpus-try" / "ref"
    out.mkdir(parents=True, exist_ok=True)
    src = io.open(REF, encoding="utf-8").read()
    section = "0"
    counter = {}
    rows = []
    pos = 0
    for m in re.finditer(r"^(### ([\d.]+) [^\n]*|## (\d+)\.[^\n]*|```[a-z]*\n(.*?)```)", src, re.S | re.M):
        if m.group(2):
            section = m.group(2)
            continue
        if m.group(3):
            section = m.group(3)
            continue
        block = m.group(4)
        counter[section] = counter.get(section, 0) + 1
        n = counter[section]
        lines = [l for l in block.split("\n")]
        nonblank = [l for l in lines if l.strip()]
        indent = min(len(l) - len(l.lstrip(" ")) for l in nonblank) if nonblank else 0
        text = "\n".join(l[indent:] if l.strip() else "" for l in lines).rstrip("\n") + "\n"
        name = f"{section.replace('.', '_')}_{n:02d}.takt"
        reason = EXCLUDE.get((section, n))
        if section.startswith("9.") and not reason:
            reason = "formale Semantik (Pseudocode)"
        if reason:
            rows.append((name, section, "ausgeschlossen", reason))
            continue
        io.open(out / name, "w", encoding="utf-8", newline="").write(text)
        rows.append((name, section, "Schnipsel", nonblank[0].strip()[:60] if nonblank else ""))
    with io.open(out / "manifest.csv", "w", encoding="utf-8", newline="") as fh:
        fh.write("Datei,Abschnitt,Status,Bemerkung\n")
        for name, sec, status, note in rows:
            fh.write(f'{name},{sec},{status},"{note}"\n')
    kept = sum(1 for r in rows if r[2] == "Schnipsel")
    print(f"{len(rows)} Bloecke, {kept} Schnipsel nach {out}, {len(rows) - kept} ausgeschlossen")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
