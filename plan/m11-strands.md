# M11: zwei Stränge

Stand 2026-10-10, nach Schritt 29. Dieser Plan regelt, wie die offenen
Schritte aus `plan/m11.md` zu zweit abgearbeitet werden: wer was nimmt,
welche Ressourcen geteilt sind, wie geprüft wird und wie die Arbeit
zusammenkommt. `plan/definition.md` gewinnt gegen alles, auch gegen diesen
Plan; `plan/m11.md` bleibt die Quelle für Ziel und Abnahme je Schritt.

## Rollen

- **Koordination** (Hauptsitzung, zugleich Strang A): teilt Punkte aus der
  Warteschlange zu, führt diese Datei, merged die Zweige nach `main`, fährt
  vor jedem Merge die volle Suite und vergibt bei Bedarf Register-Nummern.
- **Strang A**: die Hauptsitzung, im Hauptbaum `G:\takt`.
- **Strang B**: ein Agent in einem eigenen Worktree auf eigenem Zweig. Er
  arbeitet einen Punkt vollständig ab, committet auf seinem Zweig und
  übergibt mit einem Bericht. Danach bekommt er den nächsten Punkt.

## Geteilte Ressourcen

| Ressource | Regel |
|---|---|
| Zielverzeichnis | A: `G:\rust\target`, B: `D:\rust\target-b` (auf `G:` ist kein Platz für ein zweites). Nie ein fremdes; ein Worktree mit dem Ziel des Hauptbaums vermischt Artefakte. |
| Platz | Vor jedem langen Lauf `pwsh tools/prune-target.ps1 -Apply` (für B mit dem eigenen Zielverzeichnis). Auf `G:` unter 10 GB frei: kein neuer Bau, Koordination fragen. Die Zielverzeichnisse anderer Projekte unter `G:\rust` bleiben unberührt. |
| Schwere Läufe | Volle Suite, `the_three_executors_agree`, `solver_paths` mit `UPDATE_PATHS`, Release-Bauten, Docker/qemu, Mutationen, Board-Bauten. Vorher die Sperre `G:\rust\heavy.lock` anlegen (Inhalt: Strang, Zeit, Lauf), danach löschen. Liegt sie schon da und ist jünger als drei Stunden, warten (alle fünf Minuten nachsehen). Der Rechner hat eine Commit-Grenze: Zwei schwere Läufe zugleich haben schon `rustc` und `z3` abstürzen lassen. |
| Kerne | Höchstens Kerne − 1; ein Kern bleibt für den Nutzer frei. |
| Boards | ESP32-C6 (COM4) und STM32F401 (COM7) je einzeln: Sperre `G:\rust\board-c6.lock` bzw. `board-f401.lock`. Board-Läufe nur über `tools/board-run.ps1` (eigener Worktree, eigenes Ziel, abgekoppelt). |
| Werkzeuge | Zephyr, FreeRTOS, CMake, Ninja unter `D:\takt-tools`, je Sitzung aktivieren; z3 und cvc5 unter `~\.takt\bin`. |

## Register und gemeinsame Dateien

- **FB-Nummern in Blöcken**: Strang A nimmt FB-512 bis FB-599, Strang B
  FB-600 bis FB-699. `tests.csv` nach demselben Muster mit dem Präfix des
  Bereichs; im Zweifel fragt der Strang die Koordination.
- `feedback.csv` und `tests.csv` nur zeilenweise ändern oder anhängen, nie
  ganz neu schreiben (sonst ändert sich das Quoting fremder Zeilen).
  Angehängte Zeilen beider Stränge kollidieren beim Merge am Dateiende; die
  Koordination löst das.
- `plan/m11.md`: Jeder Strang ändert nur die Statuszeile seines Schritts.
- `plan/definition.md`: Änderungen im betroffenen Abschnitt und je ein
  Eintrag im Änderungsprotokoll am Ende.
- **Kern-Crates** (`takt-syntax`, `takt-sema`, `takt-mir`, `takt-interp`,
  `takt-llvm`, `takt-hal`): Jeder Strang darf dort Fehler beheben, die er
  findet, aber klein und mit eigenem Test. Im Bericht nennt er jede Änderung
  dort, damit der andere Strang nach dem Merge zeitnah nachzieht.
