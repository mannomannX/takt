#!/usr/bin/env python3
"""Differenzieller Vergleich: Rust-Parser gegen das Grammatik-Orakel parse_corpus.py.

    python grammar/diff_parse.py [--snippet] [--mutate] DATEI...

Ohne --mutate wird jede Datei von beiden gelesen; sie muessen dasselbe Urteil
(angenommen/abgelehnt) faellen. Mit --mutate wird zusaetzlich aus jeder Datei
eine Familie von Varianten erzeugt (je Zeile ein Wort geloescht oder verdoppelt);
die Urteile muessen ebenfalls uebereinstimmen. Jede Abweichung ist ein Fehler im
Parser, im Orakel oder eine Luecke der Grammatik und wird mit der veraenderten
Zeile ausgegeben. --snippet reicht an beide Werkzeuge durch.
"""
import io, os, subprocess, sys, tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BATCH = 150  # Windows begrenzt die Laenge der Kommandozeile


def batches(items):
    for i in range(0, len(items), BATCH):
        yield items[i:i + BATCH]


def rust(paths, snippet):
    ok = set()
    errors = {}
    for chunk in batches(paths):
        cmd = ["cargo", "run", "-q", "-p", "takt-syntax", "--example", "parse", "--"]
        if snippet:
            cmd.append("--snippet")
        r = subprocess.run(cmd + chunk, capture_output=True, text=True, encoding="utf-8", cwd=ROOT)
        for line in r.stdout.splitlines():
            path, _, rest = line.partition(": ")
            if rest.startswith("ok ("):
                ok.add(path)
            else:
                errors.setdefault(path, rest)
    return ok, errors


def python(paths, snippet):
    ok = set()
    errors = {}
    for chunk in batches(paths):
        cmd = [sys.executable, "-X", "utf8", os.path.join(ROOT, "grammar", "parse_corpus.py")]
        if snippet:
            cmd.append("--snippet")
        r = subprocess.run(cmd + chunk, capture_output=True, text=True, encoding="utf-8", cwd=ROOT)
        for line in r.stdout.splitlines():
            if line.startswith("[OK"):
                ok.add(line.split("] ", 1)[1].split("  (")[0])
            elif line.startswith("[PARSEFEHLER]") or line.startswith("[LEXFEHLER]"):
                path, _, rest = line.split("] ", 1)[1].partition(": ")
                errors[path] = rest
    return ok, errors


def mutants(path, out_dir):
    """Je Zeile und Wort eine Variante: Wort geloescht, Wort verdoppelt."""
    text = io.open(path, encoding="utf-8").read()
    lines = text.split("\n")
    base = os.path.splitext(os.path.basename(path))[0]
    result = []
    for i, line in enumerate(lines):
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        indent = line[:len(line) - len(line.lstrip())]
        words = line.split()
        for j in range(len(words)):
            for op, new_words in (("del", words[:j] + words[j + 1:]), ("dup", words[:j + 1] + words[j:])):
                if not new_words:
                    continue
                mutated = lines[:i] + [indent + " ".join(new_words)] + lines[i + 1:]
                name = f"{base}__{i + 1}_{j}_{op}.takt"
                p = os.path.join(out_dir, name)
                io.open(p, "w", encoding="utf-8", newline="\n").write("\n".join(mutated))
                result.append((p, i + 1, " ".join(new_words)))
    return result


def main(argv):
    snippet = "--snippet" in argv
    mutate = "--mutate" in argv
    files = [a for a in argv if not a.startswith("--")]
    work = tempfile.mkdtemp(prefix="takt-mutants-")
    cases = [(os.path.relpath(f, ROOT).replace("\\", "/"), None, None) for f in files]
    if mutate:
        for f in files:
            cases += mutants(f, work)
    paths = [c[0] for c in cases]
    r_ok, r_err = rust(paths, snippet)
    p_ok, p_err = python(paths, snippet)
    disagreements = 0
    for path, line_no, line in cases:
        key = path.replace("\\", "/")
        a = key in r_ok or path in r_ok
        b = key in p_ok or path in p_ok
        if a == b:
            continue
        disagreements += 1
        where = f"{os.path.basename(path)}" if line_no is None else f"{os.path.basename(path)} Zeile {line_no}: {line}"
        if a:
            print(f"[NUR RUST nimmt an ] {where}\n    Orakel: {p_err.get(key, p_err.get(path, '?'))}")
        else:
            print(f"[NUR ORAKEL nimmt an] {where}\n    Rust: {r_err.get(key, r_err.get(path, '?'))}")
    agree_ok = sum(1 for c in cases if c[0] in r_ok and c[0] in p_ok)
    print(f"\n{len(cases)} Faelle, {agree_ok} beide angenommen, {len(cases) - agree_ok - disagreements} beide abgelehnt, "
          f"{disagreements} Abweichungen")
    return 1 if disagreements else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
