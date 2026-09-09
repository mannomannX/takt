#!/usr/bin/env python3
"""Prueft grammar/takt.ebnf und haelt sie mit Referenz und Inventur in Deckung.

    python grammar/check_grammar.py            Pruefung, Exit 1 bei Fehlern
    python grammar/check_grammar.py --sync     schreibt die Datei in definition.md 2.3

Notation der Grammatikdatei (dieselbe wie in der Referenz, Abschnitt 2.3):
    name := alt | alt        Produktion; Folgezeilen beginnen mit Leerraum
    "x"                      Terminal (Schluesselwort, Operator, Interpunktion)
    NAME                     Token aus dem Tokenizer (Grossbuchstaben)
    [ ]  { }  ( )            optional, Wiederholung, Gruppe
    (* ... *)                Kommentar; "@stage v1.1" markiert die Stufe
"""
from __future__ import annotations

import csv
import io
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
GRAMMAR = ROOT / "grammar" / "takt.ebnf"
REF = ROOT / "plan" / "definition.md"
INVENTORY = ROOT / "plan" / "features.csv"
START = "file"

# Produktion -> Inventur-Kategorie ihrer benannten Alternativen (extract_features.py)
ALTERNATIVES = {
    "system_item": "system-Eintrag", "attr": "Channel-Attribut",
    "scalar_type": "Skalartyp", "type": "Typkonstruktor",
    "elem_type": "Stream-Elementtyp", "simple_stmt": "Statement",
    "seq_item": "Sequenz-Item",
    "capability": "Generics-Fähigkeit", "framing": "Rahmung",
    "binding": "Bindung", "campaign_item": "Kampagnen-Item",
    "gvar": "Generics-Klasse",
    "tprop|tprop_implies|tprop_or|tprop_and|tprop_not|tprop_atom": "Temporaloperator",
}
SYNTAX_NOISE = {"in"}

TOKEN_RE = re.compile(r'"(?:[^"\\]|\\.)*"|[A-Z][A-Z_]*|[a-z_]+|[{}\[\]()|]|:=|\S')


def load():
    text = io.open(GRAMMAR, encoding="utf-8").read()
    tokens_declared, reserved, prods, order = set(), [], {}, []
    current, in_reserved, in_comment = None, False, False
    for raw in text.split("\n"):
        if in_comment:
            in_comment = "*)" not in re.sub(r"\(\*.*?\*\)", "", raw)
            continue
        stage = re.search(r"@stage\s+(v[\d.]+)", raw)
        start = "@start" in raw
        checks = [int(n) for m in re.finditer(r"@check\s+([\d,\s]+)", raw)
                  for n in re.findall(r"\d+", m.group(1))]
        stripped = re.sub(r"\(\*.*?\*\)", " ", raw)
        if "(*" in stripped:
            in_comment = "*)" not in stripped[stripped.index("(*"):]
            stripped = stripped[:stripped.index("(*")]
        if not stripped.strip():
            continue
        m_res = re.match(r"^RESERVED\s*:=", stripped)
        if m_res or (in_reserved and stripped[0].isspace()):
            reserved += re.findall(r'"([^"]+)"', stripped)
            current, in_reserved = None, True
            continue
        in_reserved = False
        m_tok = re.match(r"^([A-Z][A-Z_]*)\s*:=", stripped)
        m_prod = re.match(r"^([a-z_]+)\s*:=\s*(.*)$", stripped)
        if m_tok:
            tokens_declared.add(m_tok.group(1))
            current = None
        elif m_prod:
            current = m_prod.group(1)
            if current in prods:
                sys.exit(f"doppelte Produktion: {current}")
            prods[current] = {"rhs": m_prod.group(2), "stage": "", "start": start, "checks": []}
            order.append(current)
        elif current and stripped[0].isspace():
            prods[current]["rhs"] += " " + stripped.strip()
        else:
            sys.exit(f"unverstaendliche Zeile: {raw!r}")
        if current and stage:
            prods[current]["stage"] = stage.group(1)
        if current and start:
            prods[current]["start"] = True
        if current and checks:
            prods[current]["checks"] += checks
    return text, tokens_declared, reserved, prods, order


def parse_rhs(tokens):
    """Rechte Seite -> Alternativen aus Folgen von Faktoren.

    Faktor: ("term", text) | ("tok", NAME) | ("nt", name) | ("grp", "(" | "[" | "{", alts)
    """
    pos = 0

    def alts(closing):
        nonlocal pos
        result, seq = [], []
        while pos < len(tokens):
            t = tokens[pos]
            if t == "|":
                pos += 1
                result.append(seq)
                seq = []
            elif t in ")]}":
                if t != closing:
                    sys.exit(f"Klammerfehler in Grammatik bei {tokens}")
                pos += 1
                result.append(seq)
                return result
            elif t in "([{":
                pos += 1
                seq.append(("grp", t, alts({"(": ")", "[": "]", "{": "}"}[t])))
            elif t.startswith('"'):
                pos += 1
                seq.append(("term", t[1:-1]))
            elif re.fullmatch(r"[A-Z][A-Z_]*", t):
                pos += 1
                seq.append(("tok", t))
            else:
                pos += 1
                seq.append(("nt", t))
        result.append(seq)
        return result

    return alts(None)