- Nie `git add -A`. Nie `plan/ideen-bewertung.md`, `plan/hil-request.md`
  oder `.agent/` stagen.

## Bereiche

Zwei Stränge arbeiten nie gleichzeitig im selben Bereich. Boards sind kein
Bereich; dort regelt die Sperre die Zeit.

Seit 2026-10-12 laufen schwere Prüfungen auf CI (Schritt 34): Neben einem
anderen Projekt reicht der Speicher des Entwicklungsrechners nicht mehr
für die volle Suite oder Board-Bauten. Lokal nur gezielte Tests mit
`cargo nextest run -j 1`, und nur bei mindestens 6 GB freier Zusage.

| Bereich | Umfang |
|---|---|
| P (Beweiser) | `crates/takt-prove`, `corpus-try/paths`, Tests in `takt-conformance`, die nur das Modell betreffen |
| C (Abnahme) | `crates/takt-conformance` (Rahmen der Abnahme, Vergleiche, Ratschen) |
| R (Rahmen, MCU) | `crates/takt-frame` (MCU-Teil), Bring-ups, `takt-rt-baremetal` |
| E (Einbettung, Wirt) | `crates/takt-embed`, Host-Formen, `takt-rt-core`, `takt-rt-linux` |
| S (SDKs und Ports) | C-SDK, CMake, Zephyr-Modul, FreeRTOS- und ESP-IDF-Ports, Adapter |
| W (Werkzeug) | `crates/takt-cli` (`port-test`, `check-image`, `new`, `doctor`) |

## Warteschlange

Wer fertig ist, nimmt den obersten Punkt, der

1. frei ist,
2. dessen Abhängigkeiten erledigt sind und
3. dessen Bereiche der andere Strang gerade nicht bearbeitet.

Punkte mit *exklusiv* laufen nur, wenn der andere Strang in dieser Zeit
keinen schweren Lauf braucht. Die Koordination trägt Strang und Datum ein.

| Nr | Punkt | Bereiche | Boards | hängt an | Stand |
|---|---|---|---|---|---|
| B1 | FB-497 Rest: Maps und Puffer als Arrays der SMT-Theorie | P | – | 29 | Strang B, umgesetzt |
| B2 | FB-511: jede Prüfstelle erreicht oder bewiesen unerreichbar | P, C | – | B1 | Schritt 1 umgesetzt und eingeordnet (CI-Lauf 38188098609); Schritt 2, Teil 1 (kleine Anfragen zuerst) umgesetzt und gemergt 2026-10-12, eingeordnet in 38199208294: Speicher 106 → 68, 38 bewiesen (FB-612); die Gruppe „nicht k-induktiv“ ist größtenteils tief erreichbar und gehört zu Schritt 3 (FB-613); Schritt 3 (tiefe Pfade) bei Strang B (Helfer, seit 2026-10-12, Zweig `b3-fb511-step3`); Schritte 2 (Rest), 4 und 5 frei |
| 14 | Rust ohne RTOS und mit Frameworks | R, E | beide | 10 | Strang A, umgesetzt 2026-10-11 |
| 17 | Wirt: Instanzen in Fäden, logische Zeit; `linux_rt` | E | – | 8 | Strang D (Helfer, seit 2026-10-12, Zweig `d-17-host-threads`) |
| 15 | `takt port-test` mit `--judge` | W, R | beide | 14 | Strang A, umgesetzt 2026-10-11 |
| 16 | Zwei Programme auf einem Kern; Antwortzeiten in `check-image` | R, W | beide | 11, 14 | frei |
| 18 | C-SDK und CMake; C-Ports Interruptform und POSIX (18a Wirt, 18b F401, 18c Linux) | S, W | F401 | 11, 15 | Strang A: 18a und 18c umgesetzt (POSIX-Port besteht die Portprüfung, b52327e); 18b nach 34e (Strang A) |
| 9 | Ein Rahmen: Wirtsdifferential über den Produktrahmen | C, R, E | – | 8, 18a | frei, nach 17 (beide bauen den Rahmen des Wirts um) |
| 19 | Zephyr-Modul, Partition, Devicetree-Adapter; CMSIS-RTOS2 | S | beide | 18 | frei |
| 20 | FreeRTOS auf dem F401, FreeRTOS-MPU | S | F401 | 18 | frei |
| 21 | ESP-IDF auf dem C6 (21a Komponente und Port, 21b Funk und Messung) | S | C6 | 11, 18 | frei |
| 22 | Adapter und Senken | S | beide | 14, 19 | frei |
| 23 | Speicherschutz neben fremdem Code; `plan/cert.md` | S, R | beide | 10, 19, 21 | frei |
| 33 | Regelwerk für den eigenen Code (33a Katalog und Rust-Stufen, 33b C-Teil, 33c Miri) | E, R, S; Stufe 1 in allen | – | 33b: 18a | 33a Strang A bis auf die Board-Crates, Bring-ups und das Sprungbrett: die bei Strang E (Helfer, seit 2026-10-12, Zweig `e-33a-board-tiers`, mit Lint-Job auf CI); 33b und 33c Strang C, Zweig `c-33bc-analysis`, wartet auf den Merge |
| 34 | Suite auf Runnern von GitHub (34a Linux, 34b Windows, 34c Solver-Läufe, 34d Board-Abbilder, 34e Board-Läufe ohne lokalen Bau) | C, W | – | – | 34a bis 34d umgesetzt (34d von Strang B, gemergt 8eb251d); 34e Strang A: die Test-Binaries der Boards als nextest-Archiv von einem Windows-Runner, lokal nur flashen und prüfen |
| 30 | Wachsamkeit: `tools/mutants.sh`, Ratsche je Crate | alle (lesend), exklusiv | – | 28, 29, B2 | frei, möglichst spät |
| 24 | Vorlagen, `takt doctor`, Anleitung, Inventur, Abschluss | W | beide | alle | zuletzt |

