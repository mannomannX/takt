#!/usr/bin/env python3
"""Grammatik-Fuzzer: erzeugt wohlgeformte Takt-Programme aus grammar/takt.ebnf und
schickt sie durch Parser, Orakel und Formatter (Entwurf: plan/fuzzer.md).

    python grammar/fuzz_grammar.py [--count N] [--seed S] [--start file|snippet]
                                   [--contextual] [--no-oracle] [--keep DIR] [--cover K]

Je Ableitung entstehen fuenf Zwillinge: kanonisch, minimaler Leerraum, maximaler
Leerraum, geklammert (jede binaere Operation in Klammern) und verrauscht
(Fortsetzungszeilen, Kommentare, Leerzeilen). Geprueft wird: alle werden vom
Parser angenommen, Orakel bestaetigt, alle liefern denselben Baum, die drei
Leerraumfassungen formatieren identisch, und jede Fassung besteht `fmt --verify`.
Am Ende steht die Abdeckung der Grammatikalternativen; Fehlschlaege werden
verkleinert und unter --keep abgelegt (Default out/fuzz).
"""
from __future__ import annotations

import argparse
import io
import os
import random
import re
import shutil
import subprocess
import sys
import tempfile
from collections import Counter
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import check_grammar as cg  # noqa: E402
import parse_corpus as pc  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent
BATCH = 120

# Produktionen, in denen "<" … ">" einen Typ klammert (wie im Orakel).
ANGLE_PRODS = {"type", "scalar_type", "elem_type", "stream_decl"}
# Ausdruecke: Vorrangkette fuer die Klammer-Zwillinge (linksassoziative Ketten).
CHAIN_PRODS = {"or_expr", "and_expr", "cmp_expr", "bitor_expr", "bitxor_expr", "bitand_expr", "shift_expr",
               "add_expr", "mul_expr", "tprop_implies", "tprop_or", "tprop_and"}
# Rekursionsbudget je Produktion (Verschachtelungstiefe).
NEST_LIMIT = {"expr": 3, "tprop": 2, "type": 3, "state_decl": 2, "block": 3, "stmt": 5, "seq_item": 3,
              "unit_expr": 2, "case_pattern": 2, "postfix": 2, "unary": 2, "not_expr": 2, "tprop_not": 2}
SCALAR_WORDS = {"bool", "int", "i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64", "float", "f32", "f64",
                "Duration", "str"}
TYPE_WORDS = {"bytes", "vec", "line", "stream", "samples", "table", "mat", "map", "capture", "Edge"}
TIME_SUFFIXES = set(pc.TIME)
PAREN_DEPTH = 24
TWO_CHAR_OPS = {"->", "..", "+=", "-=", "*=", "/=", "==", "!=", "<=", ">=", "<<", ">>"}
# STRING-Stellen mit Teilsprache (Produktion -> Teilsprache)
STRING_KIND = {"pattern": "pattern_text", "binding": "address_text", "node_decl": "address_text",
               "system_item": "address_text", "log_stmt": "format_text", "check_stmt": "format_text",
               "alert_stmt": "format_text", "verify_stmt": "format_text", "abort_stmt": "format_text",
               "verdict_stmt": "format_text", "seq_item": "format_text"}

UNIT_NAMES = ["bar", "mV", "V", "A", "K", "m", "s", "min", "h", "Hz", "pct", "degC", "KiB", "B", "Pa", "us", "ms",
              "N", "W", "kg"]
IDENT_POOL = ["x", "y", "z", "foo", "bar_1", "tank_p", "i_u", "pos", "count", "rx", "tx", "level", "temp", "q",
              "valve_cmd", "n", "k", "acc", "seq_no", "big_name_value"]
UPPER_POOL = ["A", "B", "IDLE", "RUN", "FAULT", "SAFE", "N", "U", "K_MAX", "P1", "OPEN", "CLOSED", "LONG_STATE_NAME"]
TYPE_POOL = ["Header", "Frame", "CanFrame", "Kind", "Result", "ValveCmd", "Rec", "Pt", "ImageState", "Xy"]


class Atom:
    """Ein Ausgabestueck: Tokentext oder Strukturmarker."""

    __slots__ = ("kind", "text", "glue", "space")

    def __init__(self, kind, text="", glue=False, space=False):
        self.kind, self.text, self.glue, self.space = kind, text, glue, space

    def __repr__(self):
        return f"{self.kind}:{self.text}"


