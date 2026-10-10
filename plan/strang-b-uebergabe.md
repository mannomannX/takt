# Übergabe Strang B, Punkt B1 (Stand 2026-10-11, 00:30)

Angehalten auf Bitte der Koordination (Neustart des Rechners). Zweig
`worktree-agent-a73ea15b6deb3cb5b`, Worktree
`G:\takt\.claude\worktrees\agent-a73ea15b6deb3cb5b`. Alles Eigene steht im
WIP-Commit „WIP Strang B (B1): …“; nur die zwei Sonden
`crates/takt-prove/tests/zz_probe.rs` und `zz_probe2.rs` liegen bewusst
unversioniert im Worktree (Werkzeug, nicht committen, vor dem Abschluss
löschen).

## Fertig und geprüft

- **Arrays der SMT-Theorie im Modell** (`crates/takt-prove`):
  - `term.rs`: Sorten `Array(Elem)` und `Array2(Elem)` (Array aus Arrays),
    Operationen `Select`, `Store`, `ConstArray`, Faltung `select` über
    `store`/`const` bei festem Index.
  - `eval.rs`: `Val::Arr` mit geteilten Blöcken zu 64 Plätzen; `Val` ist nicht
    mehr `Copy`.
  - `smt.rs`: Druck in SMT-LIB und für Spacer (LIA mit `(Array Int Int)`).
    `horn` liefert jetzt `Err(Grund)` außerhalb des Fragments; `solve.rs`
    nennt den Grund im Urteil (`Search::Outside`), statt still zu übergehen.
  - `solve.rs`: Arrays in `get-value` (z3 schreibt `store`-Ketten mit
    `let`), `lookup` für Stimuli aus Arrays.
  - `encode/array.rs` (neu): Schwelle `ARRAY_FROM = 256` Blätter, `V::Seq`,
    `Shape::Seq`, `Places`, `flat`/`conform`/`regroup`; Analyse
    `slot_order_observed` (welche map ihre Slotfolge zeigt).
  - `encode/value.rs`, `map.rs`, `tx.rs`, `stream.rs`, `read.rs`, `text.rs`,
    `canon.rs`, `wire.rs`, `run.rs`: Sammlungen, map nach Schlüsseln,
    Sendepuffer und Ring als Arrays; `Model::run` liest sie.
  - `encode.rs`: Funktionsrumpf ohne die Bedingung des Aufrufers kodiert,
    geschlossene Funktion ohne Argumente einmal (`calls`, `returned`,
    `closed`); `UNROLL_LIMIT` 4096 → 8192.
- **Zweite Lücke 14.8** (Ursache: Sendepuffer eines `sim`-Stroms zweier
  Szenarien gehörte dem Sema-Besitzer statt dem laufenden Szenario):
  `tx_defs`, zusammengesetzter Output und `run_end` fragen `Enc::foreign`.
  Test zuerst rot, dann grün: `a_sim_stream_of_two_scenarios_belongs_to_the_one_that_runs`.
- **Neue Tests** (`against_interpreter.rs`): `large_collections_agree`,
  `large_maps_agree` (Mutation an der Kapazität fällt auf),
  `a_large_buffer_over_a_sim_binding_agrees`; (`with_solver.rs`):
  `a_path_through_a_large_buffer_is_confirmed`,
  `an_inductive_invariant_over_a_large_map_is_proven` (Spacer mit Arrays,
  bewiesen k = 0); umgeschrieben: `a_state_beyond_the_limit_is_out_of_reach`,
  `a_send_beyond_the_node_limit_is_refused_before_it_is_built`.
- **Register/Korpus/Definition**: `MODEL_GAPS` leer (`examples.rs`); 45 und
  110 in der Suite `beweiser` (`corpus-try/manifest.csv`); 13.3 und
  Änderungsprotokoll in `plan/definition.md` (nennt FB-600, die Zeile fehlt
  noch in `feedback.csv`, siehe unten).