Mitgeführte Register-Termine: Schritt 9 nimmt FB-391 und FB-382 mit,
Schritt 15 KON1-022, Schritt 17 FB-501, Schritt 21b KON1-021. Wer den
Schritt nimmt, schließt diese Zeilen mit ab.

## Ablauf je Punkt

So arbeitet jeder Strang, als wäre er allein:

1. **Lesen.** Die Zeile des Schritts in `plan/m11.md` samt seinem Abschnitt
   darin, die zitierten Abschnitte von `plan/definition.md`, die offenen
   Zeilen in `feedback.csv` und `tests.csv` zum Punkt, `CLAUDE.md`.
2. **Entscheiden.** Je Frage die langfristig beste, vollständige Lösung,
   keine Umgehung. Eine echte Sprachentscheidung, die die Referenz offen
   lässt, geht mit Optionen und Empfehlung an die Koordination, bevor sie
   gebaut wird.
3. **Umsetzen.**
   - Zu jedem gefundenen Fehler zuerst ein Test, der scheitert.
   - Der Interpreter ist die Spezifikation; weicht der erzeugte Code ab, ist
     der erzeugte Code falsch.
   - Kein `unsafe`. Platzhalter nur begründet und mit `TODO` markiert.
   - Bezeichner englisch, Kommentare deutsch (`ue`, `ae`, `oe`).
   - Bearbeitungsskripte als Datei im Scratchpad, nie als Heredoc mit
     Backslashes.
4. **Prüfen**, gestuft nach der Prüfmatrix unten. Während der Arbeit nur
   `cargo check` und gezielte Tests. Vor der Übergabe: `cargo fmt --all`,
   `cargo clippy --workspace --all-targets` ohne Warnung, die Suiten der
   berührten Bereiche und, wo Boards betroffen sind, ein Board-Lauf.
5. **Register.** FB- und Test-Zeilen, die Statuszeile in `plan/m11.md`, die
   Definition mit Änderungsprotokoll, `LIMITS` und Ratschen: eine
   geschlossene Lücke fällt heraus, eine bleibende steht mit Grund und
   offener Zeile darin.
6. **Übergeben.** Commit auf dem eigenen Zweig: nur eigene Dateien, deutsche
   Nachricht, am Ende die Zeile `Co-Authored-By`. Dazu ein Bericht an die
   Koordination:
   - was gebaut wurde und warum,
   - jede Prüfung mit ihrem Ergebnis,
   - Änderungen in Kern-Crates,
   - offene Punkte.