class Grammar:
    def __init__(self, start):
        _, _, reserved, prods, _ = cg.load()
        keywords, _ = cg.keywords_from_reference()
        self.keywords, self.reserved = set(keywords), set(reserved)
        self.contextual = pc.contextual_words(prods, self.keywords, self.reserved)
        prods = dict(prods)
        prods["snippet"] = {"rhs": pc.SNIPPET_RHS, "stage": "", "start": True, "checks": []}
        self.trees = {name: cg.parse_rhs(cg.TOKEN_RE.findall(p["rhs"])) for name, p in prods.items()}
        self.start = start
        self.group_ids = {}
        for name, alts in self.trees.items():
            self._number_groups(name, alts, [0])
        self.min_size = self._min_sizes()
        handmade = {"unit_expr", "unit_term", "unit_lit", "number", "int_lit", "duration_lit"}
        self.alt_keys = [(name, i) for name, alts in self.trees.items() for i in range(len(alts))
                         if len(alts) > 1 and name not in handmade]
        self.alt_keys += [k for k in self.group_ids.values() if k[0] not in handmade]

    def _number_groups(self, prod, alts, counter):
        for alt in alts:
            for f in alt:
                if f[0] == "grp":
                    counter[0] += 1
                    self.group_ids[id(f)] = (prod, f"g{counter[0]}")
                    self._number_groups(prod, f[2], counter)

    def _min_sizes(self):
        size = {name: float("inf") for name in self.trees}

        def factor(f):
            if f[0] in ("term", "tok"):
                return 1
            if f[0] == "nt":
                return size.get(f[1], 1)
            if f[1] == "(":
                return min(sum(map(factor, alt)) for alt in f[2])
            return 0

        changed = True
        while changed:
            changed = False
            for name, alts in self.trees.items():
                best = min(sum(map(factor, alt)) for alt in alts)
                if best < size[name]:
                    size[name] = best
                    changed = True
        return size

    def factor_size(self, f):
        if f[0] in ("term", "tok"):
            return 1
        if f[0] == "nt":
            return self.min_size[f[1]]
        if f[1] == "(":
            return min(self.seq_size(alt) for alt in f[2])
        return 0

    def seq_size(self, seq):
        return sum(self.factor_size(f) for f in seq)


def lead(alt):
    """Erstes Element einer Alternative als Kennung (fuer Ausschluesse)."""
    if not alt:
        return ""
    f = alt[0]
    return f[1] if f[0] in ("term", "tok", "nt") else "grp"