- **Geprüft** (Debug, `CARGO_TARGET_DIR=D:/rust/target-b`):
  - Suite `takt-prove` mit Solver: alles grün (with_solver 37 Tests, 198 s) —
    gelaufen *vor* der Änderung an `horn`/`Search` und `lookup`; danach nur
    `cargo check`. Muss noch einmal laufen.
  - `capabilities.rs` (Wahrheitstest der Matrix): grün, 92 s.
  - `examples.rs` (`the_reference_examples_agree_on_both_paths`,
    ASan-Variante): grün, 132 s.
  - Sonde: 45, 110 und 14.8 (alle drei Szenarien) kodieren
    (45: 351 Zustandsorte, 830 000 Knoten, 10 s Debug) und stimmen Zeile für
    Zeile mit dem Interpreter.

## Angefangen, nicht fertig

- **`solver_paths` UPDATE für 45 und 110 (Release)**: abgebrochen während des
  Laufs (nach dem Release-Bau, im Erzeugen). Für **keines** der beiden
  Programme wurden Dateien geschrieben: Unter `corpus-try/paths` gibt es
  nichts zu 45 oder 110, nichts Halbes. Beide sind noch zu erzeugen.
  - Befund vorher: Eine BMC-Anfrage der Tiefe 5 an 45 ist rund 183 MB
    SMT-LIB (der Kegel umfasst fast das ganze Flash-Modell); z3 läuft in die
    10 s und braucht etwa 640 MB. Der Lauf erzeugt daher vermutlich kaum
    Pfade; geschätzt 20–40 min für beide.
- **FB-Zeilen**: vorbereitet in `D:\rust\agent-b\scratch\fb_rows.py`
  (FB-497 → behoben, neu FB-600 Sendestrom zweier Szenarien, FB-601
  Funktionsrumpf/`UNROLL_LIMIT`, FB-602 offen: Interpreter liest `sim`-Output
  eines zweiten Szenarios mit Unit-Delay). Erst ausführen, wenn
  `solver_paths` und `the_three_executors_agree` grün sind (FB-497 nennt
  beide): `python -I D:\rust\agent-b\scratch\fb_rows.py 2026-10-11`.

## Genauer nächster Schritt

1. Sperre prüfen (`G:\rust\heavy.lock`), anlegen mit
   `Strang B, <Zeit>, solver_paths UPDATE 45/110 (Release)`.
2. Im Worktree:
   `CARGO_TARGET_DIR=D:/rust/target-b CARGO_BUILD_JOBS=5 UPDATE_PATHS=45_journal_cut.takt,110_journal_log.takt cargo test --release -p takt-conformance --test solver_paths -- --nocapture`
   (Release-Bau liegt schon in `D:\rust\target-b`; Protokoll z. B. nach
   `D:\rust\agent-b\scratch\solver_paths.log`). Danach Sperre löschen.
3. `the_three_executors_agree` (`differential.rs`) unter der Sperre; die
   Meldung nennt die exakten Zahlen für 45 und 110 → `UNREACHED` nachziehen
   (heute `(14, 2, 26)` und `(17, 3, 45)`), erneut laufen lassen.
4. Suite `takt-prove` mit Solver noch einmal (wegen `horn`/`Search`/`lookup`).
5. `cargo clippy --workspace --all-targets` ohne Warnung, `cargo fmt --all`.
6. FB-Skript ausführen, Sonden löschen, WIP-Commit durch den Abschluss-Commit
   ergänzen, Bericht an die Koordination.

## Offene Fragen

- FB-602 (Besitzer eines `sim`-Outputs je Lauf) betrifft Interpreter,
  erzeugten Code und Modell; nicht in B1 gebaut, Entscheidung der
  Koordination.
- Die Größe des Flash-Modells (≈ 0,8 Mio. Knoten je Tick) macht Anfragen
  der Tiefe 5 für z3 zu groß; das gehört zu B2 (Ausrollen von 4096
  Durchläufen, `for x in seed` über 256 Plätze).

## Benutzte FB-Nummern

FB-600, FB-601, FB-602 (vorbereitet, noch nicht in `plan/feedback.csv`).
Block von Strang B: FB-600 bis FB-699.