7. **Merge.** Die Koordination rebased den Zweig auf `main`, fährt die volle
   Suite und merged. Der andere Strang zieht `main` beim nächsten Halt nach.

## Prüfmatrix

| Geändert | Mindestens |
|---|---|
| Interpreter, Sema, MIR | betroffene Crate-Tests, `the_three_executors_agree`, `the_reference_examples_agree_on_both_paths`, `golden_sim` |
| Codegen, Rahmen | `the_three_executors_agree`, `streams`, `ports`, `targets`; Board-Lauf beider Boards |
| Beweiser | Suite `takt-prove` (mit `with_solver`), `the_three_executors_agree`, bei Beweisdateien `solver_paths` im Release-Bau |
| Korpus (`corpus-try/*.takt`) | `takt fmt`, `UPDATE_GOLDEN=1 … sexpr_golden`, `solver_paths` für das Programm |
| Einbettung, Ports, SDKs | die Portprüfung des Ports, Board-Lauf, `embed_cmd` |
| Merge nach `main` | volle Suite `cargo nextest run --workspace` (nur die Koordination, unter der Sperre) |

## Die Punkte im Einzelnen

### B1 — Maps und Puffer als Arrays der SMT-Theorie (FB-497)

- **Ziel:** Das Modell kodiert das Flash-Modell (`flash_model`, 8.10) und
  damit 14.8 sowie die Korpusprogramme 45 und 110. Heute liegt ihr Zustand
  über `STATE_LIMIT` (32 768 Blätter), weil jede Zelle einer `map` und jedes
  Byte eines großen Puffers ein eigenes Blatt ist. Zweite Lücke in 14.8: In
  den Szenarien `fallback` und `no_image` ordnet der Kodierer den Sendestrom
  des Flash-Modells keiner kodierten Maschine zu.
- **Vorgehen:**
  - Eine Sorte `Array` (Index und Wert als Ganzzahl) in `term.rs` mit `select`
    und `store`.
  - Konkrete Auswertung in `eval.rs`, damit `Model::run` und der Vergleich mit
    dem Interpreter weiter tragen.
  - Druck in SMT-LIB.
  - Houdini und die Horn-Klauseln für Spacer prüfen; wo Spacer Arrays nicht
    trägt, ein ausdrücklicher Rückfall mit Grund im Bericht.
  - In der Kodierung `map` und `bytes` ab einer Schwelle als Array;
    `STATE_LIMIT` bleibt die Grenze für Blätter.
  - Die zweite Lücke von ihrer Ursache her beheben, nicht mit einer
    Ausnahme.
- **Abnahme:**
  - `MODEL_GAPS` in `examples.rs` ist leer.
  - 45 und 110 stehen in der Suite `beweiser` (`corpus-try/manifest.csv`),
    ihre Pfade sind erzeugt.
  - `the_three_executors_agree` ist grün; die Ratsche `UNREACHED` ist für 45
    und 110 nachgezogen.
  - Die Suite von `takt-prove` ist grün, mit neuen Tests „Modell gleich
    Interpreter“ für Programme mit großen Maps und Puffern.
  - 13.3 beschreibt die Arrays.

### B2 — Jede Prüfstelle erreicht oder bewiesen unerreichbar (FB-511)

- **Ausgangslage:** Die Ratsche `UNREACHED` hält 94 Übergänge, 72 nie
  bestandene und 522 nie verletzte Prüfstellen in 98 Programmen fest. Am
  häufigsten sind es `range` (224), `fin` (99), `valid` (88) und `ovf` (53).
  Der Solver findet bis Tiefe 5 in 10 s je Anfrage keinen Pfad, und
  k-Induktion mit k = 5 schließt sie nicht aus.
