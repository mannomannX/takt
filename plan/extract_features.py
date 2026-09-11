#!/usr/bin/env python3
"""Erzeugt plan/features.csv aus plan/definition.md.

Die Referenz ist die Quelle der Wahrheit; die Inventur ist ihr Erzeugnis
(plan/plan.md, Abschnitt 6, Punkt 1). Nach jeder Aenderung an definition.md:

    python plan/extract_features.py

Neue IDs erscheinen als 'offen', bestehende behalten ihren Status aus der
vorhandenen CSV, entfallene werden gemeldet. Rationale-Kategorien (Kapitel,
Leitentscheidungen, Entscheidungslog, Stufenplan) tragen dauerhaft
'rationale' und werden nie abgehakt.
"""
from __future__ import annotations

import csv
import io
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent
REF = ROOT / "definition.md"
OUT = ROOT / "features.csv"
COLS = ["ID", "Kategorie", "Abschnitt", "Bezeichnung", "Stufe/Klasse",
        "Meilenstein (Vorschlag)", "Status"]

# Kategorien ohne Implementierung: Begruendung, nicht Bauteil.
RATIONALE = {"Kapitel", "Leitentscheidung", "Entscheidung", "Stufe (Roadmap)"}

# Abschnittsnummer -> Meilenstein (Kuratierung nach plan.md, Abschnitt 4).
MILESTONE_BY_CHAPTER = {
    "0": "Doku/Rationale",
    "1": "M0 (Parser/MIR-Entwurf)",
    "2": "M0 (Parser/MIR-Entwurf)",
    "3": "M1 (Kernsemantik)",
    "4": "M1 (Kernsemantik)",
    "5": "M1 (Kernsemantik)",
    "6": "M1 (Kernsemantik)",
    "7": "M1 (Kernsemantik)",
    "8": "M2 (Ströme)",
    "9": "M1/M2 (Interpreter)",
    "10": "M3 (Gate)",
    # Kapitel 11 mischt vier Dinge mit sehr verschiedenen Zeitpunkten:
    # 11.1 Komponenten (durchgehend), 11.2 Lowering (M1-M4), 11.3
    # Build-Regeln (M4), 11.4 Standardbibliothek (M6) und 11.5 `takt size`
    # (M3). MILESTONE_BY_SECTION loest sie auf; ohne Eintrag gilt 11 -> M4.
    "11": "M4 (Codegen)",
    "12": "M4/M5 (Runtime)",
    "13": "M4–M7",
    "14": "je Meilenstein",
    "15": "Doku/Rationale",
    "16": "Doku/Rationale",
}
MILESTONE_BY_STAGE = {"v1.1": "M6", "v1.2": "M8", "v2": "M9 (v2)"}

# Nur bei diesen Kategorien verschiebt die Stufe den Meilenstein allein.
STAGE_DRIVES_MILESTONE = {"Semantik/Abschnitt", "Absatz/Regel"}

# Grammatikproduktionen spaeterer Stufen werden nach dem Exit-Kriterium von M0
# bereits dort geparst; nur ihre Semantik faellt in die spaetere Stufe. Sie
# tragen deshalb beide Meilensteine (plan.md, Abschnitt 4).
STAGE_SPLIT_PARSER = "Grammatik"