class Generator:
    def __init__(self, grammar, rng, contextual=False, cover=2):
        self.g = grammar
        self.rng = rng
        self.contextual = contextual
        self.cover = cover
        self.coverage = Counter()
        self.nest = Counter()
        self.depth = 0
        self.size = 0
        self.size_limit = 240
        self.expr_start = None
        self.expr_limit = 40

    # ------------------------------------------------------------ Ableitung

    def program(self):
        """Eine Ableitung des Startsymbols als Atomliste; leere Bloecke werden verworfen."""
        for _ in range(50):
            self.nest.clear()
            self.depth = 0
            self.size = 0
            atoms = self.derive(self.g.start, {"angle": 0})
            if not any(a.kind == "in" and b.kind == "de" for a, b in zip(atoms, atoms[1:])):
                return atoms
        raise RuntimeError("keine nichtleere Ableitung gefunden")

    def budget_ok(self):
        if self.expr_start is not None and self.size - self.expr_start >= self.expr_limit:
            return False
        return self.hard_ok()

    def hard_ok(self):
        """Grenzen, die auch kleine Wiederholungen nicht ueberschreiten duerfen."""
        return self.size < self.size_limit * 1.3 and self.depth < 60 \
            and all(self.nest[p] <= lim for p, lim in NEST_LIMIT.items())

    def pick(self, alts, key_prefix, exclude=None):
        """Waehlt eine Alternative: bei knappem Budget die kuerzeste, sonst gewichtet
        zugunsten noch nicht abgedeckter Alternativen."""
        candidates = [(i, alt) for i, alt in enumerate(alts) if not exclude or not exclude(alt)]
        if not candidates:
            candidates = list(enumerate(alts))
        if not self.budget_ok():
            best = min(self.g.seq_size(alt) for _, alt in candidates)
            candidates = [(i, alt) for i, alt in candidates if self.g.seq_size(alt) == best]
        weights = [1.0 / (1 + self.coverage[key_prefix + (i,)]) ** self.cover for i, _ in candidates]
        i, alt = self.rng.choices(candidates, weights=weights)[0]
        self.coverage[key_prefix + (i,)] += 1
        return alt

    def repeat_count(self, key_prefix, minimum=0, cost=1):
        """Wiederholungen eines { … }; ist nur das weiche Budget erschoepft, darf eine
        kleine Wiederholung (Kosten bis 3 Tokens) noch mit 1/4 Wahrscheinlichkeit folgen."""
        if not self.budget_ok():
            if cost <= 3 and self.hard_ok() and self.rng.random() < 0.25:
                return max(minimum, 1)
            return minimum
        options = [0, 1, 2, 3]
        base = [4.0, 3.0, 2.0, 1.0]
        weights = [b / (1 + self.coverage[key_prefix + (min(n, 2),)]) ** self.cover for n, b in zip(options, base)]
        n = max(minimum, self.rng.choices(options, weights=weights)[0])
        self.coverage[key_prefix + (min(n, 2),)] += 1
        return n

    def derive(self, prod, ctx):
        self.nest[prod] += 1
        self.depth += 1
        outermost_expr = prod in ("expr", "tprop") and self.expr_start is None
        if outermost_expr:
            self.expr_start = self.size
        try:
            alts = self.g.trees[prod]
            exclude = self.exclusion(prod, ctx)
            alt = self.pick(alts, (prod,), exclude) if len(alts) > 1 else alts[0]
            return self.sequence(prod, alt, ctx)
        finally:
            self.nest[prod] -= 1
            self.depth -= 1
            if outermost_expr:
                self.expr_start = None

    def exclusion(self, prod, ctx):
        """Alternativen, die an dieser Stelle nicht erzeugt werden duerfen (plan/fuzzer.md, Abschnitt 4)."""
        if prod == "primary":
            forbidden = set(ctx.pop("lead_exclude", ()))
            if ctx.get("no_string"):
                forbidden.add("STRING")
            if forbidden:
                return lambda alt: lead(alt) in forbidden
        return None

    def sequence(self, prod, seq, ctx):
        atoms = []
        angle = ctx["angle"]
        saved = []
        local = dict(ctx)
        prev = None
        for k, f in enumerate(seq):
            kind = f[0]
            if kind == "term":
                text = f[1]
                if prod in ANGLE_PRODS and text == "<":
                    angle += 1
                elif prod in ANGLE_PRODS and text == ">":
                    angle -= 1
                elif text in ("(", "[", "{"):
                    saved.append(angle)
                    angle = 0
                elif text in (")", "]", "}") and saved:
                    angle = saved.pop()
                local["angle"] = angle
                atoms.append(Atom("t", text))
                self.size += 1
                prev = text
            elif kind == "tok":
                atoms.extend(self.token(f[1], prod, prev, local))
                self.size += 1
                prev = f[1]
            elif kind == "nt":
                atoms.extend(self.nonterminal(f[1], prod, local, k, seq, atoms))
                prev = f[1]
            else:
                bracket, alts = f[1], f[2]
                key = self.g.group_ids[id(f)]
                atoms.extend(self.group(prod, bracket, alts, key, local, atoms, seq, k))
        return atoms

    def nonterminal(self, name, prod, ctx, k, seq, atoms_so_far):
        if name == "unit_lit":
            return self.unit_lit(ctx)
        if name == "unit_expr":
            return self.unit_expr(compact=False, ctx=ctx)
        if name in ("number", "int_lit", "duration_lit"):
            return self.derive(name, ctx)
        if name == "postfix":
            return self.postfix(ctx)
        if name == "const_expr" and prod == "generic_arg":
            ctx["lead_exclude"] = ("TYPE_IDENT", "[")
        if name == "const_expr" and prod == "case_pattern":
            ctx["lead_exclude"] = ("UPPER_IDENT",)
        if name == "const_expr" and prod == "campaign_item" and k == 3:
            ctx["lead_exclude"] = ("[",)
        if name in CHAIN_PRODS:
            return self.chain(name, ctx)
        if name in ("not_expr", "unary", "cast_expr", "tprop_not", "expr"):
            return self.wrapped(name, ctx)
        return self.derive(name, ctx)

    def group(self, prod, bracket, alts, key, ctx, atoms_so_far, seq, k):
        if bracket == "(":
            exclude = None
            if ctx.get("angle", 0) > 0:
                exclude = lambda alt: lead(alt) in (">", ">=", ">>")  # noqa: E731
            alt = self.pick(alts, key, exclude)
            return self.sequence(prod, alt, ctx)
        if bracket == "[":
            take = self.optional(prod, key, ctx, atoms_so_far, alts)
            self.coverage[key + (int(take),)] += 1
            return self.sequence(prod, alts[0], ctx) if take else []
        first_of_start = prod == self.g.start and not atoms_so_far
        minimum = 1 if first_of_start or (atoms_so_far and atoms_so_far[-1].kind == "in") else 0
        if first_of_start:
            n = self.rng.randint(1, 6)
            self.coverage[key + (min(n, 2),)] += 1
        else:
            cost = min(self.g.seq_size(alt) for alt in alts)
            n = self.repeat_count(key, minimum, cost)
        out = []
        for _ in range(n):
            if first_of_start:
                self.size = 0  # Budget je Deklaration
            alt = self.pick(alts, key + ("alt",)) if len(alts) > 1 else alts[0]
            out.extend(self.sequence(prod, alt, ctx))
        return out

    def optional(self, prod, key, ctx, atoms_so_far, alts):
        """Entscheidung fuer ein [ … ] mit den Zwaengen aus plan/fuzzer.md, Abschnitt 4."""
        first = lead(alts[0])
        if prod == "fn_decl" and first == "->":
            if not any(a.text == "inout" for a in atoms_so_far):
                return True
        if prod == "primary" and first == "(" and atoms_so_far and atoms_so_far[-1].text == "]":
            return True  # IDENT generic_args verlangt "(" args ")"
        if self.size >= self.size_limit * 1.3:
            return False  # harte Grenze; das Ausdrucksbudget bremst nur Ketten und Rekursion
        weight = 1.0 / (1 + self.coverage[key + (1,)]) ** self.cover
        weight0 = 1.0 / (1 + self.coverage[key + (0,)]) ** self.cover
        return self.rng.random() < weight / (weight + weight0)

    # ------------------------------------------------------------ Ausdruecke mit Klammer-Zwilling

    def chain(self, prod, ctx):
        """Linksassoziative Kette `a op b op c` -> Marker fuer ((a op b) op c)."""
        alts = self.g.trees[prod]
        seq = self.pick(alts, (prod,)) if len(alts) > 1 else alts[0]
        self.nest[prod] += 1
        self.depth += 1
        try:
            rest_group = seq[1] if len(seq) > 1 else None
            if rest_group is None or rest_group[0] != "grp" or rest_group[1] == "(":
                return self.sequence(prod, seq, ctx)  # keine Kette (z. B. `x matches P`)
            first = None
            key = self.g.group_ids[id(rest_group)]
            if rest_group[1] == "[":
                n = int(self.optional(prod, key, ctx, [], rest_group[2]))
                self.coverage[key + (n,)] += 1
            else:
                n = self.repeat_count(key, 0, min(self.g.seq_size(alt) for alt in rest_group[2]))
                if self.nest["expr"] >= 2:
                    n = min(n, 1)  # geschachtelte Ausdruecke bleiben kurz
            if n == 0:
                return self.sequence(prod, [seq[0]], ctx)
            # Klammer-Zwilling: hoechstens PAREN_DEPTH Ebenen, damit die Tiefengrenze des
            # Parsers (MAX_DEPTH) nicht getroffen wird
            mark = ctx.get("paren", 0) + n <= PAREN_DEPTH
            inner = dict(ctx, paren=ctx.get("paren", 0) + (n if mark else 0))
            atoms = ([Atom("po")] * n if mark else []) + self.sequence(prod, [seq[0]], inner)
            for _ in range(n):
                alt = self.pick(rest_group[2], key + ("alt",)) if len(rest_group[2]) > 1 else rest_group[2][0]
                atoms.extend(self.sequence(prod, alt, inner))
                if mark:
                    atoms.append(Atom("pc"))
            return atoms
        finally:
            self.nest[prod] -= 1
            self.depth -= 1

    def wrapped(self, prod, ctx):
        """not/unaer/cast/bedingt: der ganze Ausdruck in Marker, wenn er zusammengesetzt ist."""
        if ctx.get("paren", 0) >= PAREN_DEPTH:
            return self.derive(prod, ctx)
        ctx = dict(ctx, paren=ctx.get("paren", 0) + 1)
        atoms = self.derive(prod, ctx)
        composite = (prod in ("not_expr", "unary") and atoms and atoms[0].text in ("not", "-", "~")) or \
                    (prod == "cast_expr" and any(a.text == "as" for a in atoms)) or \
                    (prod == "expr" and any(a.text == "if" for a in atoms))
        if composite:
            return [Atom("po")] + atoms + [Atom("pc")]
        return atoms

    def postfix(self, ctx):
        """`primary { … }`: hinter einem Zahlen- oder Dauerliteral steht kein `.`."""
        self.nest["postfix"] += 1
        try:
            seq = self.g.trees["postfix"][0]
            group = seq[1]
            key = self.g.group_ids[id(group)]
            n = self.repeat_count(key)
            if n > 0:
                ctx["lead_exclude"] = tuple(ctx.get("lead_exclude", ())) + ("number", "duration_lit")
            atoms = self.sequence("postfix", [seq[0]], ctx)
            for _ in range(n):
                alt = self.pick(group[2], key + ("alt",))
                atoms.extend(self.sequence("postfix", alt, ctx))
            return atoms
        finally:
            self.nest["postfix"] -= 1

    # ------------------------------------------------------------ Einheiten

    def unit_term(self, first, compact):
        name = self.rng.choice(UNIT_NAMES + UPPER_POOL[:4] + ["KiB", "MiB"])
        atoms = [Atom("t", name, glue=not first, space=first)]
        if self.rng.random() < 0.2:
            atoms.append(Atom("t", "^", glue=True))
            atoms.append(Atom("t", str(self.rng.randint(1, 3)), glue=True))
        return atoms

    def unit_expr(self, compact, ctx):
        atoms = []
        inverse = self.rng.random() < 0.15
        if inverse:
            atoms.append(Atom("t", "1", space=compact))
            atoms.append(Atom("t", "/", glue=compact))
            atoms.extend(self.unit_term(False, compact))
        else:
            atoms.extend(self.unit_term(True, compact))
        for _ in range(self.rng.choice([0, 0, 0, 1, 2])):
            atoms.append(Atom("t", self.rng.choice(["*", "/"]), glue=compact))
            atoms.extend(self.unit_term(False, compact))
        if not compact:
            for a in atoms:
                a.glue = a.space = False
        return atoms

    def unit_lit(self, ctx):
        """Einheit nach einer Zahl: kompakt, nie ein einzelnes Zeitsuffix, nie kontextuell."""
        while True:
            atoms = self.unit_expr(compact=True, ctx=ctx)
            names = [a.text for a in atoms if a.text not in ("*", "/", "^", "1") and not a.text.isdigit()]
            if len(atoms) == 1 and atoms[0].text in TIME_SUFFIXES:
                continue
            if any(n in self.g.contextual or n in self.g.keywords for n in names):
                continue
            atoms[0].space = True
            atoms[0].glue = False
            for a in atoms[1:]:
                a.glue = True
            return atoms

    # ------------------------------------------------------------ Tokens

    def word(self, kind, prev):
        for _ in range(100):
            if kind == "IDENT":
                w = self.rng.choice(IDENT_POOL) if self.rng.random() < 0.7 else self.random_word("[a-z]", "[a-z0-9_]")
                if self.contextual and self.rng.random() < 0.15:
                    # Typwoerter sind ueberall Typen, wo ein Typ stehen kann (Generik, nach `as`)
                    w = self.rng.choice(sorted(self.g.contextual - SCALAR_WORDS - TYPE_WORDS))
            elif kind == "UPPER_IDENT":
                w = self.rng.choice(UPPER_POOL) if self.rng.random() < 0.7 else self.random_word("[A-Z]", "[A-Z0-9_]")
            else:
                w = self.rng.choice(TYPE_POOL) if self.rng.random() < 0.7 else self.random_word("[A-Z]", "[a-z]") + \
                    self.random_word("[a-z]", "[a-zA-Z0-9]")
            klass = pc.classify(w, self.g.keywords, self.g.reserved)
            want = {"IDENT": "IDENT", "UPPER_IDENT": "UPPER", "TYPE_IDENT": "TYPE"}[kind]
            if klass != want:
                continue
            if w in self.g.contextual and not (self.contextual and kind == "IDENT"):
                continue
            if prev == "as" and w in SCALAR_WORDS:
                continue
            return w
        raise RuntimeError("kein Name gefunden")

    def random_word(self, first, rest):
        chars = {"[a-z]": "abcdefghijklmnopqrstuvwxyz", "[A-Z]": "ABCDEFGHIJKLMNOPQRSTUVWXYZ",
                 "[a-z0-9_]": "abcdefghijklmnopqrstuvwxyz0123456789_", "[A-Z0-9_]": "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_",
                 "[a-zA-Z0-9]": "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"}
        n = self.rng.randint(0, 6)
        return self.rng.choice(chars[first]) + "".join(self.rng.choice(chars[rest]) for _ in range(n))

    def number(self, kind):
        r = self.rng
        if kind == "INT":
            return r.choice(["0", "1", "7", "42", "255", "1_000", "65536", "3"])
        if kind == "HEX":
            return r.choice(["0x1F", "0xFF", "0x40000000", "0xA5_5A", "0x0"])
        if kind == "BIN":
            return r.choice(["0b1010", "0b1", "0b1111_0000"])
        if kind == "OCT":
            return r.choice(["0o17", "0o755"])
        return r.choice(["4.25", "0.5", "1e3", "1e-3", "2.5E3", "1_000.5", "9.81", "0.0005"])

    def duration(self):
        r = self.rng
        if r.random() < 0.7:
            return f"{r.choice(['0', '1', '3', '20', '200', '1500'])} {r.choice(sorted(TIME_SUFFIXES))}"
        return f"{r.choice(['0.5', '1.5', '2.5'])} {r.choice(['us', 'ms', 's', 'min', 'h'])}"

    def string(self, prod, ctx):
        kind = STRING_KIND.get(prod)
        if kind == "pattern_text":
            body = self.subtext("pattern_text", ctx)
        elif kind == "address_text":
            body = self.subtext("address_text", ctx)
        elif kind == "format_text":
            body = self.subtext("format_text", ctx)
        else:
            body = self.plain_text()
        return '"' + body.replace("\\", "\\\\").replace('"', '\\"') + '"'

    def plain_text(self):
        words = ["ok", "DC bus", "Phase U", "x=", "42", "a b c", "Temperatur", "", "tab\\there"]
        text = self.rng.choice(words)
        if self.rng.random() < 0.2:
            text += "\\n"
        return text

    def subtext(self, start, ctx):
        """Teilsprache in einem String, mit eigenem Budget (Strings stehen oft tief in Ausdruecken)."""
        saved = (self.size, self.expr_start, dict(self.nest), self.depth)
        self.size, self.expr_start, self.depth = 0, None, 0
        self.nest.clear()
        try:
            atoms = self.derive(start, dict(ctx, no_string=True, angle=0))
        finally:
            self.size, self.expr_start, nest, self.depth = saved
            self.nest.clear()
            self.nest.update(nest)
        return "".join(a.text for a in atoms if a.kind == "t")

    def token(self, name, prod, prev, ctx):
        r = self.rng
        if name == "NEWLINE":
            return [Atom("nl")]
        if name == "INDENT":
            return [Atom("in")]
        if name == "DEDENT":
            return [Atom("de")]
        if name in ("IDENT", "UPPER_IDENT", "TYPE_IDENT"):
            return [Atom("t", self.word(name, prev))]
        if name == "KEYWORD":
            return [Atom("t", r.choice(sorted(self.g.keywords)))]
        if name in ("INT", "HEX", "BIN", "OCT", "FLOAT"):
            return [Atom("t", self.number(name))]
        if name == "DURATION":
            return [Atom("t", self.duration())]
        if name == "STRING":
            return [Atom("t", self.string(prod, ctx))]
        if name == "TEXT_CHAR":
            return [Atom("t", r.choice("abcdefghijklmnopqrstuvwxyz0123456789 .,:;-=+*!?/()[]"))]
        if name == "ADDR_WORD":
            return [Atom("t", r.choice(["adc1", "gpio", "uart0", "i2c1", "0x6B", "tc", "daq_1", "ai.0", "cap-0"]))]
        raise RuntimeError(f"unbekanntes Token {name}")