- **Vorgehen:**
  0. **Erst beschleunigen**, denn die folgenden Läufe sind lang:
     - Die beiden langen Tests von `takt-prove` aufteilen, damit nextest sie
       parallel fährt: `z3_and_cvc5_do_not_contradict_each_other` mit etwa
       170 s und `max_slew_bounds_the_change_since_the_last_good_delivery`
       mit etwa 112 s.
     - Ein Cache für Solver-Antworten: Schlüssel ist der Hash der Anfrage
       samt Solver und Version, abgelegt im Zielverzeichnis. Eine
       unveränderte Anfrage braucht dann keinen Solver, und das Neuerzeugen
       der Pfade rechnet nur, was sich geändert hat (heute über eine Stunde).
     - Ein `opt-level` für die Beweiser-Crates im Test-Profil nur, wenn eine
       Messung zeigt, dass die Kodierung und nicht der Solver die Zeit
       braucht.
  1. **Einordnen.** Je Stelle ein eigener, langsamer Lauf mit Tiefe 10 und 20
     und Frist 60 s. Daraus vier Gruppen: erreichbar, aber tief; unerreichbar,
     doch nicht k-induktiv; außerhalb des Fragments (Fließkomma,
     Bitoperationen in Horn-Klauseln); Frist. Die Zahlen gehören in den
     Bericht.
  2. **Induktive Invariante je Stelle.** Wie bei Eigenschaften (13.3): Bleibt
     der Induktionsschritt offen, dieselbe Frage als Horn-Klauseln an Spacer.
     Dazu Houdini-Vorlagen je Stelle, etwa die Range einer Variablen im
     Zustand, an dem die Stelle hängt.
  3. **Tiefe Pfade.** Die Tiefe je Programm aus seinen Fristen (`--depth
     auto`), begrenzt durch `cases::MAX_TICKS`. Läuft das zu lange für den
     regulären Lauf, wird es ein eigener Opt-in-Lauf, dessen Pfade wie die
     anderen gespeichert werden.
  4. **Beweisdateien.** Jede neu bewiesene Stelle geht in die `.takt-proof`
     ihres Programms; der erzeugte Code lässt die Prüfung weg, der Interpreter
     prüft weiter (Prüfung 65). Ein falscher Beweis fällt damit im
     Dreiervergleich auf.
  5. **Ratsche senken.** Was bleibt, steht mit seiner Gruppe als Grund in
     `UNREACHED` und in `LIMITS`.
- **Stand nach Schritt 1 (2026-10-12):** Die Einordnung lief auf CI
  (`solver.yml`, Lauf 38188098609, acht Teile, z3 mit 6 GB, Frist 60 s;
  Artefakt `fb511-ergebnis` mit einer Zeile je Stelle und dem Cache der
  Antworten). Bei Tiefe 10 sind es 282 Stellen:

  | Gruppe | Tiefe 10 | Tiefe 20 |
  |---|---|---|
  | Speicher (z3 über 6 GB) | 139 | 138 |
  | Frist | 83 | 86 |
  | nicht k-induktiv | 22 | 21 |
  | bewiesen | 16 | – |
  | Fragment (Fließkomma) | 14 | 14 |
  | ohne Urteil | 3 | 3 |
  | erreichbar | 3 | – |
  | erreichbar, tiefer | 1 | 1 |
  | unbestätigt | 1 | – |

  Bei Tiefe 20 fehlen die 19 Stellen, die Tiefe 10 schon entschied.
  Der Speicher ist die größte Gruppe, und drei Programme stellen fast alle
  dieser Stellen: `11_foc_drive` (52), `46_matrices` (31) und
  `sim/14_7` (23). Betroffen sind vor allem `fin` (70) und
  `valid` (35). Schritt 2 beginnt darum bei der Kodierung dieser drei
  Programme, nicht bei mehr Speicher: 6 GB reichten bei Tiefe 20 so wenig
  wie bei Tiefe 10.
