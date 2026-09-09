#!/usr/bin/env python3
"""Abdeckung der Schreibweisen: Welche Nuance der Grammatik kommt in keinem Beispiel vor?

    python corpus-try/coverage.py            prueft corpus-try/*.takt und corpus-try/ref/*.takt

Jede Zeile von NUANCES ist (Bezeichnung, Regex). Die Liste folgt der Inventur:
Konstrukte aus 2.4, Typformen aus 3.x, Statements und Deklarationen aus 2.3,
Attribute, Literale. Sie ist bewusst redundant zur Grammatik: Der Grammatikpruefer
sagt, was erlaubt ist; diese Liste sagt, was ein Beispiel tatsaechlich benutzt.
"""
from __future__ import annotations

import glob
import io
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))

NUANCES = [
    # --- system ---
    ("system tick", r"^\s*tick = "), ("system language", r"^\s*language = "),
    ("system output_timing boundary", r"output_timing = boundary"), ("system output_timing asap", r"output_timing = asap"),
    ("system fault_is_fail", r"fault_is_fail = "), ("system tick_source", r"tick_source = hw\("),
    ("system tick_tolerance for N ticks", r"tick_tolerance = .* for \d+ ticks"), ("system target", r"^\s*target = "),
    ("system float = f32", r"float = f32"),
    # --- Deklarationen ---
    ("import Modul as", r"^import [a-z_.]+ as "), ("import channels from", r"^import channels from "),
    ("type-Alias", r"^type [A-Z]\w* = "), ("unit mit Faktor", r"^unit \w+ = [\d.]+ [A-Za-z]"),
    ("unit dimensionslos", r"^unit \w+ = [\d.]+\s*(#|$)"), ("unit affine", r"^unit \w+ = affine\("),
    ("unitvec", r"^unitvec [A-Z]"), ("enum einzeilig", r"^enum \w+: [A-Z]"), ("enum mehrzeilig", r"^enum \w+:\s*$"),
    ("enum layout u8 mit Diskriminanten", r"^enum \w+ layout u8: \w+ = 0x"), ("enum open", r"^enum \w+ open:"),
    ("Variante mit Feldern", r"\b[A-Z_]+\([a-z_]+: "), ("Variantenfeld mit Range", r"\b[A-Z_]+\([^)]*\bin \d"),
    ("record ohne layout", r"^record \w+:\s*$"), ("record layout little", r"^record \w+ layout little"),
    ("record layout big", r"layout big"), ("record align", r"layout \w+, align = "),
    ("Konstantenfeld", r"^\s+\w+\s*: u\d+ = 0x"), ("Feld mit offset", r"\boffset = \d"),
    ("Padding _", r"^\s+_\s*: \["), ("Bitfelder with bits", r"with bits:"), ("Bitbereich at a..b", r"\bat \d+\.\.\d+"),
    ("len_field", r"with len = \w+"), ("Array-Feld [N] u8", r": \[\d+\] u8"),
    ("const mit Typ", r"^const \w+ : "), ("const ohne Typ", r"^const \w+ = "), ("table-Konstante", r"table<.*> = \[\("),
    ("param Duration", r"^param \w+\s*: Duration"), ("param mit Range und Einheit", r"^param \w+\s*: \w+\[\w+\] in "),
    ("tunable param", r"^tunable param "), ("profile", r"^profile [A-Z]"),
    ("input mit max_age", r"^input .* with .*max_age = "), ("output mit safe", r"^output .* safe = "),
    ("sim-Bindung", r'@ sim\("'), ("none-Bindung", r"@ none\b"), ("Channel-Array [16]", r"^input\s+\w+\s*: \[\d+\] "),
    ("Adressbereich [0:16]", r'\[\d+:\d+\]"'), ("max_slew", r"max_slew = "), ("debounce", r"debounce = "),
    ("jitter-Attribut", r"jitter = "), ("wake = true", r"wake = true"), ("rate (samples)", r"\brate = \d"),
    ("max_rate", r"max_rate = "), ("capacity", r"\bcapacity = "), ("capacity_bytes", r"capacity_bytes = "),
    ("expect_len", r"expect_len = "), ("framing lines", r"framing = lines"), ("framing cobs", r"framing = cobs"),
    ("framing length_prefixed", r"framing = length_prefixed\("), ("framing fixed", r"framing = fixed\("),
    ("framing raw", r"framing = raw"), ("overflow fault", r"overflow = fault"), ("overflow drop_oldest", r"overflow = drop_oldest"),
    ("overflow drop", r"overflow = drop\b"), ("irreversible", r"irreversible = true"),
    ("label-Metadatum", r'label = "'), ("display-Metadatum", r"display = "), ("group-Metadatum", r'group = "'), ("doc-Metadatum", r'doc = "'),
    ("command", r"^command \w+\s*$"), ("command with wake", r"^command \w+ with wake"),
    ("stream u8", r"stream<u8>"), ("stream bytes<N>", r"stream<bytes<"), ("stream line<N>", r"stream<line<"),
    ("stream Record", r"stream<[A-Z]\w+>"), ("stream Edge", r"stream<Edge>"), ("stream capture", r"stream<capture<"),
    ("interner Stream", r"^stream<\w+> \w+ with"), ("samples<T, N>", r"samples<"), ("port mmio", r"@ mmio\("),
    ("node-Deklaration", r"^node \w+ @ hw"), ("property", r"^property \w+:"), ("property with monitor", r"with monitor = true"),
    # --- Funktionen, Natives, Bloecke ---
    ("fn mit Einheitenvariable", r"^fn \w+\[[A-Z]\]\("), ("fn mit type/const", r"^fn \w+\[type "), ("fn ohne Rueckgabe (inout)", r"^fn \w+\[?[^\n]*\(inout "),
    ("native fn cost-Vektor", r"^native fn .* cost = \{"), ("native fn cost-Zahl", r"^native fn .* cost = \d"),
    ("native job duration", r"^native job .* duration = "), ("native from Datei", r'^native .* from "'),
    ("block mit step", r"^\s+step\("), ("block mit weiterer Methode", r"^block[^\n]*\n(?:[^\n]*\n)*?    [a-z_]+\([^)]*\)[^\n]*:\s*$"),
    ("block generic", r"^block \w+\[[A-Z]"), ("machine mit Parametern input/output", r"^machine \w+\([^)]*: input "),
    ("Parameter-Default", r"^machine \w+\([^)]*= \d"), ("machine every", r"^machine \w+.* every "),
    ("machine phase", r"^machine .* phase "), ("machine follows", r"^machine \w+ follows "), ("machine node", r"^machine \w+ node "),
    ("driver machine", r"^driver machine"), ("machine with label", r"^machine [^\n]* with label"),
    ("scenario", r'^scenario "'), ("campaign", r"^campaign \w+:"), ("sweep Bereich", r"^\s+sweep \w+ = .* step "),
    ("sweep Liste", r"^\s+sweep \w+ = \["), ("stop_on never", r"stop_on never"), ("stop_on fail", r"stop_on fail"),
    ("trigger", r"^trigger \w+"), ("trigger node", r"^trigger \w+ node "), ("arm", r"^\s+arm \w+"), ("disarm", r"^\s+disarm \w+"),
    ("instance", r"^instance \w+ = "), ("instance-Array", r"^instance \w+\[\w+ in "), ("instance resume", r"^\s*instance \w+ resume = "),
    ("gescopte Instanz im Zustand", r"^    +instance \w+ = "),
    # --- Maschinen ---
    ("fault -> X (Maschine)", r"^    fault -> "), ("fault -> X (Zustand)", r"^        +fault -> "),
    ("persist var", r"^\s+persist var "), ("persist min_interval", r"min_interval = "), ("pub var", r"^\s+pub var "),
    ("signal", r"^\s+signal \w+"), ("raise", r"^\s+raise \w+"), ("maschinenweiter loop", r"^machine[^\n]*\n(?:[^\n]*\n)*?    loop:"),
    ("state idle", r"^\s+state \w+ idle:"), ("state resume", r"^\s+state \w+ resume:"), ("state with label", r"^\s+state \w+ with label"),
    ("state FAULTED mit Uebergang", r"^\s+state FAULTED:"), ("-> FAULTED", r"-> FAULTED"),
    ("zustandslokale var", r"^        +var \w+ : .* = "), ("enter inline", r"^\s+enter: \w"), ("enter Block", r"^\s+enter:\s*$"),
    ("exit", r"^\s+exit:"), ("loop inline", r"^\s+loop: \w"), ("when inline ->", r"^\s+when .*: -> [A-Z]"),
    ("when Block", r"^\s+when [^\n]*:\s*$"), ("after inline", r"^\s+after [^:]*: -> "), ("after Block", r"^\s+after [^\n]*:\s*$"),
    ("when Stream as e", r"^\s+when \w+ as \w+: "), ("when matches Record-Muster", r"^\s+when \w+ matches [A-Z]\w+\("),
    ("on matches", r"^\s+on \w+ matches "), ("on has", r"^\s+on \w+ has "), ("on as (Catch-all)", r"^\s+on \w+ as \w+:"),
    ("on auf Maschinenebene", r"^machine[^\n]*\n(?:[^\n]*\n)*?    on \w+"),
    # --- Sequenzen ---
    ("sequence", r"^\s+sequence:"), ("wait", r"^\s+wait "), ("until timeout -> X", r"until .* timeout .* -> [A-Z]"),
    ("until timeout else", r"until .* timeout .* else:"), ("until ohne timeout", r"^\s+until [^\n]*[^s:]\s*$"),
    ("until matches as m", r"until \w+ matches .* as \w+ timeout"), ("until has", r"until \w+ has "),
    ("expect mit Text", r'^\s+expect .*, "'), ("expect ohne Text", r'^\s+expect [^,"\n]+$'), ("repeat", r"^\s+repeat "),
    ("step \"name\":", r'^\s+step "'), ("var in Sequenz", r"^\s+sequence:\n(?:[^\n]*\n)*?\s+var \w+ = "),
    # --- Statements ---
    ("check mit Text", r'^\s+check .*, "'), ("check ohne Text", r'^\s+check [^,"\n]+$'), ("check for d", r"^\s+check .* for \d+ "),
    ("check -> Ziel", r"^\s+check .* -> [A-Z]"), ("check req", r'check .* req "'), ("alert for d", r"^\s+alert .* for \d+ "),
    ("abort mit Text", r'^\s+abort "'), ("abort ohne Text", r"^\s+abort\s*$"), ("log", r'^\s+log "'), ("measure", r"^\s+measure \w+ = "),
    ("verify", r'^\s+verify .*, "'), ("verify req", r'verify .* req "'), ("verdict pass mit Text", r'verdict pass "'),
    ("verdict fail", r"verdict fail"), ("verdict ohne Text", r"verdict (?:pass|fail)\s*$"),
    ("send String", r'^\s+send \w+, "'), ("send Ausdruck", r"^\s+send \w+, [a-zA-Z]"), ("at T:", r"^\s+at .*:\s*$"),
    ("pulse", r"^\s+pulse \w+ = .* for "), ("cancel", r"^\s+cancel \w+"), ("job", r"^\s+job \w+ = \w+\("),
    ("every d: in loop", r"^\s+every \d+ \w+:"), ("for range", r"^\s+for \w+ in range\("), ("for ueber Array/Stream", r"^\s+for \w+ in \w+:"),
    ("for (k, v)", r"^\s+for \(\w+, \w+\) in "), ("break", r"^\s+break\s*$"), ("pass", r"^\s+pass\s*$"),
    ("if/elif/else", r"^\s+elif "), ("match", r"^\s+match .*:"), ("case Variante mit Feldern", r"^\s+case [A-Z_]+\(\w+"),
    ("case _", r"^\s+case _:"), ("case Bereich/Mehrfachwerte", r"^\s+case \d+\.\.\d+"), ("case OK/ERR", r"^\s+case (?:OK|ERR)\("),
    ("return", r"^\s+return "), ("Zuweisung +=", r" \+= "), ("Zuweisung -=", r" -= "), ("Zuweisung *=", r" \*= "), ("Zuweisung /=", r" /= "),
    ("lvalue Feld", r"^\s+\w+\.\w+ = "), ("lvalue Index", r"^\s+\w+\[\w+\] = "),
    # --- Ausdruecke ---
    ("a if c else b", r" if .* else "), ("and/or/not", r"\bnot \w"), ("Vergleichskette", r" <= "), ("!=", r" != "),
    ("Bitoperatoren", r" [&|^] 0x"), ("Shift", r" << | >> "), ("Komplement ~", r"= ~"), ("Modulo", r" % "),
    ("unaeres Minus", r"= -\w"), ("Cast as u16", r" as u(?:8|16|32)\b"), ("Cast as int", r" as int\b"),
    ("wrap_", r"\.wrap_u\d+\("), ("saturating/wrapping_add", r"\.(?:saturating|wrapping)_add\("), ("fma", r"\bfma\("),
    ("round/floor/ceil", r"\b(?:round|floor|ceil)\("), ("x.bit(i)", r"\.bit\("), ("x.bits(hi, lo)", r"\.bits\("),
    ("with_bit", r"\.with_bit\("), ("rotl/rotr", r"\brot[lr]\("), ("to(Einheit)", r"\.to\(\w+\)"), ("to_float", r"\.to_float\("),
    ("Duration.as(s)", r"\.as\(s\)"), ("Duration.as(min)", r"\.as\(min\)"), ("now", r"\bnow\b"), ("time_in_state", r"time_in_state"),
    ("tick als Ausdruck", r"\btick\.\."), ("last_fault", r"last_fault"), (".valid", r"\.valid\b"), (".or()", r"\.or\("),
    (".suspect/.stale/.age/.reason", r"\.(?:suspect|stale|age|reason)\b"), (".ok/.err", r"\.(?:ok|err)\b"),
    ("OK(v)", r"\bOK\("), ("ERR(e)", r"\bERR\("), ("default-Literal", r"= default\b"), ("none-Literal", r"= none\b|\bnone\)"),
    ("Hex-Literal", r"\b0x[0-9A-Fa-f]"), ("Bin-Literal", r"\b0b[01]"), ("Okt-Literal", r"\b0o[0-7]"), ("Unterstrich in Zahl", r"\d_\d"),
    ("Exponent", r"\d[eE]-?\d"), ("Float mit Einheit", r"\d\.\d+ [A-Za-z]"), ("Einheit zusammengesetzt", r"\d [A-Za-z]+[*/][A-Za-z]"),
    ("Einheit mit Exponent", r"\d [a-z]+/[a-z]+\^\d"), ("1/s dimensionslos", r"\d 1/[a-z]"), ("Duration-Literal", r"\d (?:ns|us|ms|s|min|h|d)\b"),
    ("String mit Escape", r'\\n"'), ("Format {x}", r'"\{[^}:]+\}"|\{\w+\}'), ("Format {x:hex}", r"\{\w+:hex\}"),
    ("Format {x:.3}", r"\{\w+:\.\d\}"), ("Format {x:08}", r"\{\w+:0\d\}"), ("Format {{ }}", r"\{\{"), ("Muster {n:int}", r"\{\w+:int\}"),
    ("Muster {w:word}", r"\{\w+:word\}"), ("Muster {f:float}", r"\{\w+:float\}"), ("Muster {h:hex}", r"\{\w+:hex\}"), ("Muster {s:str<N>}", r"\{\w+:str<"),
    ("Muster {_}", r"\{_\}"), ("Record-Muster", r"matches [A-Z]\w+\(\w+ = "), ("matches in Ausdruck", r"\bif \w+ matches "),
    ("has in Ausdruck", r"\bif \w+ has "), ("Array-Literal", r"= \[\d"), ("Matrix-Literal", r"= \[\["), ("Tupel-Literal", r"\(\d[\d.]* [A-Za-z]+, "),
    ("Index a[i]", r"\w\[\w+\]"), ("Matrix-Index A[i, j]", r"\w\[\d, \d\]"), ("Slice b[a..c]", r"\w\[\w+\.\.\w+\]"),
    ("Reduktion .min()/.max()", r"\.(?:min|max|mean|rms)\(\)"), (".count/.last", r"\.(?:count|last)\b"), ("transpose/inv/det", r"\.(?:transpose|inv|det)\(\)"),
    ("solve/cholesky", r"\b(?:solve|cholesky)\("), ("interp", r"\binterp\("), ("TYPE.decode", r"[A-Z]\w+\.decode\("), (".encode()", r"\.encode\(\)"),
    ("vec.get", r"\.get\("), ("push/clear", r"\.(?:push|clear)\("), ("map insert/remove", r"\.(?:insert|remove)\("), ("reader/writer", r"\b(?:reader|writer)\("),
    ("explizite Instanziierung f[U](", r"\b[a-z_]+\[[A-Z]\w*\]\("), ("Array von Blockinstanzen", r"= \[\d+\] [a-z_]+\("),
    ("Blockaufruf var f = block(...)", r"^\s+var \w+ = [a-z_]+(?:\[\w+\])?\(\w+ = "), (".step(", r"\.step\("), (".reset()", r"\.reset\(\)"),
    ("Stream .t/.seq/.text/.data", r"\.(?:t|seq|text|data)\b"), (".dropped/.count/.skip", r"\.(?:dropped|overflowed|malformed|skip)\b"),
    ("tx.free", r"\.free\b"), ("job .done/.result", r"\.(?:done|result)\b"), ("trigger .fired/.armed", r"\.(?:fired|armed)\b"),
    ("Output .jitter", r"\.jitter\b"), ("capture .pre/.post/.samples", r"\.(?:pre|post|samples)\b"), ("m.state (Maschine)", r"\w+\.state\b"),
    ("cells[i].state", r"\w+\[\w+\]\.\w+"),
    # --- Typen ---
    ("int in a..b", r": int in "), ("float[U] in a..b U", r": float\[\w+\] in "), ("Duration in a..b", r": Duration in "),
    ("u16[mV]", r": u\d+\[\w+\]"), ("i8/i16/i32/i64", r"\bi(?:8|16|32|64)\b"), ("u64", r"\bu64\b"), ("bool", r": bool\b"),
    ("f32/f64 explizit", r": f(?:32|64)\b"), ("str<N>", r"\bstr<\d+>"), ("bytes<N>", r"\bbytes<\w+>"), ("vec<T, N>", r"\bvec<"),
    ("line<N>", r"\bline<\d+>"), ("table<A, B>", r"\btable<"), ("map<K, V, N>", r"\bmap<"), ("mat<R, C>", r"\bmat<\d"),
    ("mat<R, C>[U]", r"\bmat<\d+, \d+>\["), ("mat[X, 1/X]", r"\bmat\[[A-Z], 1/[A-Z]\]"), ("vec[X]", r"\bvec\[[A-Z]\]"),
    ("[N] T Array", r": \[\d+\] "), ("T?", r"\w\?\s*[:=\n)]"), ("T!E", r"\w![A-Z]\w+"), ("Typvariable T?", r"-> T\?"),
    ("Typ mit Range und ?", r"in [^\n]*\?"), ("generic const N in", r"const N in \d"), ("capability", r"type \w+: (?:pod|eq|ord|numeric|integer|float)"),
    ("inout", r"\binout "),
]


def main():
    files = sorted(glob.glob(os.path.join(HERE, "*.takt"))) + sorted(glob.glob(os.path.join(HERE, "ref", "*.takt")))
    texts = {f: io.open(f, encoding="utf-8").read() for f in files}
    missing = []
    for name, pattern in NUANCES:
        rx = re.compile(pattern, re.M)
        hits = [os.path.relpath(f, HERE) for f, t in texts.items() if rx.search(t)]
        if not hits:
            missing.append(name)
    print(f"{len(NUANCES)} Nuancen, {len(files)} Dateien, {len(NUANCES) - len(missing)} abgedeckt, {len(missing)} ohne Beispiel")
    for name in missing:
        print("  fehlt:", name)
    return 1 if missing else 0


if __name__ == "__main__":
    sys.exit(main())