# Einzelfaelle, die weder aus Abschnitt noch Stufe folgen (plan.md, Abschnitt 4).
MILESTONE_OVERRIDE = {
    "SEM-3.12": "M1 ([U]) / M6 (const) / M8 (type)",
    "G-generic_vars": "M0 (Parser) / M6 (const) / M8 (type)",
    "G-gvar": "M0 (Parser) / M6 (const) / M8 (type)",
    "G-instance_decl": "M0 (Parser) / M8 (im Zustand)",
    # 28, 29, 32 und 39 rechnet M3 aus, aber ihre Eingabe entsteht erst
    # spaeter: `jitter`, `c_target[c]` und die Speichergroessen kommen aus
    # der Konformitaetsmessung (13.8, M5) und der Hardware-Konfiguration
    # (8.10, M6). Ohne sie gaebe es nur ein Urteil ueber geschaetzte Zahlen
    # (plan/feedback-design.md, Abschnitt 10).
    "SC-28": "M6 (mit 8.10)",
    "SC-29": "M6 (mit 8.10)",
    "SC-32": "M6 (mit 8.10)",
    "SC-39": "M6 (mit 8.10)",
    # Die Treiberstufe (`port`, `driver machine`) kommt mit v1.2.
    "SC-59": "M8 (mit port)",
    # Braucht die Hardware-Konfiguration als Eingabe (8.10).
    "SC-60": "M6 (mit 8.10)",
    # Latenz und Maschinenbudget rechnet das Gate (9.4.5, 7.2).
    "SC-61": "M3 (Gate)",
    "SC-62": "M3 (Gate)",
    "SC-63": "M3 (Gate)",
    # Bewertet und vorgezogen: erster Baustein von M6, weil Grammatik,
    # AST, MIR-Feld und Einheitenalgebra stehen (plan/einheiten-integer.md).
    "SC-38": "M6 (frueh)",
    # `fma` ist eine Primitive aus `libtaktm` (4.2), kein Baustein der
    # Standardbibliothek; Satz 9.4.4 wird mit dem Codegen pruefbar.
    "LIB-fma": "M4 (libtaktm)",
    "THM-Satz-9.4.4": "M4 (Codegen)",
    # Die Annahmen gehoeren zu `property` und damit zu M6; die
    # Hardware-Konfiguration ist die Eingabe der Pruefungen 28/32/39/60
    # und entsteht mit 8.10, nicht im Parser.
    "PAR-13.3-Umgebungsannahmen_assumption": "M6 (v1.1)",
    "PAR-13.3-Kanal_Attribute_gelten_automatisch_als_Annahmen": "M6 (v1.1)",
    "PAR-13.3-Kompositionalität": "M6 (v1.1)",
    "FMT-Hardware-Konfiguration": "M6 (mit 8.10)",
    # Jedes Format entsteht mit seinem Verbraucher: die Aufzeichnung mit
    # Record/Replay (12.5), der Lauf-Header mit `takt run`, der
    # Konformitaetsbericht mit `takt bench` (13.8).
    "FMT-Aufzeichnung": "M4 (Codegen)",
    "FMT-Lauf-Header": "M4 (Codegen)",
    "FMT-Konformitätsbericht": "M5 (Embedded)",
    "CLI-latency": "M3 (Gate)",
    # `append` ist mit M3 gebaut, nicht mit dem Parser (FB-42).
    "MEM-append": "M3",
}


def slug(text):
    """Titel -> ID-Bestandteil: Buchstaben, Ziffern, Punkt, Unterstrich."""
    t = text.replace("`", "").strip().rstrip(".")
    t = re.sub(r"[^\w.]+", "_", t, flags=re.UNICODE)
    return re.sub(r"_+", "_", t).strip("_")


def stage_of(text):
    """Stufenmarkierung v1.1 / v1.2 / v2 aus einem Titel oder Produktionsrumpf.

    Sie steht mal allein in Klammern ('(v1.1)'), mal in einer Aufzaehlung
    ('(`idle`, v1.1)') und mal in einem EBNF-Kommentar ('(* …; v1.2 *)').
    """
    m = re.search(r"\bv(1\.1|1\.2|2)\b", text)
    return "v" + m.group(1) if m else ""


# Abschnitt (zwei Ebenen) -> Meilenstein, feiner als das Kapitel. Greift
# nach MILESTONE_OVERRIDE und vor MILESTONE_BY_CHAPTER.
MILESTONE_BY_SECTION = {
    "11.1": "je Meilenstein",   # Komponentenliste: waechst mit jedem Strang
    "11.2": "M1-M4 (Lowering)",  # Lowering-Skizze: Sema baut, Codegen nutzt
    "11.3": "M4 (Codegen)",      # Sim/HW-Build, reproduzierbare Builds
    "11.4": "M6 (stdlib)",       # Standardbibliothek in Takt selbst
    "11.5": "M3 (Gate)",         # `takt size` — mit M3 gebaut
}