- **Stand nach Schritt 2, Teil 1 (2026-10-12, Zweig `b2-fb511-step2`):**
  - Ursache (FB-612): Die Kodierung ist für z3 nicht redundant, teuer ist
    das Bit-Blasting der Gleitkommarechnung je Tick, zehnmal ausgerollt.
    Je Operation allein (z3 4.13.4): Division `f32` 20 MB, `f64` 163 MB,
    `fma` und `mul` `f64` 11 MB. `11_foc_drive`: Anfangszustand und ein Tick
    brauchen 1,65 GB, davon 1,4 GB die 18 Divisionen durch `v_dc`
    (`svpwm` in ALIGN, RUNNING, BRAKING); `46_matrices`: 524 verschiedene
    `fma` in `f64` je Anfrage der Tiefe 10; `sim/14_7`: 81 Divisionen in
    `f64` im Induktionsschritt mit k = 10.
  - Umgesetzt: je Prüfstelle zuerst die Fehlerbedingung allein
    (`CheckSite::fail`, `smt::fail_query`), dann BMC und Induktion mit
    k = 1, erst dann die Tiefe; reicht der Speicher schon für k = 1 nicht,
    bleibt die Stelle mit diesem Grund offen. `Model::run` prüft in jedem
    Lauf, dass eine feuernde Stelle ihre Fehlerbedingung erfüllt.
  - Neu eingeordnet (`solver.yml`, Lauf 38199208294, nur die drei
    Programme, je ein Teil, 1:36 bis 2:12 h mit Bau), 119 Stellen:

    | Gruppe | Schritt 1, Tiefe 10 | Schritt 2, Tiefe 10 | Schritt 2, Tiefe 20 |
    |---|---|---|---|
    | Speicher | 106 | 68 | 63 |
    | bewiesen | – | 38 | – |
    | Frist | 13 | 13 | 18 |

    Bewiesen sind 20 Stellen von `11_foc_drive`, 4 von `46_matrices` und
    14 von `sim/14_7`, alle mit k = 1. Offen bleiben dort 32, 27 und 22;
    fünf in `46_matrices` scheitern schon mit k = 1 am Speicher.
  - Die Gruppe „nicht k-induktiv“ (FB-613) ist zum großen Teil tief
    erreichbar: Zähler laufen in Läufen ohne Stimulus bei t = 300 bis 2503
    über ihre Range. Invarianten (Schritt 2) tragen dort nicht, tiefe Pfade
    (Schritt 3) schon.
  - Weiter: tiefe Pfade (Schritt 3); für die Speicherfälle Rechnungen
    verschiedener Blätter teilen oder ein Solver mit kleinerem
    Bit-Blasting (FB-612).
- **Abnahme:**
  - `UNREACHED` ist leer, oder jede verbleibende Zeile hat eine Gruppe als
    Grund.
  - FB-511 ist behoben oder mit Rest begründet; das Abnahmekriterium von
    Schritt 29 ist in `plan/m11.md` nachgetragen.
  - Die Dauer des Pfadlaufs steht im Bericht.
  - Grün: `solver_paths` (Release), `the_three_executors_agree`, die Suite
    von `takt-prove` einschließlich `z3_and_cvc5_do_not_contradict_each_other`.
- **Hinweise:**
  - Parallele z3-Läufe sind schon am Speicher gescheitert (FB-510); die Zahl
    der Fäden bleibt begrenzt.
  - Spacer kennt kein Fließkomma; was dort offen bleibt, ist ein Grund, kein
    Fehler.

### 14 — Rust ohne RTOS und mit Frameworks

- **Stand 2026-10-11: umgesetzt.** 14a bis 14d auf beiden Boards bestanden
  (letzte Läufe 3d1caad, 1a9f995, ecb56b8, 2e3d5bb); die Einzelheiten stehen in
  der Zeile von Schritt 14 in `plan/m11.md`.
- **Ziel und Abnahme:** wie `plan/m11.md`: Interruptform auf beiden Boards,
  Pollform auf dem F401, Embassy auf beiden, RTIC; je eine Korpusauswahl
  neben einer fremden Hauptschleife bzw. Aufgabe, Traces gleich dem
  Interpreter.
- **Hinweise:** Board-Einrichtung siehe Gedächtnis (C6 an COM4, die Brücke
  an COM5 blockiert RX; F401 an COM7, DfuSe-Schreiber). Board-Läufe über
  `tools/board-run.ps1`.
- **Ausgangslage (2026-10-10):** Beide Boards laufen in der Form „eigener
  Kern“ mit periodischem Timer. Den Kern ohne Warten (`service`, `Next`) gibt
  es, die Form `--form interrupt|poll` setzt aber nur das Profil. RTIC hängt
  an `takt-rt-rtos` mit periodischem TIM2. Embassy fehlt ganz; dafür müssen
  `embassy-executor`, `embassy-time`, `embassy-stm32` und die Anbindung des C6
  aus crates.io nachgeladen werden. Die Hülle `Program` hält rohe Zeiger und
  ist nicht `Send`.