# ---------------------------------------------------------------- Rendering

def wordy(c):
    return c.isalnum() or c == "_" or c == '"'


def separator(prev, atom, mode, rng):
    """Leerraum zwischen zwei Tokens je Fassung; None = Umbruch erlaubt."""
    if prev is None:
        return ""
    if atom.glue:
        return ""
    if atom.space:
        return " " if mode != "max" else "  "
    a, b = prev.text, atom.text
    in_unit = prev.glue or prev.space  # prev gehoert zu einem Einheitenliteral
    joinable = not (wordy(a[-1]) and wordy(b[0])) and (a[-1] + b[0]) not in TWO_CHAR_OPS \
        and not (b in ("*", "/", "^") and (" " in a or a[-1].isalpha() or in_unit)) \
        and not (a[-1].isdigit() and b[0] == ".") \
        and not (a[0].isdigit() and b == "/")  # `1/s` waere ein Einheitenliteral
    if mode == "min":
        return "" if joinable else " "
    if mode == "max":
        return "  " if rng.random() < 0.5 else " "
    return " "


def render(atoms, mode, rng, noise=False):
    """Atome zu Text. mode: canonical | min | max | paren; noise: Fortsetzungszeilen,
    Kommentare und Leerzeilen (nur mit mode canonical sinnvoll)."""
    out = []
    depth = 0
    at_line_start = True
    brackets = 0
    prev = None
    line_has_code = False
    for atom in atoms:
        if atom.kind == "nl":
            if noise and line_has_code and rng.random() < 0.2:
                out.append("  # nachgestellt")
            out.append("\n")
            at_line_start = True
            line_has_code = False
            prev = None
            if noise and rng.random() < 0.15:
                out.append("\n")
            if noise and rng.random() < 0.15:
                out.append(" " * (4 * depth + rng.choice([0, 4])) + "#Kommentar\n")
            continue
        if atom.kind == "in":
            depth += 1
            continue
        if atom.kind == "de":
            depth -= 1
            if noise and rng.random() < 0.1:
                out.append(" " * (4 * (depth + 1)) + "# am Blockende\n")
            continue
        if atom.kind in ("po", "pc"):
            if mode != "paren":
                continue
            text = "(" if atom.kind == "po" else ")"
            atom = Atom("t", text, glue=(atom.kind == "pc"))
        text = atom.text
        if at_line_start:
            out.append(" " * (4 * depth))
            at_line_start = False
        else:
            sep = separator(prev, atom, mode, rng)
            if noise and brackets > 0 and sep != "" and rng.random() < 0.15:
                sep = "\n" + " " * (4 * depth + rng.choice([2, 4, 8]))
                if rng.random() < 0.3:
                    sep = "  # in Klammern" + sep
            out.append(sep)
        out.append(text)
        line_has_code = True
        prev = atom
        if text in ("(", "[", "{"):
            brackets += 1
        elif text in (")", "]", "}"):
            brackets -= 1
    text = "".join(out)
    if not text.endswith("\n"):
        text += "\n"
    return text