def show(factor):
    if factor[0] == "grp":
        return factor[1] + "…" + {"(": ")", "[": "]", "{": "}"}[factor[1]]
    return factor[1]


def analyse(prods):
    used_nt, used_tok, terminals = {}, set(), {}
    for name, p in prods.items():
        depth = 0
        for t in TOKEN_RE.findall(p["rhs"]):
            if t.startswith('"'):
                terminals.setdefault(t[1:-1], set()).add(name)
            elif re.fullmatch(r"[A-Z][A-Z_]*", t):
                used_tok.add(t)
            elif re.fullmatch(r"[a-z_]+", t):
                used_nt.setdefault(t, set()).add(name)
            elif t in "{[(":
                depth += 1
            elif t in "}])":
                depth -= 1
        if depth != 0:
            yield ("FEHLER", f"unausgeglichene Klammern in {name}")
    for nt, users in used_nt.items():
        if nt not in prods:
            yield ("FEHLER", f"undefiniert: {nt}  (benutzt in {', '.join(sorted(users))})")
    # Ein Block endet mit DEDENT, und der Tokenizer liefert danach kein NEWLINE
    # (lexer.md L2.5). In keiner Folge darf daher auf ein Element, das mit DEDENT
    # enden kann, ueber nullbare Elemente hinweg eines folgen, das mit NEWLINE
    # beginnen kann. Dazu wird die rechte Seite strukturell gelesen.
    trees = {name: parse_rhs(TOKEN_RE.findall(p["rhs"])) for name, p in prods.items()}
    props = {name: {"nullable": False, "starts_nl": False, "ends_dedent": False} for name in prods}

    def prop(factor, key):
        kind = factor[0]
        if kind == "tok":
            return (key == "starts_nl" and factor[1] == "NEWLINE") or (key == "ends_dedent" and factor[1] == "DEDENT")
        if kind == "term":
            return False
        if kind == "nt":
            return props.get(factor[1], {}).get(key, False)
        bracket, alts = factor[1], factor[2]
        if key == "nullable":
            return bracket in "[{" or any(all(prop(f, "nullable") for f in seq) for seq in alts)
        return any(seq_prop(seq, key) for seq in alts)

    def seq_prop(seq, key):
        order = seq if key == "starts_nl" else list(reversed(seq))
        for f in order:
            if prop(f, key):
                return True
            if not prop(f, "nullable"):
                return False
        return False

    changed = True
    while changed:
        changed = False
        for name, alts in trees.items():
            new = {"nullable": any(all(prop(f, "nullable") for f in seq) for seq in alts),
                   "starts_nl": any(seq_prop(seq, "starts_nl") for seq in alts),
                   "ends_dedent": any(seq_prop(seq, "ends_dedent") for seq in alts)}
            if new != props[name]:
                props[name] = new
                changed = True

    def walk(alts, name):
        for seq in alts:
            for i, f in enumerate(seq):
                if f[0] == "grp":
                    yield from walk(f[2], name)
                if not prop(f, "ends_dedent"):
                    continue
                for g in seq[i + 1:]:
                    if prop(g, "starts_nl"):
                        yield ("FEHLER", f"NEWLINE nach moeglichem DEDENT in {name}: nach '{show(f)}' folgt '{show(g)}'")
                        break
                    if not prop(g, "nullable"):
                        break

    for name, alts in trees.items():
        yield from walk(alts, name)
    reach, todo = set(), [START] + [n for n, p in prods.items() if p["start"]]
    while todo:
        n = todo.pop()
        if n in reach or n not in prods:
            continue
        reach.add(n)
        todo += [t for t in TOKEN_RE.findall(prods[n]["rhs"]) if re.fullmatch(r"[a-z_]+", t)]
    for name in prods:
        if name not in reach:
            yield ("FEHLER", f"unerreichbar von {START}: {name}")
    yield ("DATA", (used_tok, terminals))


def keywords_from_reference():
    src = io.open(REF, encoding="utf-8").read()
    block = re.search(r"### 2\.2 Schlüsselwörter\n```\n(.*?)```", src, re.S).group(1)
    kws, res = [], []
    for line in block.split("\n"):
        if line.startswith("reserviert"):
            res = line.split(":", 1)[1].split()
        else:
            kws += line.split()
    return sorted(set(kws)), res