- **Teilschritte:**
  - **14a Interruptform**, je Board.
    1. Die 64-Bit-Zeit aus einem 32-Bit-Zähler und den Vergleich mit der Frist
       als reine Rechnung in `takt-board-support`, mit Tests auf dem Wirt.
    2. Der Port in `takt-rt-baremetal` (`interrupt`): Die Timer-ISR ruft
       `service_with`, stellt den Alarm auf `deadline`, löst bei `jobs` den
       Job-Interrupt aus und endet mit der Bilanz.
    3. Je Board ein Alarm: TIM2 freilaufend mit Compare (F401), SYSTIMER-Ziel
       (C6). Dazu die Uhr und ein Job-Interrupt niedrigster Priorität.
    4. Im Bring-up ein Feature je Form und eine fremde Hauptschleife, die
       zählt und die LED führt.
    5. In der Abnahme wird `Options::rtos` zu einer Form. Board-Tests: eine
       Korpusauswahl gleich dem Interpreter, `drift` im Bericht.
  - **14b Pollform** auf dem F401: Die Hauptschleife ruft `service` an
    `deadline`. `check_form` prüft die Regeln aus GEN-036 (Jobs nur mit
    Job-Interrupt; der Tick nicht kürzer als die längste Runde). Die
    Rundenlänge wird ein Schlüssel der Hardware-Konfiguration.
  - **14c RTIC** als Interruptform mit dem Planer von RTIC: die Grenze aus
    `deadline`; die Umbenennung von Feature `rtos`, `Options::rtos` und
    `takt-rt-rtos`.
  - **14d Embassy** auf beiden Boards: eine Aufgabe auf einem Executor, den
    ein freier Interrupt treibt, die Zeit aus `embassy-time` über einen
    Zeittreiber der Board-Crates. Umgesetzt mit dem rohen Executor: Der
    `InterruptExecutor` nimmt nur `Send`-Aufgaben, und für RISC-V gibt es
    ihn nicht; `embassy-stm32` und die Anbindung des C6 brauchte es nicht.
  - Unterwegs: FB-385 (der Port verlangt das Journal) und FB-458 (der
    Stack von `main` auf dem C6; behoben 2026-10-11).
  - FB-513: die Stack-Reserve je Form statt je Ziel (der Bench misst sie
    schon je Form; auf dem F401 2628 Byte im eigenen Kern, 3648 unter RTIC).
  - **FB-385, entschieden 2026-10-11:**
    - **Die Regel im Typ:** `takt_rt_core::Program` nennt, was das
      Programm braucht (`Volatile` oder `Persisting`; die Hülle setzt es,
      `Guarded` reicht es durch). Die `Runtime` hält das Journal selbst und
      lässt sich für ein Programm mit `persist` nur mit einem Journal bauen;
      ohne es kompiliert der Wirt nicht. Die Ports verlieren ihren Parameter
      `persist` und damit das stille `None`. Ein Programm ohne `persist` hat
      kein solches Argument. Dasselbe Muster für den Griff der Jobs.
    - **Das Journal des F401 im Flash:** Sektor 2 und 3 (je 16 KiB). In
      Sektor 1 (0x0800_4000, wohin der HID-Bootloader springt) liegt ein
      Sprungbrett: Es prüft die Anwendung ab Sektor 4 (0x0801_0000, 192
      KiB) und startet sie; bei leerer Anwendung geht es in den ROM-DFU.
      Sektor 0 bleibt unberührt. Der DfuSe-Schreiber schreibt das
      Sprungbrett nur, wenn es fehlt oder abweicht. `FlashNvm` für den
      F401 (blockierend, 12.3), Zeiten aus `takt bench`, und die
      Persistenz-Tests des C6 auch auf dem F401.
    - **Verworfen:** eine Laufzeitprüfung (Fehler erst am Board); ein
      flüchtiges RAM-Journal (die stille Ersatzlösung, die 12.11
      ausschließt); `persist`-Programme auf dem F401 auslassen (zehn
      Korpusprogramme weniger auf dem M4); eine eigene Startstufe in Sektor
      0 (gewinnt nur 16 KiB, braucht einen Eingriff in Sektor 0).
    - **Teil 2 umgesetzt 2026-10-11**: `FlashNvm`,
      Sprungbrett, Anwendung ab 0x0801_0000, Journal in allen vier Formen
      des F401, Journal-Tests auf beiden Boards. Das Messprogramm von
      `takt bench` passt mit 432 Byte Luft in die 192 KiB (`plan/f401.md`).
      Unterwegs FB-514: Der Stand der Board-Abbilder kannte die
      Hardware-Konfiguration nicht.
    - **Teil 1 umgesetzt 2026-10-11**: `Program::Memory`
      (`Volatile` oder `Persisting`, die Hülle setzt es), die Runtime hält das
      Journal als Typparameter; `Runtime::new` nur ohne `persist`,
      `Runtime::journaled` mit einem Journal, das es nur geladen gibt
      (`Unloaded::load`: ein ungeladenes schrieb still nie). Die Ports
      (`run`, `stats`, Interrupt- und Pollform) verlieren `persist`; Wirte
      für jedes Programm nehmen `Memory::store`. `compile_fail`-Doctests in
      `takt-rt-core` und im Beispiel `rust-host`.
    - **Griff der Jobs umgesetzt 2026-10-11**: Die
      Hülle nennt `takt_embed::Program::Work` (`NoJobs` oder `WithJobs`);
      `JobContext::start` beider Boards nimmt Stack, Verteiler und Griff ohne
      `Option` — drei Wege, den Job-Faden still abzuschalten, sind fort. Wirte
      für jedes Programm wählen über `Work::context`; `JobStack::bytes` ist
      ohne Jobs leer statt `None`. Die Runtime hält den Job-Kontext nicht:
      Er läuft in einem anderen Kontext als der Schritt (12.11).
    - **Boardlauf 2026-10-11, FB-385 behoben:** auf 3d1caad C6 26 von 26,
      F401 25 von 26. Der Watchdog-Test scheiterte, weil `NVM_BLOCKING_NS`
      auch ohne `persist` galt (Frist 522 statt 20 ms); behoben in 1a9f995,
      danach Watchdog, Pollform-Runde und Journal-Tests auf dem F401
      bestanden. Gemessen: Löschen 197 ms, Programmieren 0,14 ms; in
      `stm32f401.hw` mit Reserve 300 ms und 0,2 ms. Der Schlaffenster-Test
      verlangt jetzt zwei Schreibvorgänge, einen im Schlaf. KON1-022 ist
      geschlossen.