# ---------------------------------------------------------------- Werkzeuge

def batches(items):
    for i in range(0, len(items), BATCH):
        yield items[i:i + BATCH]


def run_parse(paths, snippet):
    """Rust-Parser: Pfad -> (ok, Baum oder Fehlermeldung). Stuerzt der Parser bei
    einer Datei ab, wird der Stapel halbiert, bis der Absturz einer Datei zugeordnet ist."""
    result = {}
    for chunk in batches(paths):
        parse_chunk(chunk, snippet, result)
    return result


def parse_chunk(chunk, snippet, result):
    cmd = ["cargo", "run", "-q", "-p", "takt-syntax", "--example", "parse", "--", "--ast"]
    if snippet:
        cmd.append("--snippet")
    r = subprocess.run(cmd + chunk, capture_output=True, text=True, encoding="utf-8", cwd=ROOT)
    seen = {}
    tree = []
    for line in r.stdout.splitlines():
        done = False
        for p in chunk:
            if line.startswith(p + ": "):
                rest = line[len(p) + 2:]
                if rest.startswith("ok ("):
                    seen[p] = (True, "\n".join(tree))
                else:
                    seen.setdefault(p, (False, rest))
                tree = []
                done = True
                break
        if not done:
            tree.append(line)
    missing = [p for p in chunk if p not in seen]
    if missing and len(chunk) > 1:
        half = len(chunk) // 2
        parse_chunk(chunk[:half], snippet, result)
        parse_chunk(chunk[half:], snippet, result)
        return
    result.update(seen)
    for p in missing:
        crash = r.stderr.strip().splitlines()
        result[p] = (False, "Parser abgestuerzt: " + (crash[-1] if crash else "keine Ausgabe"))