def inventory():
    rows = list(csv.DictReader(io.open(INVENTORY, encoding="utf-8")))
    return rows


def check():
    text, tokens_declared, reserved, prods, order = load()
    findings = []
    used_tok, terminals = set(), {}
    for kind, payload in analyse(prods):
        if kind == "DATA":
            used_tok, terminals = payload
        else:
            findings.append((kind, payload))
    for t in sorted(used_tok - tokens_declared):
        findings.append(("FEHLER", f"Token benutzt, aber nicht deklariert: {t}"))
    for t in sorted(tokens_declared - used_tok):
        findings.append(("WARNUNG", f"Token deklariert, aber unbenutzt: {t}"))

    kws, res_ref = keywords_from_reference()
    word_terms = {t for t in terminals if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", t)}
    for kw in kws:
        if kw not in word_terms:
            findings.append(("FEHLER", f"Schluesselwort aus 2.2 kommt in der Grammatik nicht vor: {kw}"))
    contextual = sorted(word_terms - set(kws))
    if set(reserved) != set(res_ref):
        findings.append(("FEHLER", "reservierte Woerter weichen von 2.2 ab: "
                         f"nur Grammatik {sorted(set(reserved) - set(res_ref))}, "
                         f"nur Referenz {sorted(set(res_ref) - set(reserved))}"))

    rows = inventory()
    g_ids = {r["ID"][2:] for r in rows if r["Kategorie"] == "Grammatik"}
    for name in sorted(set(prods) - g_ids):
        findings.append(("HINWEIS", f"Produktion ohne Inventur-Zeile (extract_features.py laufen lassen): {name}"))
    for name in sorted(g_ids - set(prods)):
        findings.append(("FEHLER", f"Inventur-Zeile ohne Produktion: G-{name}"))
    for prod, kategorie in ALTERNATIVES.items():
        group = set(prod.split("|"))
        want = {r["Bezeichnung"].split('"')[1] for r in rows if r["Kategorie"] == kategorie}
        have = {t for t, users in terminals.items() if users & group
                and re.fullmatch(r"[a-z_][a-z0-9_]*", t) and t not in SYNTAX_NOISE}
        for lit in sorted(want - have):
            findings.append(("FEHLER", f"{kategorie} '{lit}' fehlt in Produktion {prod}"))
        for lit in sorted(have - want):
            findings.append(("HINWEIS", f"{kategorie} '{lit}' neu in {prod}, Inventur nachziehen"))

    sc_ids = {int(r["ID"][3:]) for r in rows if r["Kategorie"] == "Statische Prüfung"}
    anchored = {}
    for name, p in prods.items():
        for n in p["checks"]:
            if n not in sc_ids:
                findings.append(("FEHLER", f"@check {n} in {name}: keine Zeile SC-{n} in der Inventur"))
            anchored.setdefault(n, []).append(name)
    unanchored = sorted(sc_ids - set(anchored))

    errors = [f for f in findings if f[0] == "FEHLER"]
    for kind, msg in findings:
        print(f"{kind:8} {msg}")
    print(f"Anker: {len(anchored)} von {len(sc_ids)} statischen Pruefungen haben eine Produktion; "
          f"ohne Anker (ganzes Programm oder reine Semantik): {' '.join(map(str, unanchored))}")
    print(f"\n{len(prods)} Produktionen, {len(tokens_declared)} Tokens, {len(terminals)} Terminale, "
          f"{len(reserved)} reservierte Woerter, {len(errors)} Fehler")
    print("kontextuelle Terminale (nicht in 2.2, gelten nur an ihrer Stelle): " + " ".join(contextual))
    stages = {name: p["stage"] for name, p in prods.items() if p["stage"]}
    print("Stufen: " + ", ".join(f"{n}={s}" for n, s in stages.items()))
    return 1 if errors else 0


def sync():
    text = io.open(GRAMMAR, encoding="utf-8").read()
    src = io.open(REF, encoding="utf-8").read()
    m = re.search(r"(### 2\.3 Grammatik \(EBNF\).*?```\n)(.*?)(```)", src, re.S)
    if not m:
        sys.exit("Grammatikblock in definition.md nicht gefunden")
    if m.group(2) == text:
        print("definition.md 2.3 ist bereits aktuell")
        return 0
    src = src[:m.start(2)] + text + src[m.end(2):]
    io.open(REF, "w", encoding="utf-8", newline="").write(src)
    print("definition.md 2.3 aus grammar/takt.ebnf aktualisiert")
    return 0


if __name__ == "__main__":
    sys.exit(sync() if "--sync" in sys.argv else check())
