#!/usr/bin/env python3
"""Fuehrt grammar/takt.ebnf als Parser aus und prueft damit Takt-Dateien.

    python grammar/parse_corpus.py DATEI...          parst jede Datei, meldet OK oder Position
    python grammar/parse_corpus.py --tokens DATEI    zeigt den Tokenstrom

Der Tokenizer folgt grammar/lexer.md; der Parser ist ein memoisierter
Backtracking-Interpreter ueber die EBNF, der jede Ableitung zulaesst. Er ist
kein Ersatz fuer den Parser in takt-syntax, sondern das Orakel dafuer: Was er
akzeptiert, akzeptiert die Grammatik; was er ablehnt, lehnt sie ab.
"""
from __future__ import annotations

import io
import re
import sys
from fractions import Fraction
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import check_grammar as cg  # noqa: E402

sys.setrecursionlimit(20000)

TIME = {"ns": 1, "us": 10**3, "ms": 10**6, "s": 10**9, "min": 60 * 10**9,
        "h": 3600 * 10**9, "d": 86400 * 10**9}
OPERATORS = ["->", "..", "+=", "-=", "*=", "/=", "==", "!=", "<=", ">=", "<<", ">>",
             "+", "-", "*", "/", "%", "&", "|", "^", "~", "<", ">", "=", ".", ",", ":",
             "(", ")", "[", "]", "{", "}", "@", "?", "!"]
NUMBER_RE = re.compile(r"0x[0-9a-fA-F_]+|0b[01_]+|0o[0-7_]+|[0-9][0-9_]*(\.[0-9][0-9_]*)?([eE][+-]?[0-9]+)?")
WORD_RE = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")
ESCAPES = {"\\": "\\", '"': '"', "n": "\n", "t": "\t", "r": "\r", "0": "\0"}


class LexError(Exception):
    def __init__(self, code, line, col, detail=""):
        super().__init__(f"{code} in Zeile {line}, Spalte {col}{': ' + detail if detail else ''}")
        self.code, self.line, self.col = code, line, col


class Token:
    __slots__ = ("kind", "text", "line", "col")

    def __init__(self, kind, text, line, col):
        self.kind, self.text, self.line, self.col = kind, text, line, col

    def __repr__(self):
        return f"{self.kind}({self.text})" if self.kind not in ("OP", "NEWLINE", "INDENT", "DEDENT") else (
            self.text if self.kind == "OP" else self.kind)


def classify(word, keywords, reserved):
    if word in keywords:
        return "KW"
    if word in reserved:
        return "RESERVED"
    if word == "_":
        return "WILD"
    if word[0].islower() or word[0] == "_":
        return "IDENT"
    if re.fullmatch(r"[A-Z][A-Z0-9_]*", word):
        return "UPPER"
    return "TYPE"


