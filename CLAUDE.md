# Takt

Deterministische, absturzfreie Steuer- und Testsprache. Die Referenz ist
`plan/definition.md`; sie gewinnt gegen jede andere Quelle, auch gegen
diesen Text.

## Sprachregel

**Bezeichner sind englisch, Prosa ist deutsch.** Die Trennlinie ist, was
der Compiler sieht:

| Was | Sprache |
|---|---|
| Variablen, Funktionen, Typen, Felder, Konstanten, Module | **Englisch** |
| Datei- und Verzeichnisnamen | **Englisch** |
| Testfunktionen (`fn the_window_starts_at_the_cursor`) | **Englisch** |
| Kommentare und Doc-Kommentare | Deutsch |
| `plan/*.md`, `feedback.csv`, `features.csv` | Deutsch |
| Commit-Meldungen | Deutsch |
| Diagnosetexte und Trace-Ausgaben | Deutsch |

Umlaute in Rust-Quelltext als `ue`, `ae`, `oe` (auch in Kommentaren);
in Markdown die echten Umlaute.

Testdateien heissen nach dem, was sie pruefen: `examples.rs`,
`streams.rs`, `stream_invariants.rs`, `ring_agreement.rs` — nicht
`beispiele.rs` oder `stroeme.rs`.

## Werkzeuge

- `cargo nextest run --workspace` — die volle Suite, jeder Test in seinem
  Prozess und die Binaries zugleich (`.config/nextest.toml`); `cargo test
  --workspace` prueft dasselbe, nur nacheinander
- `cargo clippy --workspace --all-targets` — muss ohne Warnung laufen
- `cargo fmt --all` — rustfmt, `max_width = 120`
- `UPDATE_GOLDEN=1 cargo test -p takt-syntax --test sexpr_golden` —
  Golden-Dateien neu schreiben, wenn ein Korpusprogramm sich aendert
- `pwsh tools/prune-target.ps1 [-Apply]` — veraltete Build-Artefakte im
  Zielverzeichnis (alte Testfassungen, Caches); vor langen Laeufen
- `pwsh tools/board-run.ps1 [-Rev <stand>] [-Boards esp32c6|stm32f401]` —
  die Board-Suiten in einem eigenen Worktree und Zielverzeichnis, ohne den
  Hauptbaum zu blockieren; Protokolle unter `G:ustoard-runs`
- `cargo run -p takt-cli -- fmt <datei>` — Korpusdateien kanonisch
  formatieren; ein Test prueft das

Nach jeder Aenderung an `corpus-try/*.takt`: formatieren *und* die
Golden-Dateien nachziehen, sonst schlagen zwei Tests fehl.

## Grenzen, die nicht verhandelbar sind

- `unsafe_code = "forbid"` gilt workspace-weit (13.4 nennt es einen
  Zertifizierungsgrund). Was `unsafe` braeuchte, wird anders geloest
  oder gar nicht.
- Kein `float` in eigener Dezimalkonversion: 4.2 verlangt bitgleiche
  Ergebnisse ueber alle Targets, und eine zweite Implementierung waere
  eine zweite Rundungsquelle.
- Der Interpreter ist die Spezifikation; weicht der Codegen ab, ist der
  Codegen falsch (Satz 9.4.4).