def run_oracle(paths, snippet):
    ok = set()
    for chunk in batches(paths):
        cmd = [sys.executable, "-X", "utf8", str(ROOT / "grammar" / "parse_corpus.py")]
        if snippet:
            cmd.append("--snippet")
        r = subprocess.run(cmd + chunk, capture_output=True, text=True, encoding="utf-8", cwd=ROOT)
        for line in r.stdout.splitlines():
            if line.startswith("[OK"):
                ok.add(line.split("] ", 1)[1].split("  (")[0])
    return ok


def run_fmt(paths, snippet, verify):
    """fmt --verify: Pfad -> Meldung; ohne verify formatiert in place."""
    problems = {}
    for chunk in batches(paths):
        cmd = ["cargo", "run", "-q", "-p", "takt-syntax", "--example", "fmt", "--"]
        if verify:
            cmd.append("--verify")
        if snippet:
            cmd.append("--snippet")
        r = subprocess.run(cmd + chunk, capture_output=True, text=True, encoding="utf-8", cwd=ROOT)
        for line in (r.stdout + r.stderr).splitlines():
            path, _, rest = line.partition(": ")
            if verify and rest != "ok":
                problems[path] = rest
            elif not verify and rest not in ("formatiert", ""):
                problems[path] = rest
    return problems


