#!/usr/bin/env python3
"""Prueft plan/mir_map.csv: die Abbildung der Inventur auf die MIR (plan/mir.md, Abschnitt 3).

    python plan/check_mir_map.py        Exit 1 bei Fehlern

Jede SEM-, G-, PAR-, RULE-, FN-, DESUGAR- und MEM-Zeile der Inventur hat genau eine Zeile in
mir_map.csv mit einem Ziel:

    Knoten     Name ist ein `pub struct`, `pub enum` oder `Enum::Variante` in crates/takt-mir/src
    Lowering   wie Knoten: der Knoten, den das Lowering aus dem Konstrukt erzeugt
    Regel      Name ist `modul::funktion` in crates/takt-mir/src
    Prüfung    Name ist eine SC-Zeile der Inventur
    Runtime    Name ist eine Komponente aus Referenz 11.1
    Frontend   Name ist takt-syntax oder takt-sema
    Rationale  ohne Name
"""
from __future__ import annotations

import csv
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
INVENTORY = ROOT / "plan" / "features.csv"
MAP = ROOT / "plan" / "mir_map.csv"
SRC = ROOT / "crates" / "takt-mir" / "src"
REF = ROOT / "plan" / "definition.md"
FAMILIES = {"SEM", "G", "PAR", "RULE", "FN", "DESUGAR", "MEM"}
FRONTEND = {"takt-syntax", "takt-sema"}


def rust_names() -> tuple[set[str], set[str]]:
    """Typen (`Struct`, `Enum`, `Enum::Variante`) und Funktionen (`modul::fn`)."""
    types: set[str] = set()
    fns: set[str] = set()
    for path in SRC.rglob("*.rs"):
        module = path.stem if path.stem != "mod" else path.parent.name
        text = path.read_text(encoding="utf-8")
        for m in re.finditer(r"^pub struct (\w+)", text, re.M):
            types.add(m.group(1))
        for m in re.finditer(r"^pub enum (\w+) \{\n(.*?)^\}", text, re.M | re.S):
            name = m.group(1)
            types.add(name)
            for v in re.finditer(r"^    ([A-Z]\w*)", m.group(2), re.M):
                types.add(f"{name}::{v.group(1)}")
        for m in re.finditer(r"^\s*(?:pub(?:\(crate\))? )?fn (\w+)", text, re.M):
            fns.add(f"{module}::{m.group(1)}")
    return types, fns


def components() -> set[str]:
    text = REF.read_text(encoding="utf-8")
    start = text.index("### 11.1 Komponenten")
    block = text[start:text.index("### 11.2", start)]
    return set(re.findall(r"^(takt-[\w-]+|libtaktm)\s", block, re.M))


def main() -> int:
    inventory = list(csv.DictReader(INVENTORY.open(encoding="utf-8", newline="")))
    wanted = [r["ID"] for r in inventory if r["ID"].split("-")[0] in FAMILIES]
    checks = {r["ID"] for r in inventory if r["ID"].startswith("SC-")}
    rows = list(csv.DictReader(MAP.open(encoding="utf-8", newline="")))
    types, fns = rust_names()
    comps = components()
    errors: list[str] = []
    seen: dict[str, int] = {}
    for r in rows:
        seen[r["ID"]] = seen.get(r["ID"], 0) + 1
        ziel, name = r["Ziel"], r["Name"]
        if ziel in ("Knoten", "Lowering"):
            if name not in types:
                errors.append(f"{r['ID']}: Knoten {name} existiert nicht in crates/takt-mir/src")
        elif ziel == "Regel":
            if name not in fns:
                errors.append(f"{r['ID']}: Regel {name} ist keine Funktion in crates/takt-mir/src")
        elif ziel == "Prüfung":
            if name not in checks:
                errors.append(f"{r['ID']}: Prüfung {name} ist keine SC-Zeile der Inventur")
        elif ziel == "Runtime":
            if name not in comps:
                errors.append(f"{r['ID']}: Komponente {name} steht nicht in 11.1")
        elif ziel == "Frontend":
            if name not in FRONTEND:
                errors.append(f"{r['ID']}: Frontend {name} unbekannt")
        elif ziel == "Rationale":
            if name:
                errors.append(f"{r['ID']}: Rationale mit Name")
        else:
            errors.append(f"{r['ID']}: unbekanntes Ziel {ziel}")
    for i in wanted:
        if i not in seen:
            errors.append(f"{i}: keine Zeile in mir_map.csv")
    for i, n in seen.items():
        if n > 1:
            errors.append(f"{i}: {n} Zeilen")
        if i not in wanted:
            errors.append(f"{i}: nicht (mehr) in der Inventur")
    for e in errors:
        print(e)
    by_target: dict[str, int] = {}
    for r in rows:
        by_target[r["Ziel"]] = by_target.get(r["Ziel"], 0) + 1
    summary = ", ".join(f"{k} {v}" for k, v in sorted(by_target.items()))
    print(f"{len(rows)} Zeilen ({summary}); {len(errors)} Fehler")
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
