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
| B1 | FB-497 Rest: Maps und Puffer als Arrays der SMT-Theorie | P | – | 29 | Strang B |
| B2 | FB-511: jede Prüfstelle erreicht oder bewiesen unerreichbar | P, C | – | B1 | Strang B, nach B1 |
| 14 | Rust ohne RTOS und mit Frameworks | R, E | beide | 10 | Strang A |
| 17 | Wirt: Instanzen in Fäden, logische Zeit; `linux_rt` | E | – | 8 | frei (nicht neben 14) |
| 15 | `takt port-test` mit `--judge` | W, R | beide | 14 | frei |
| 16 | Zwei Programme auf einem Kern; Antwortzeiten in `check-image` | R, W | beide | 11, 14 | frei |
| 18 | C-SDK und CMake; C-Ports Interruptform und POSIX (18a Wirt, 18b F401, 18c Linux) | S, W | F401 | 11, 15 | frei |
| 9 | Ein Rahmen: Wirtsdifferential über den Produktrahmen | C, R, E | – | 8, 18a | frei |
| 19 | Zephyr-Modul, Partition, Devicetree-Adapter; CMSIS-RTOS2 | S | beide | 18 | frei |
| 20 | FreeRTOS auf dem F401, FreeRTOS-MPU | S | F401 | 18 | frei |
| 21 | ESP-IDF auf dem C6 (21a Komponente und Port, 21b Funk und Messung) | S | C6 | 11, 18 | frei |
| 22 | Adapter und Senken | S | beide | 14, 19 | frei |
| 23 | Speicherschutz neben fremdem Code; `plan/cert.md` | S, R | beide | 10, 19, 21 | frei |
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
  - **14d Embassy** auf beiden Boards: eine Aufgabe auf einem
    `InterruptExecutor`, die Zeit aus `embassy-time`.
  - Unterwegs: FB-385 (der Port verlangt das Journal) und FB-458 (der
    Stack von `main` auf dem C6).

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
