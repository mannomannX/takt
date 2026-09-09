#!/usr/bin/env python3
"""Struktureller Rauchtest fuer Takt-Beispieldateien.

Ersetzt keinen Parser. Prueft, was ohne Grammatik-Frontend pruefbar ist:
Einrueckung, Blockstruktur, reservierte Namen, initial-Pflicht, Transitions-
bloecke mit Uebergang am Ende, Aktionsblock-Beschraenkungen, Namenskonventionen.

Schluessel- und Membernamen werden aus der Sprachreferenz gelesen, damit der
Test nicht von ihr abweicht:   python3 check_examples.py [--spec PFAD] [DATEIEN...]
"""
import re, sys, glob, os

DEFAULT_SPEC = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "plan", "definition.md")

def load_names(spec_path):
    kw, reserved, members = set(), set(), set()
    if not os.path.exists(spec_path):
        return kw, reserved, members
    s = open(spec_path, encoding="utf-8").read()
    a = s.find("### 2.2 Schlüsselwörter"); b = s.find("### 2.3", a)
    for line in s[a:b].split("\n"):
        if line.startswith("reserviert"):
            reserved.update(line.split(":", 1)[1].split())
        elif line and not line.startswith(("#", "```")):
            kw.update(w for w in line.split() if re.fullmatch(r"[a-z_][a-z_0-9]*", w))
    # 2.5: als Feldnamen verboten sind nur die Zugriffe der Wrapper
    m = re.search(r"Verboten als Feldnamen sind nur die Zugriffe der Wrapper `(.*?)`", s)
    if m:
        members.update(m.group(1).split())
    return kw, reserved, members

DECL = re.compile(r"^\s*(?:pub\s+|persist\s+|tunable\s+|driver\s+)?"
                  r"(machine|scenario|fn|block|native\s+fn|native\s+job|var|input|output|command|unit|unitvec|"
                  r"instance|signal|param|const|enum|record|type|node|property|port|profile|trigger|campaign)\s+"
                  r"([A-Za-z_][A-Za-z_0-9]*)")
FIELD = re.compile(r"^\s+([a-z_][a-z_0-9]*)\s*:\s*[A-Za-z_\[]")

def indent(line): return len(line) - len(line.lstrip(" "))

def block_of(lines, i):
    """Zeilen des eingerueckten Blocks, der bei i+1 beginnt."""
    base = indent(lines[i]); out = []
    for j in range(i + 1, len(lines)):
        l = lines[j]
        if not l.strip() or l.lstrip().startswith("#"): continue
        if indent(l) <= base: break
        out.append((j, l))
    return out

def check(path, kw, reserved, members):
    errs, warns = [], []
    raw = open(path, encoding="utf-8").read().split("\n")
    def err(n, msg): errs.append(f"{path}:{n}: {msg}")
    def warn(n, msg): warns.append(f"{path}:{n}: {msg}")

    in_record = False; rec_indent = 0
    for i, line in enumerate(raw):
        n = i + 1
        if "\t" in line:
            err(n, "Tabulator gefunden (2.1: 4 Leerzeichen, Tabs sind Fehler)")
        if line.strip() and not line.lstrip().startswith("#") and indent(line) % 4 != 0:
            err(n, f"Einrueckung {indent(line)} ist kein Vielfaches von 4 (2.1)")
        stripped = line.strip()
        if not stripped or stripped.startswith("#"): continue

        m = DECL.match(line)
        if m:
            kind, name = re.sub(r"\s+", " ", m.group(1)), m.group(2)
            if name in kw or name in reserved:
                err(n, f"'{name}' ist Schluessel-/reserviertes Wort und nicht als Bezeichner erlaubt (2.5)")
            if kind in ("machine", "fn", "block", "instance", "node", "port", "property", "unit", "signal",
                        "command", "input", "output", "var", "native fn", "native job") \
               and not re.fullmatch(r"[a-z_][a-z_0-9]*", name):
                warn(n, f"{kind} '{name}' sollte snake_case sein (2.1)")
            if kind in ("const", "param", "profile") and not re.fullmatch(r"[A-Z][A-Z_0-9]*", name):
                warn(n, f"{kind} '{name}' sollte UPPER_SNAKE_CASE sein (2.1)")
            if kind in ("enum", "record", "type", "unitvec") and not re.fullmatch(r"[A-Z][A-Za-z0-9]*", name):
                warn(n, f"{kind} '{name}' sollte PascalCase sein (2.1)")

        if re.match(r"^\s*record\s+", line):
            in_record, rec_indent = True, indent(line)
        elif in_record and indent(line) <= rec_indent:
            in_record = False
        if in_record:
            f = FIELD.match(line)
            if f:
                fname = f.group(1)
                if fname in members:
                    err(n, f"Feldname '{fname}' ist reservierter Membername (2.5)")
                if fname in kw or fname in reserved:
                    err(n, f"Feldname '{fname}' ist Schluessel-/reserviertes Wort (2.5)")

        ms = re.match(r"^\s*state\s+([A-Za-z_][A-Za-z_0-9]*)", line)
        if ms and not re.fullmatch(r"[A-Z][A-Z_0-9]*", ms.group(1)):
            warn(n, f"Zustand '{ms.group(1)}' sollte UPPER_SNAKE_CASE sein (2.1)")

        if stripped.endswith(":") and not stripped.startswith("#"):
            if not block_of(raw, i):
                err(n, "Blockkopf ohne eingerueckten Block")

        if re.match(r"^\s*(when|after)\b.*:\s*$", line):
            blk = block_of(raw, i)
            if blk and not blk[-1][1].strip().startswith("->"):
                err(blk[-1][0] + 1, "Transitionsblock endet nicht mit '-> ZIEL' (2.3: trans_block)")

        if re.match(r"^\s*(enter|exit)\s*:\s*$", line):
            for j, l in block_of(raw, i):
                bad = re.match(r"^\s*(check|expect|abort|alert|wait)\b", l)
                if bad:
                    err(j + 1, f"'{bad.group(1)}' ist in Aktionsbloecken nicht erlaubt (5.5)")

        if re.match(r"^\s*(machine|scenario)\b", line) and stripped.endswith(":"):
            body = block_of(raw, i)
            depth = indent(body[0][1]) if body else 0
            if not any(re.match(r"^\s*initial\s+[A-Z]", l) and indent(l) == depth for _, l in body):
                err(n, "Maschine/Szenario ohne 'initial ZUSTAND' auf oberster Ebene (2.3: machine_body)")
    return errs, warns

def main():
    args = sys.argv[1:]
    spec = DEFAULT_SPEC
    if "--spec" in args:
        k = args.index("--spec"); spec = args[k + 1]; del args[k:k + 2]
    files = args or sorted(glob.glob(os.path.join(os.path.dirname(os.path.abspath(__file__)), "*.takt")))
    kw, reserved, members = load_names(spec)
    if not kw:
        print(f"warn: Spezifikation {spec} nicht gefunden — Namensprüfungen inaktiv")
    total_e = 0
    for f in files:
        errs, warns = check(f, kw, reserved, members)
        total_e += len(errs)
        status = "FEHLER" if errs else ("hinweise" if warns else "ok")
        print(f"[{status:8}] {f}")
        for e in errs:   print("   E", e)
        for w in warns:  print("   W", w)
    print(f"\n{len(files)} Dateien, {total_e} Fehler")
    return 1 if total_e else 0

if __name__ == "__main__":
    sys.exit(main())