def tokenize(text, keywords, reserved):
    if text.startswith("﻿"):
        raise LexError("E_BOM", 1, 1)
    text = text.replace("\r\n", "\n")
    if "\r" in text:
        raise LexError("E_CR", text[:text.index("\r")].count("\n") + 1, 1)
    tokens, stack, depth = [], [0], 0
    for lineno, line in enumerate(text.split("\n"), start=1):
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        pos = len(line) - len(line.lstrip(" "))
        if "\t" in line[:pos]:
            raise LexError("E_TAB", lineno, line.index("\t") + 1)
        if depth == 0:
            if pos % 4:
                raise LexError("E_INDENT", lineno, 1, f"Einrueckung {pos}")
            if pos > stack[-1]:
                if pos != stack[-1] + 4:
                    raise LexError("E_INDENT", lineno, 1, f"Sprung von {stack[-1]} auf {pos}")
                stack.append(pos)
                tokens.append(Token("INDENT", "", lineno, 1))
            while pos < stack[-1]:
                stack.pop()
                tokens.append(Token("DEDENT", "", lineno, 1))
            if pos != stack[-1]:
                raise LexError("E_DEDENT", lineno, 1)
        while pos < len(line):
            c = line[pos]
            if c == " ":
                pos += 1
                continue
            if c == "\t":
                raise LexError("E_TAB", lineno, pos + 1)
            if c == "#":
                break
            col = pos + 1
            if ord(c) > 127:
                raise LexError("E_NONASCII", lineno, col, repr(c))
            if c == '"':
                out, pos = [], pos + 1
                while True:
                    if pos >= len(line):
                        raise LexError("E_STRING", lineno, col)
                    ch = line[pos]
                    if ch == '"':
                        pos += 1
                        break
                    if ch == "\\":
                        if pos + 1 >= len(line) or line[pos + 1] not in ESCAPES:
                            raise LexError("E_ESCAPE", lineno, pos + 1)
                        out.append(ESCAPES[line[pos + 1]])
                        pos += 2
                    else:
                        out.append(ch)
                        pos += 1
                tokens.append(Token("STRING", "".join(out), lineno, col))
                continue
            if c.isdigit():
                m = NUMBER_RE.match(line, pos)
                text_num = m.group(0)
                end = m.end()
                if end < len(line) and (line[end].isalnum() or line[end] == "_"):
                    raise LexError("E_UNIT_SPACE" if line[end].isalpha() else "E_NUMBER", lineno, end + 1)
                if text_num.endswith("_") or re.match(r"0[xbo]_", text_num):
                    raise LexError("E_NUMBER", lineno, col)
                kind = ("HEX" if text_num.startswith("0x") else "BIN" if text_num.startswith("0b")
                        else "OCT" if text_num.startswith("0o") else
                        "FLOAT" if re.search(r"[.eE]", text_num) else "INT")
                # Dauer: Zahl, Leerraum, genau ein Zeitsuffix, danach kein * / ^ (L4.4)
                after = re.match(r" +([a-z]+)", line[end:])
                if kind in ("INT", "FLOAT") and after and after.group(1) in TIME:
                    nxt = end + after.end()
                    if nxt >= len(line) or line[nxt] not in "*/^" and not (line[nxt].isalnum() or line[nxt] == "_"):
                        value = Fraction(text_num.replace("_", "")) * TIME[after.group(1)]
                        if value.denominator != 1 or abs(value) >= 2**63:
                            raise LexError("E_DURATION", lineno, col, f"{text_num} {after.group(1)}")
                        tokens.append(Token("DURATION", str(value.numerator), lineno, col))
                        pos = nxt
                        continue
                tokens.append(Token(kind, text_num, lineno, col))
                pos = end
                continue
            if c.isalpha() or c == "_":
                m = WORD_RE.match(line, pos)
                word = m.group(0)
                tokens.append(Token(classify(word, keywords, reserved), word, lineno, col))
                pos = m.end()
                continue
            for op in OPERATORS:
                if line.startswith(op, pos):
                    if op == ">>":
                        # L6.1: ">>" ist ein Token, das der Parser in Typen teilt. Hier als
                        # zwei verklebte ">" dargestellt; ">>" als Operator verlangt beide.
                        tokens.append(Token("OP", ">", lineno, col))
                        tokens.append(Token("OP>", ">", lineno, col + 1))
                    else:
                        tokens.append(Token("OP", op, lineno, col))
                    pos += len(op)
                    if op in "([{":
                        depth += 1
                    elif op in ")]}":
                        depth = max(0, depth - 1)
                    break
            else:
                raise LexError("E_CHAR", lineno, col, repr(c))
        if depth == 0:
            tokens.append(Token("NEWLINE", "", lineno, len(line) + 1))
    if depth > 0:
        raise LexError("E_UNCLOSED", lineno, 1)
    while len(stack) > 1:
        stack.pop()
        tokens.append(Token("DEDENT", "", lineno, 1))
    return tokens


TOKEN_KINDS = {"IDENT": "IDENT", "UPPER_IDENT": "UPPER", "TYPE_IDENT": "TYPE", "KEYWORD": "KW",
               "INT": "INT", "HEX": "HEX", "BIN": "BIN", "OCT": "OCT", "FLOAT": "FLOAT",
               "STRING": "STRING", "DURATION": "DURATION", "NEWLINE": "NEWLINE",
               "INDENT": "INDENT", "DEDENT": "DEDENT"}
WORD_KINDS = {"KW", "IDENT", "UPPER", "TYPE"}


def terminal_matches(term, tok):
    if term == "_":
        return tok.kind == "WILD"
    if tok.kind in ("OP", "OP>"):
        return tok.text == term
    if tok.kind in WORD_KINDS:
        return tok.text == term
    if tok.kind == "INT" and term == "1":
        return tok.text == "1"
    return False