def milestone(entry_id, kategorie, section, stage):
    if entry_id in MILESTONE_OVERRIDE:
        return MILESTONE_OVERRIDE[entry_id]
    # Die Abschnittszuordnung gilt, solange keine Stufe etwas anderes sagt:
    # ein `@stage v1.1` verschiebt den Eintrag unabhaengig vom Abschnitt.
    if section in MILESTONE_BY_SECTION and not (stage and kategorie in STAGE_DRIVES_MILESTONE):
        return MILESTONE_BY_SECTION[section]
    if stage in MILESTONE_BY_STAGE:
        if kategorie in STAGE_DRIVES_MILESTONE:
            return MILESTONE_BY_STAGE[stage]
        if kategorie == STAGE_SPLIT_PARSER:
            return "M0 (Parser) / " + MILESTONE_BY_STAGE[stage]
    return MILESTONE_BY_CHAPTER.get(section.split(".")[0], "")


def exec_signature(line):
    """Argumentliste eines exec(...)-Aufrufs ohne Zustand und Modus."""
    depth, start = 0, line.index("(")
    for i in range(start, len(line)):
        if line[i] == "(":
            depth += 1
        elif line[i] == ")":
            depth -= 1
            if depth == 0:
                inner = line[start + 1:i]
                return re.sub(r",\s*s,\s*\w+$", "", inner).strip()
    return None


SRC = io.open(REF, encoding="utf-8").read()
LINES = SRC.split("\n")
ROWS = []


def add(entry_id, kategorie, section, bezeichnung, stage="", ms=None):
    ROWS.append({
        "ID": entry_id,
        "Kategorie": kategorie,
        "Abschnitt": section,
        "Bezeichnung": bezeichnung,
        "Stufe/Klasse": stage,
        "Meilenstein (Vorschlag)": (milestone(entry_id, kategorie, section, stage)
                                    if ms is None else ms),
        "Status": "",
    })


def slice_between(start_marker, end_marker):
    a = SRC.index(start_marker)
    b = SRC.index(end_marker, a)
    return SRC[a:b]


def code_block(anchor, nth=0):
    """Inhalt des n-ten ```-Blocks nach einem Anker (Regex)."""
    m = re.search(anchor, SRC)
    if not m:
        sys.exit("Anker nicht gefunden: " + anchor)
    blocks = re.findall(r"```[a-z]*\n(.*?)```", SRC[m.end():], re.S)
    return blocks[nth]


def table(anchor):
    """Datenzeilen der ersten Markdown-Tabelle nach einem Anker (Regex)."""
    m = re.search(anchor, SRC)
    if not m:
        sys.exit("Anker nicht gefunden: " + anchor)
    out, seen = [], False
    for line in SRC[m.end():].split("\n"):
        if line.startswith("|"):
            seen = True
            row = line.strip().strip("|")
            cells = [c.replace("\\|", "|").strip()
                     for c in re.split(r"(?<!\\)\|", row)]
            if not all(re.fullmatch(r":?-{2,}:?", c) for c in cells):
                out.append(cells)
        elif seen:
            break
    return out[1:] if out else []


# --- Kapitel ---------------------------------------------------------------
for line in LINES:
    m = re.match(r"^## (\d+)\. (.+)$", line)
    if m:
        add("CH-" + m.group(1), "Kapitel", m.group(1), m.group(2).strip(),
            ms="Doku/Rationale")

# --- Semantikabschnitte -----------------------------------------------------
SECTIONS = []
for line in LINES:
    m = re.match(r"^### ([\d.]+) (.+)$", line)
    if m:
        section, title = m.group(1), m.group(2).strip()
        SECTIONS.append((section, title))
        add("SEM-" + section, "Semantik/Abschnitt", section, title, stage_of(title))

# --- Grammatikproduktionen --------------------------------------------------
EBNF = code_block(r"### 2\.3 Grammatik \(EBNF\)")
PROD, current = {}, None
for line in EBNF.split("\n"):
    m = re.match(r"^([a-z_]+)\s*:=", line)
    if m:
        current = m.group(1)
        PROD[current] = line
    elif current is not None and line.startswith(" "):
        PROD[current] += "\n" + line
for name, body in PROD.items():
    add("G-" + name, "Grammatik", "2.3", name, stage_of(body))

# --- Alternativen einzelner Produktionen ------------------------------------
ALTERNATIVES = [
    ("system_item", "system-Eintrag"),
    ("attr", "Channel-Attribut"),
    ("scalar_type", "Skalartyp"),
    ("type", "Typkonstruktor"),
    ("elem_type", "Stream-Elementtyp"),
    ("simple_stmt", "Statement"),
    ("seq_item", "Sequenz-Item"),
    ("tprop|tprop_implies|tprop_or|tprop_and|tprop_not|tprop_atom", "Temporaloperator"),
    ("capability", "Generics-Fähigkeit"),
    ("framing", "Rahmung"),
    ("binding", "Bindung"),
    ("campaign_item", "Kampagnen-Item"),
    ("gvar", "Generics-Klasse"),
]
# In diesen Produktionen leitet das Literal eine Range ein, es ist keine
# Alternative der Produktion selbst.
SYNTAX_NOISE = {"in"}

