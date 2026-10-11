# Übergabe Strang B, Punkt B1 (Stand 2026-10-11, 02:35)

Zweig `worktree-agent-a73ea15b6deb3cb5b`, Worktree
`G:\takt\.claude\worktrees\agent-a73ea15b6deb3cb5b`. Commits: `8041bc6`
(WIP, Arrays) und der Folgecommit dazu. Diese Datei fällt mit dem
Abschlusscommit von B1 weg.

## Fertig und geprüft

- Arrays der SMT-Theorie im Modell von `takt prove` (`crates/takt-prove`,
  `encode/array.rs`), map nach Schlüsseln, Arrays aus Arrays, Sendepuffer und
  Ringe als Arrays; Funktionsrümpfe ohne die Bedingung des Aufrufers,
  geschlossene Funktionen ohne Argumente einmal; `UNROLL_LIMIT` 8192.
- Zweite Lücke 14.8 von der Ursache her (FB-600).
- Folgecommit: `clear` einer map aus Puffern nullte die inneren Arrays in der
  falschen Sorte (Test erst rot, dann grün: `large_maps_agree` prüft jetzt
  die Sorte jedes Folgezustands); der Kodierer lehnt einen Ort, dessen
  Folgewert die Sorte wechselt, laut ab; `value_at` meldet Arrays aus Arrays
  als Lücke statt sie falsch zu lesen; Clippy.
- Grün (Debug, `CARGO_TARGET_DIR=D:/rust/target-b`): Suite `takt-prove` mit
  Solver (139 Tests, nach allen Änderungen), `capabilities.rs`, `examples.rs`
  (`MODEL_GAPS` leer), `cargo clippy --workspace --all-targets` ohne Warnung,
  `cargo fmt --all`.
- FB-Zeilen in `plan/feedback.csv`: FB-497 behoben, FB-600 und FB-601
  behoben, FB-602 offen (Besitzer eines `sim`-Outputs je Lauf, alle drei
  Ausführer), FB-603 offen (Pfade für 45 und 110, für B2).

## Pfade für 45 und 110

Nicht erzeugt (FB-603): Der Lauf mit `UPDATE_PATHS` schaffte etwa eine
Anfrage je Minute, jede 183–195 MB SMT-LIB, z3 im Zeitlimit von 10 s. Nach
66 von geschätzt 130 Anfragen in 70 min auf Bitte der Koordination
abgebrochen; kein Programm war fertig, es gibt keine Pfaddateien. Die
meiste Zeit kostet `smt::cone` (Variablen je Zustandsort einzeln über den
geteilten Graphen, je Anfrage).

## Noch offen in B1

1. Unter `G:\rust\heavy.lock`, wenn Strang A fertig ist:
   `the_three_executors_agree` (`crates/takt-conformance/tests/differential.rs`)
   mit `CARGO_TARGET_DIR=D:/rust/target-b`. 45 und 110 laufen jetzt mit
   Modellspalte; ohne neue Pfade sollte `UNREACHED` für sie gleich bleiben
   (`(14, 2, 26)` und `(17, 3, 45)`). Meldet der Test andere Zahlen, diese
   übernehmen.
2. Abschlusscommit (diese Datei löschen), Bericht an die Koordination.

## FB-Nummern

FB-600 bis FB-603 benutzt; Block von Strang B: FB-600 bis FB-699.