class Parser:
    def __init__(self, prods, tokens):
        self.trees = {name: cg.parse_rhs(cg.TOKEN_RE.findall(p["rhs"])) for name, p in prods.items()}
        self.tokens = tokens
        self.memo = {}
        self.far, self.expected = 0, set()

    def fail(self, i, what):
        if i > self.far:
            self.far, self.expected = i, {what}
        elif i == self.far:
            self.expected.add(what)

    def nt(self, name, i):
        key = (name, i)
        if key in self.memo:
            return self.memo[key]
        self.memo[key] = ()
        result = set()
        for seq in self.trees[name]:
            result |= self.seq(seq, i)
        self.memo[key] = tuple(sorted(result))
        return self.memo[key]

    def seq(self, seq, i):
        positions = {i}
        for f in seq:
            nxt = set()
            for p in positions:
                nxt |= self.factor(f, p)
            positions = nxt
            if not positions:
                break
        return positions

    def factor(self, f, i):
        kind = f[0]
        if kind == "term":
            if f[1] == ">>":
                if (i + 1 < len(self.tokens) and self.tokens[i].kind == "OP"
                        and self.tokens[i].text == ">" and self.tokens[i + 1].kind == "OP>"):
                    return {i + 2}
                self.fail(i, '">>"')
                return set()
            if i < len(self.tokens) and terminal_matches(f[1], self.tokens[i]):
                return {i + 1}
            self.fail(i, '"' + f[1] + '"')
            return set()
        if kind == "tok":
            if i < len(self.tokens) and self.tokens[i].kind == TOKEN_KINDS.get(f[1], f[1]):
                return {i + 1}
            self.fail(i, f[1])
            return set()
        if kind == "nt":
            return set(self.nt(f[1], i))
        bracket, alts = f[1], f[2]
        if bracket == "(":
            out = set()
            for alt in alts:
                out |= self.seq(alt, i)
            return out
        if bracket == "[":
            out = {i}
            for alt in alts:
                out |= self.seq(alt, i)
            return out
        result, frontier = {i}, {i}
        while frontier:
            new = set()
            for p in frontier:
                for alt in alts:
                    new |= self.seq(alt, p)
            new -= result
            result |= new
            frontier = new
        return result

    def parse(self, start="file"):
        ends = self.nt(start, 0)
        return len(self.tokens) in ends


# Startsymbol fuer Schnipsel aus der Referenz: Deklarationen, Zustandsinhalte,
# Sequenzschritte und Anweisungen in beliebiger Folge.
SNIPPET_RHS = ("{ NEWLINE | import | system_decl | type_decl | unitvec_decl | enum_decl | record_decl | unit_decl"
               " | stream_decl | port_decl | node_decl | property_decl | const_decl | param_decl | profile_decl"
               " | channel_decl | command_decl | fn_decl | native_decl | block_decl | machine_decl | instance_decl"
               " | scenario_decl | campaign_decl | trigger_decl"
               " | fault_clause | persist_decl | signal_decl | \"initial\" UPPER_IDENT NEWLINE"
               " | enter_block | exit_block | loop_block | on_handler | sequence_block | transition | state_decl"
               " | seq_item | step_decl }")


def main(argv):
    show_tokens = "--tokens" in argv
    snippet = "--snippet" in argv
    files = [a for a in argv if not a.startswith("--")]
    keywords, reserved = cg.keywords_from_reference()
    keywords = set(keywords)
    reserved = set(reserved)
    _, _, _, prods, _ = cg.load()
    if snippet:
        prods["snippet"] = {"rhs": SNIPPET_RHS, "stage": "", "start": True, "checks": []}
    start = "snippet" if snippet else "file"
    failures = 0
    for path in files:
        text = io.open(path, encoding="utf-8").read()
        try:
            tokens = tokenize(text, keywords, reserved)
        except LexError as e:
            print(f"[LEXFEHLER] {path}: {e}")
            failures += 1
            continue
        if show_tokens:
            # ART<TAB>Text<TAB>~ wie crates/takt-syntax/examples/tokens.rs, fuer den Differenzvergleich
            for t in tokens:
                text = "" if t.kind in ("NEWLINE", "INDENT", "DEDENT") else t.text
                kind = {"OP>": "Op", "OP": "Op", "UPPER": "UpperIdent", "TYPE": "TypeIdent", "KW": "Keyword",
                        "STRING": "Str", "DURATION": "Duration"}.get(t.kind, t.kind.capitalize())
                print(f"{kind}	{text}")
            print("Eof	")
        bad = [t for t in tokens if t.kind == "RESERVED"]
        if bad:
            t = bad[0]
            print(f"[LEXFEHLER] {path}: E_RESERVED '{t.text}' in Zeile {t.line}, Spalte {t.col}")
            failures += 1
            continue
        parser = Parser(prods, tokens)
        if parser.parse(start):
            print(f"[OK      ] {path}  ({len(tokens)} Tokens)")
        else:
            failures += 1
            far = parser.far
            tok = tokens[far] if far < len(tokens) else None
            where = f"Zeile {tok.line}, Spalte {tok.col}, Token {tok!r}" if tok else "Dateiende"
            exp = ", ".join(sorted(parser.expected))[:160]
            print(f"[PARSEFEHLER] {path}: {where}; erwartet: {exp}")
    print(f"\n{len(files)} Dateien, {failures} abgelehnt")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