for prod, kategorie in ALTERNATIVES:
    seen = []
    body = "\n".join(PROD[p] for p in prod.split("|") if p in PROD)
    label = prod.split("|")[0]
    for lit in re.findall(r'"([^"]+)"', body):
        if (re.fullmatch(r"[a-z_][a-z0-9_]*", lit)
                and lit not in seen and lit not in SYNTAX_NOISE):
            seen.append(lit)
    for lit in seen:
        add(kategorie + "-" + lit, kategorie, "2.3", '%s: "%s"' % (label, lit))

# --- Schluesselwoerter und reservierte Woerter ------------------------------
keywords, reserved = [], []
for line in code_block(r"### 2\.2 Schlüsselwörter").split("\n"):
    if not line.strip():
        continue
    if line.startswith("reserviert"):
        reserved = line.split(":", 1)[1].split()
    else:
        for word in line.split():
            if word not in keywords:
                keywords.append(word)
for word in keywords:
    add("KW-" + word, "Schlüsselwort", "2.2", word)
for word in reserved:
    add("KW-reserviert-" + word, "Reserviertes Wort", "2.2/2.5", word)

# --- Reservierte Membernamen ------------------------------------------------
members = re.search(r"\*\*Reservierte Membernamen\.\*\* Die eingebauten Zugriffe — (.+?) —", SRC, re.S)
for name in members.group(1).replace("`", "").split():
    add("MEM-" + name, "Reservierter Membername", "2.5", name)

# --- Statische Pruefungen ---------------------------------------------------
for cells in table(r"## 10\. Statische Analysen"):
    if len(cells) >= 3 and cells[0].isdigit():
        # Die meisten Pruefungen entstehen im Gate; die wenigen, deren
        # Eingabe oder Konstrukt spaeter kommt, nennt MILESTONE_OVERRIDE.
        sc = "SC-" + cells[0]
        add(sc, "Statische Prüfung", "10", cells[1], cells[2],
            ms=MILESTONE_OVERRIDE.get(sc, "M3 (Gate)"))

# --- Beweisverpflichtungen T1..T16 ------------------------------------------
proof = re.search(r"\*\*Satz 9\.4\.2.*?∎", SRC, re.S).group(0)
for t in re.findall(r"\(T(\d+)\)", proof):
    add("OBL-T" + t, "Beweisverpflichtung", "9.4.2", "T" + t,
        ms="M1/M2 (Interpreter)")

# --- Saetze, Lemmata und Absatzregeln ---------------------------------------
section = None
for line in LINES:
    if re.match(r"^## Anhang", line):
        break
    m = re.match(r"^### ([\d.]+) ", line)
    if m:
        section = m.group(1)
        continue
    if section is None:
        continue
    m2 = re.match(r"^(?:- )?\*\*(.+?)\*\*", line)
    if not m2:
        continue
    title = m2.group(1).strip()
    if re.match(r"^(Satz|Lemma) ", title):
        name = title.split("(")[0].strip()
        thm = "THM-" + name.replace(" ", "-")
        # Die meisten Saetze prueft der Interpreter; die Ausnahmen nennt
        # MILESTONE_OVERRIDE (9.4.4 braucht den Codegen als zweite Quelle).
        add(thm, "Satz/Lemma", name.split()[1].rstrip("."), name,
            ms=MILESTONE_OVERRIDE.get(thm, "M1/M2 (Interpreter)"))
    else:
        title = title.rstrip(".")
        add("PAR-%s-%s" % (section, slug(title)), "Absatz/Regel", section,
            title, stage_of(title))

# --- Semantikfunktionen und exec-Regeln -------------------------------------
NINE = slice_between("## 9. Formale Semantik", "## 10. Statische Analysen")
functions, exec_rules = set(), []
for body in re.findall(r"```[a-z]*\n(.*?)```", NINE, re.S):
    for line in body.split("\n"):
        m = re.match(r"^([a-z_]+)\(([^)]*)\)\s*[:=]", line)
        if m and m.group(1) != "exec":
            functions.add(m.group(1))
        if line.startswith("exec("):
            signature = exec_signature(line)
            if signature and signature not in exec_rules:
                exec_rules.append(signature)