### 17 — Wirt mit Instanzen in Fäden, `linux_rt`

- **Abnahme:** acht Instanzen gleich dem Interpreter, ein Lauf im Tick-Faden
  unter Linux (Docker aus `tools/Dockerfile.linux`).
- **Mitgeführt:** FB-501 bleibt offen, bis ein Gerät der Klasse da ist; der
  Lauf unter qemu ist dafür kein Ersatz.

### 15, 16, 18, 9, 19 bis 23

Ziel und Abnahme stehen in `plan/m11.md`. Für die Zuteilung zählen hier nur
Bereiche, Boards und Abhängigkeiten aus der Warteschlange. Vor dem Start legt
der Strang in seinem ersten Bericht einen Plan von fünf bis zehn Zeilen vor:
berührte Crates, Prüfungen, offene Fragen.

### 30 — Wachsamkeit

- **Wann:** möglichst spät, nachdem B2 die Abdeckung gehoben hat. Vorher
  überleben Mutanten an Stellen, die kein Lauf erreicht, ohne Aussage.
- **Umfang:** `tools/mutants.sh` entsteht hier. Danach läuft es in jedem
  weiteren Punkt auf dessen Diff und gehört dort zur Prüfung vor der
  Übergabe.
- **Abnahme:** jeder Überlebende hat einen Test oder eine FB-Zeile; Ratsche
  je Crate.

### 24 — Abschluss

Zuletzt, wenn alle anderen Punkte gemergt sind: die Abnahme aus Abschnitt 0
von `plan/m11.md` vollständig, Inventur der Register.