def single_ok(text, snippet, check):
    """Prueft eine Datei allein (fuer die Verkleinerung)."""
    with tempfile.NamedTemporaryFile("w", suffix=".takt", delete=False, encoding="utf-8", newline="\n") as fh:
        fh.write(text)
        path = fh.name
    try:
        if check == "parse":
            return run_parse([path], snippet)[path][0]
        return path not in run_fmt([path], snippet, True)
    finally:
        os.unlink(path)


def minimize(text, snippet, check):
    """Loescht Zeilen samt eingerueckten Folgezeilen, solange der Fehler bleibt."""
    lines = text.split("\n")
    changed = True
    while changed:
        changed = False
        i = 0
        while i < len(lines):
            indent = len(lines[i]) - len(lines[i].lstrip(" "))
            j = i + 1
            while j < len(lines) and (not lines[j].strip() or len(lines[j]) - len(lines[j].lstrip(" ")) > indent):
                j += 1
            candidate = lines[:i] + lines[j:]
            if "\n".join(candidate).strip() and not single_ok("\n".join(candidate) + "\n", snippet, check):
                lines = candidate
                changed = True
            else:
                i = j if j > i + 1 and False else i + 1
    return "\n".join(lines).rstrip("\n") + "\n"


# ---------------------------------------------------------------- Laeufer

VARIANTS = ("c", "min", "max", "paren", "noise")