for name in sorted(functions | {"exec"}):
    add("FN-9-" + name, "Semantikfunktion", "9", name, ms="M1/M2 (Interpreter)")
for signature in exec_rules:
    add("RULE-9.2-" + signature.replace(" ", "_"), "exec-Regel", "9.2",
        "exec(%s)" % signature, ms="M1/M2 (Interpreter)")

# --- Desugaring-Regeln ------------------------------------------------------
for cells in table(r"### 6\.2 Desugaring \(formal\)"):
    if len(cells) >= 2 and cells[0]:
        add("DESUGAR-" + slug(cells[0]), "Desugaring-Regel", "6.2", cells[0],
            ms="M1 (Kernsemantik)")

# --- Regeln der verteilten Ausfuehrung --------------------------------------
DIST = slice_between("### 12.9 Verteilte Ausführung", "## 13. Verifikation")
for num, title in re.findall(r"^(\d+)\. \*\*(.+?)\*\*", DIST, re.M):
    add("DIST-rule" + num, "Regel", "12.9", title, "v2", ms="M9 (v2)")

# --- Defensiver Treiberrand -------------------------------------------------
for cells in table(r"### 12\.6 Defensiver Treiberrand"):
    if len(cells) >= 2 and cells[0].isdigit():
        add("RAND-" + cells[0], "Regel/Prüfung", "12.6", cells[1],
            ms="M4/M5 (Runtime)")

# --- Laufzeitprofile --------------------------------------------------------
for cells in table(r"### 12\.8 Laufzeitprofile"):
    name = cells[0].strip("`")
    if re.fullmatch(r"[a-z_]+", name):
        add("PROFIL-" + name, "Profil/Eintrag", "12.8", name,
            ms="M4/M5 (Runtime)")

# --- Fault-Arten ------------------------------------------------------------
faults = re.search(r"Fault-Arten: (.+?12\.9\))\.", SRC, re.S).group(1)
for fault in re.findall(r"`([^`]+)`", faults):
    add("FAULT-" + slug(fault), "Fault-Art", "5.3", fault, ms="M1 (Kernsemantik)")

# --- Qualitaeten ------------------------------------------------------------
quality = re.search(r"Qualität ∈ \{(.+?)\}", SRC).group(1)
for name in [q.strip() for q in quality.split(",")]:
    add("QUAL-" + name, "Qualität", "3.5", name, ms="M1 (Kernsemantik)")

# --- Standardbibliothek -----------------------------------------------------
LIB_RE = re.compile(r"^(native fn|native job|fn|block|machine)\s+([A-Za-z_][\w\[\], /]*?)\s*(?:\(|$)")
for line in code_block(r"### 11\.4 Standardbibliothek").split("\n"):
    m = LIB_RE.match(line)
    if m:
        kind, name = m.group(1), m.group(2).strip()
        key = re.match(r"[A-Za-z_]\w*", name).group(0)
        lib = "LIB-" + key
        add(lib, "Standardbibliothek", "11.4", "%s %s" % (kind, name),
            ms=MILESTONE_OVERRIDE.get(lib, MILESTONE_BY_SECTION["11.4"]))

# --- Werkzeugkommandos ------------------------------------------------------
# Die Kommandos entstehen mit sehr verschiedenen Meilensteinen: `check`,
# `fmt`, `size` und die Entwicklerstufen stehen seit M0-M3, `run`/`replay`
# brauchen die Runtime (M4), `bench`/`driver-test` die Hardware (M5).
CLI_MILESTONE = {
    "check": "M0-M3", "fmt": "M0", "size": "M3 (Gate)", "latency": "M3 (Gate)",
    "mir": "M0", "parse": "M0", "tokens": "M0",
    "sim": "M1/M2 (Interpreter)",
    "run": "M4 (Codegen)", "replay": "M4 (Codegen)",
    "test": "M4 (Codegen)", "graph": "M4 (Codegen)",
    "campaign": "M6 (v1.1)", "prove": "M6 (v1.1)", "tune": "M6 (v1.1)",
    "driver-test": "M5 (Embedded)", "bench": "M5 (Embedded)",
    "migrate": "M7 (Werkzeuge)", "import-c": "M7 (Werkzeuge)",
}
cli = re.search(r"^takt-cli\s+(.+)$", code_block(r"### 11\.1 Komponenten"), re.M)
for command in [c.strip() for c in cli.group(1).split("|")]:
    add("CLI-" + command, "Werkzeug", "11.1", "takt " + command,
        ms=CLI_MILESTONE.get(command, "M4 (Codegen)"))

