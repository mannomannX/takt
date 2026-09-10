#!/usr/bin/env python3
"""Differenzieller Vergleich: Rust-Tokenizer gegen den Python-Tokenizer aus parse_corpus.py.

    python grammar/diff_tokens.py DATEI...

Beide sind Implementierungen von grammar/lexer.md; sie muessen fuer jede Datei
denselben Tokenstrom liefern (Art und Text; Beiwerk und Anliegen ausgenommen).
"""
import subprocess, sys, os

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

def rust(path):
    r = subprocess.run(["cargo", "run", "-q", "-p", "takt-cli", "--", "tokens", path],
                       capture_output=True, text=True, encoding="utf-8", cwd=ROOT)
    lines = [l for l in r.stdout.splitlines() if "\t" in l]
    return [l.split("\t")[:2] for l in lines], r.returncode

def python(path):
    r = subprocess.run([sys.executable, "-X", "utf8", os.path.join(ROOT, "grammar", "parse_corpus.py"), "--tokens", path],
                       capture_output=True, text=True, encoding="utf-8", cwd=ROOT)
    lines = [l for l in r.stdout.splitlines() if "\t" in l]
    return [l.split("\t")[:2] for l in lines], ("LEXFEHLER" in r.stdout)

def main(files):
    bad = 0
    for f in files:
        a, rc = rust(f)
        b, lexfail = python(f)
        if lexfail or rc != 0:
            status = "beide lehnen ab" if (lexfail and rc != 0) else ("NUR RUST lehnt ab" if rc else "NUR PYTHON lehnt ab")
            if "NUR" in status: bad += 1
            print(f"[{status:18}] {f}")
            continue
        if a == b:
            print(f"[gleich {len(a):5}   ] {f}")
        else:
            bad += 1
            for i, (x, y) in enumerate(zip(a, b)):
                if x != y:
                    print(f"[VERSCHIEDEN      ] {f}: Token {i}: rust={x} python={y}")
                    break
            else:
                print(f"[VERSCHIEDEN      ] {f}: Laenge rust={len(a)} python={len(b)}")
    print(f"\n{len(files)} Dateien, {bad} Abweichungen")
    return 1 if bad else 0

if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