def main(argv):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--count", type=int, default=100)
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--start", choices=["file", "snippet"], default="file")
    ap.add_argument("--contextual", action="store_true", help="kontextuelle Woerter auch als Bezeichner")
    ap.add_argument("--no-oracle", action="store_true")
    ap.add_argument("--cover", type=int, default=2, help="Staerke der Abdeckungssteuerung")
    ap.add_argument("--keep", default=str(ROOT / "out" / "fuzz"), help="Ablage fuer Fehlschlaege")
    args = ap.parse_args(argv)

    grammar = Grammar(args.start)
    rng = random.Random(args.seed)
    gen = Generator(grammar, rng, contextual=args.contextual, cover=args.cover)
    snippet = args.start == "snippet"
    work = Path(tempfile.mkdtemp(prefix="takt-fuzz-"))
    programs = []
    for i in range(args.count):
        atoms = gen.program()
        texts = {
            "c": render(atoms, "canonical", rng),
            "min": render(atoms, "min", rng),
            "max": render(atoms, "max", rng),
            "paren": render(atoms, "paren", rng),
            "noise": render(atoms, "canonical", rng, noise=True),
        }
        paths = {}
        for v, text in texts.items():
            p = work / f"{args.seed}_{i:04d}_{v}.takt"
            io.open(p, "w", encoding="utf-8", newline="\n").write(text)
            paths[v] = str(p)
        programs.append((i, texts, paths))

    all_paths = [p for _, _, paths in programs for p in paths.values()]
    parsed = run_parse(all_paths, snippet)
    oracle_ok = set() if args.no_oracle else run_oracle(all_paths, snippet)
    fmt_problems = run_fmt(all_paths, snippet, True)
    # Formatterausgabe der Leerraumfassungen vergleichen: Kopien in place formatieren
    copies = {}
    for i, texts, paths in programs:
        for v in ("c", "min", "max"):
            p = work / f"{args.seed}_{i:04d}_{v}_fmt.takt"
            shutil.copyfile(paths[v], p)
            copies[(i, v)] = str(p)
    run_fmt(list(copies.values()), snippet, False)

    failures = []
    for i, texts, paths in programs:
        problems = []
        trees = {}
        for v in VARIANTS:
            ok, info = parsed[paths[v]]
            if not ok:
                problems.append(f"Parser lehnt Fassung {v} ab: {info}")
            else:
                trees[v] = info
            if not args.no_oracle and paths[v] not in oracle_ok:
                problems.append(f"Orakel lehnt Fassung {v} ab")
            if paths[v] in fmt_problems:
                problems.append(f"Formatter, Fassung {v}: {fmt_problems[paths[v]]}")
        if len(set(trees.values())) > 1:
            # einzeln nachpruefen, damit kein Stapelartefakt gemeldet wird
            single = {v: run_parse([paths[v]], snippet)[paths[v]][1] for v in trees}
            differing = [v for v in VARIANTS if v in single and single[v] != single.get("c")]
            if differing:
                problems.append(f"Baum weicht ab in Fassung(en) {', '.join(differing)}")
        formatted = {v: io.open(copies[(i, v)], encoding="utf-8").read() for v in ("c", "min", "max")}
        if len(set(formatted.values())) > 1:
            problems.append("Formatterausgabe der Leerraumfassungen weicht ab")
        if problems:
            failures.append((i, texts, problems))

    keep = Path(args.keep)
    if failures:
        keep.mkdir(parents=True, exist_ok=True)
    for i, texts, problems in failures:
        print(f"[FEHLER] Programm {args.seed}_{i:04d}")
        for p in problems:
            print(f"    {p}")
        check = "parse" if any(p.startswith("Parser") for p in problems) else "fmt"
        small = minimize(texts["c"], snippet, check) if check == "parse" or any(
            p.startswith("Formatter, Fassung c") for p in problems) else texts["c"]
        name = keep / f"{args.seed}_{i:04d}.takt"
        io.open(name, "w", encoding="utf-8", newline="\n").write(small)
        for v in VARIANTS:
            io.open(keep / f"{args.seed}_{i:04d}_{v}.takt", "w", encoding="utf-8", newline="\n").write(texts[v])
        print(f"    abgelegt: {name} (verkleinert) und Fassungen; nachstellen mit --seed {args.seed}")

    covered = {k for k in gen.coverage if gen.coverage[k] > 0}
    wanted = set(grammar.alt_keys)
    hit = {k[:2] if len(k) > 2 and isinstance(k[1], str) and k[1].startswith("g") else k for k in covered}
    missing = sorted(k for k in wanted if k not in hit and not (isinstance(k[1], str) and k[1].startswith("g")))
    missing_groups = sorted(k for k in wanted if isinstance(k[1], str) and k[1].startswith("g")
                            and not any(c[:2] == k for c in covered))
    print(f"\n{args.count} Programme x {len(VARIANTS)} Fassungen, {len(failures)} fehlerhaft; "
          f"Alternativen: {len(wanted) - len(missing) - len(missing_groups)} von {len(wanted)} getroffen")
    if missing or missing_groups:
        print("nicht getroffen: " + ", ".join(f"{p}[{a}]" for p, a in missing + missing_groups))
    shutil.rmtree(work, ignore_errors=True)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