# --- Beispiele --------------------------------------------------------------
for section, title in SECTIONS:
    if section.startswith("14."):
        add("EX-" + section, "Beispiel (Golden-Test)", section, title,
            ms="je Meilenstein")

# --- Leitentscheidungen -----------------------------------------------------
PRINC = slice_between("### 0.2 Die vierundzwanzig", "### 0.3 Nicht-Ziele")
for num, title in re.findall(r"^(\d+)\. \*\*(.+?)\*\*", PRINC, re.M):
    add("PRINC-" + num, "Leitentscheidung", "0.2", title)

# --- Entscheidungslog -------------------------------------------------------
for cells in table(r"## 16\. Entscheidungslog"):
    if len(cells) >= 4 and cells[0]:
        question = cells[0]
        decision = cells[2].replace("**", "").strip()
        add("DEC-" + question.replace(" ", "_"), "Entscheidung", "16",
            "%s → %s" % (question, decision))

# --- Konstrukte aus 2.4 -----------------------------------------------------
for cells in table(r"### 2\.4 Bedeutung der Kernkonstrukte"):
    if len(cells) >= 2 and cells[0]:
        construct = cells[0]
        add("CON-" + construct.replace("`", "").replace(" ", "_"),
            "Konstrukt (2.4)", "2.4", construct, ms="M0 (Parser/MIR-Entwurf)")

# --- Versionierte Formate ---------------------------------------------------
formats = re.search(r"\*\*Versionierte Formate\.\*\* (.+?) tragen Formatversion",
                    SRC, re.S).group(1)
for name in re.split(r",\s*|\s+und\s+", formats.strip()):
    name = re.sub(r"\s*\([^)]*\)", "", name).strip().rstrip(".")
    if name:
        fmt_id = "FMT-" + name.replace(" ", "-")
        # Die meisten Formate entstehen mit der MIR; die Ausnahmen nennt
        # MILESTONE_OVERRIDE (die Hardware-Konfiguration etwa kommt mit 8.10).
        add(fmt_id, "Versioniertes Format", "11.3", name,
            ms=MILESTONE_OVERRIDE.get(fmt_id, "M0 (Parser/MIR-Entwurf)"))

# --- Stufenplan -------------------------------------------------------------
for cells in table(r"## 15\. Stufenplan"):
    if len(cells) >= 2 and cells[0]:
        name = cells[0].replace("**", "").strip()
        add("STUFE-" + name.replace(" ", "_"), "Stufe (Roadmap)", "15", name)

# --- Status uebernehmen und schreiben ---------------------------------------
previous = {}
if OUT.exists():
    for row in csv.DictReader(io.open(OUT, encoding="utf-8")):
        previous[row["ID"]] = (row.get("Status") or "offen").strip()

for row in ROWS:
    if row["Kategorie"] in RATIONALE:
        row["Status"] = "rationale"
    else:
        keep = previous.get(row["ID"], "offen")
        row["Status"] = keep if keep in ("definiert", "teilweise", "fertig") else "offen"

ids = [row["ID"] for row in ROWS]
duplicates = sorted({i for i in ids if ids.count(i) > 1})
if duplicates:
    sys.exit("Doppelte IDs: " + ", ".join(duplicates))

with io.open(OUT, "w", encoding="utf-8", newline="") as handle:
    writer = csv.DictWriter(handle, fieldnames=COLS, lineterminator="\n")
    writer.writeheader()
    writer.writerows(ROWS)

added = sorted(set(ids) - set(previous))
removed = sorted(set(previous) - set(ids))
print("%d Eintraege -> %s" % (len(ROWS), OUT))
if added:
    print("  neu (%d): %s" % (len(added), ", ".join(added[:12])))
if removed:
    print("  entfallen (%d): %s" % (len(removed), ", ".join(removed[:12])))
