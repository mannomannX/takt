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
  Ab FB-760 (2026-10-20) je Strang zwanzig Nummern: I FB-760 bis FB-779,
  F FB-780 bis FB-799, B FB-800 bis FB-819, H FB-820 bis FB-839, G FB-840
  bis FB-859, die Koordination FB-860 bis FB-879. Ist ein Block
  aufgebraucht, vergibt die Koordination den nächsten.
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
| B2 | FB-511: jede Prüfstelle erreicht oder bewiesen unerreichbar | P, C | – | B1 | Schritt 1 umgesetzt und eingeordnet (CI-Lauf 38188098609); Schritt 2, Teil 1 (kleine Anfragen zuerst) umgesetzt und gemergt 2026-10-12, eingeordnet in 38199208294: Speicher 106 → 68, 38 bewiesen (FB-612); die Gruppe „nicht k-induktiv“ ist größtenteils tief erreichbar und gehört zu Schritt 3 (FB-613); Schritt 3 (tiefe Pfade) umgesetzt und gemergt 2026-10-12 (fd3a9386); FB-614 entschieden (`now` sättigt) und danach Schritt 4 bei Strang B (Zweig `b4-fb614-now`); Schritte 2 (Rest) und 5 frei |
| 14 | Rust ohne RTOS und mit Frameworks | R, E | beide | 10 | Strang A, umgesetzt 2026-10-11 |
| 17 | Wirt: Instanzen in Fäden, logische Zeit; `linux_rt` | E | – | 8 | Strang D (Helfer, seit 2026-10-12, Zweig `d-17-host-threads`): umgesetzt, CI grün auf `ubuntu-24.04` und `windows-2025` (Lauf 38249279690 auf d9b1f4a); wartet auf den Merge |
| 15 | `takt port-test` mit `--judge` | W, R | beide | 14 | Strang A, umgesetzt 2026-10-11 |
| 16 | Zwei Programme auf einem Kern; Antwortzeiten in `check-image` | R, W | beide | 11, 14 | frei |
| 18 | C-SDK und CMake; C-Ports Interruptform und POSIX (18a Wirt, 18b F401, 18c Linux) | S, W | F401 | 11, 15 | Strang A: 18a und 18c umgesetzt, der Watchdog als Dienst (Fassung 2 des SDK, 33f64ed1), Tunables und Weckereignis (3e1460c0), die Vorlage `c-port` als Adapter des gemeinsamen Ports mit Fäden (`ports/rtos`); 18b bei Strang G (Helfer, seit 2026-10-12, Zweig `g-18b-c-irq`): Port `ports/irq` mit Adapter STM32F4, C-Wirt `examples/boards/stm32f401`, der Board-Lauf steht aus |
| 9 | Ein Rahmen: Wirtsdifferential über den Produktrahmen | C, R, E | – | 8, 18a | Strang D (Helfer, seit 2026-10-12, Zweig `d-9-one-frame`): umgesetzt, CI grün auf `ubuntu-24.04` und `windows-2025` (Lauf 38302979240), Analyse grün (38303421868); FB-391 und FB-402 geschlossen, FB-382 bis auf Instanzen (FB-691); wartet auf den Merge; gemergt 2026-10-12 |
| 9b | Offene Befunde aus Schritt 9 (FB-686 bis FB-694) | C, E, R | beide (FB-687) | 9 | Strang I (Helfer, seit 2026-10-13): FB-687 und FB-693 gemergt (PR #9); FB-688 und FB-689 auf Zweig `i-688-admissible-inputs` (PR #14) umgesetzt, dabei FB-751 (Abort-Phase in Tick 0) gefunden und behoben, CI grün (38586622065), Analyse (38586622205), Board-Lauf ausstehend; FB-752 (Regionen der MPU, die ein Vorgänger stehen ließ) Empfängerseite auf `i-752-mpu-regions`; FB-694 entschieden (Laufende an der Achse, Revision von FB-614), vorbereitet auf `i-694-duration-end`, der Produktteil auf T1; FB-686 und FB-692 nach T1 (Strang H); FB-690 und FB-691 bei Strang H |
| 19 | Zephyr-Modul, Partition, Devicetree-Adapter; CMSIS-RTOS2 | S | beide | 18 | Strang F (Helfer, seit 2026-10-12, Zweig `f-19-zephyr`): 19a umgesetzt bis auf den Board-Lauf — Modul, Adapter `ports/zephyr` hinter `ports/rtos/takt_os.h`, `examples/zephyr` baut für beide Boards, gemergt cea0c88c; die Portprüfung auf den Boards fährt Strang A (`the_zephyr_port_passes_the_port_test`). 19b: Partition umgesetzt bis auf den Board-Lauf, Format des Devicetree-Adapters in der Referenz (FB-734), gemergt 9f0673d1; der Erzeuger `P_zephyr.c` nach Schritt 9 (Strang D). 19c: Vertrag `takt_os.h` erweitert (60aa91d0), Adapter `ports/cmsis-rtos2` und `examples/cmsis-rtos2` bauen für den F401; Portprüfung am Board offen |
| 20 | FreeRTOS auf dem F401, FreeRTOS-MPU | S | F401 | 18 | Strang G (Helfer, seit 2026-10-12, Zweig `g-20-freertos`): 20a Adapter `ports/freertos` und Wirt unter FreeRTOS umgesetzt, der Board-Lauf steht aus; 20b FreeRTOS-MPU (Zweig `g-20b-freertos-mpu`) umgesetzt bis auf die Board-Läufe |
| 21 | ESP-IDF auf dem C6 (21a Komponente und Port, 21b Funk und Messung) | S | C6 | 11, 18 | Strang G (Helfer, seit 2026-10-13, Zweig `g-21a-esp-idf`): 21a Komponente `esp-idf/takt`, Adapter `ports/irq/esp-idf`, Gerät `ports/esp-idf-nvm` und Wirt `examples/esp-idf` umgesetzt bis auf den Board-Lauf; 21b offen |
| 22 | Adapter und Senken | S | beide | 14, 19 | frei |
| 23 | Speicherschutz neben fremdem Code; `plan/cert.md` | S, R | beide | 10, 19, 21 | Strang F (seit 2026-10-13, Zweig `f-23-protection`): Kopfzeile `Schutz` in jedem Kopf, Urteil und Bericht, Tests je Fall, TCB-Zeilen in `plan/cert.md`, umgesetzt bis auf die Board-Läufe; offen ESP-IDF (FB-756, nach 21a) und das Fenster der Interruptform in C (FB-755) |
| 33 | Regelwerk für den eigenen Code (33a Katalog und Rust-Stufen, 33b C-Teil, 33c Miri) | E, R, S; Stufe 1 in allen | – | 33b: 18a | 33a bis 33c umgesetzt (33b/33c Strang C, gemergt 7b924a3; Board-Crates Strang E, gemergt 0a57fe7a); offen der Board-Lauf über den neuen Stand (Strang A) |
| 34 | Suite auf Runnern von GitHub (34a Linux, 34b Windows, 34c Solver-Läufe, 34d Board-Abbilder, 34e Board-Läufe ohne lokalen Bau) | C, W | – | – | 34a bis 34e umgesetzt (34d Strang B, 34e Strang A); offen der neue Bench nach dem Umbau der Runtime (Stack-Reserve `rtos` auf dem F401). Strang H (Helfer, seit 2026-10-12, Zweig `h-bench-split`): der Bench in Teilen mit Mindestreserve je Abbild (FB-740) und der Lastkern in Echtzeit, der nur seine Bilanz belegt (FB-741), umgesetzt, CI grün (38312676185), `boards.yml` baut beide Boards samt Bench-Abbildern (38312679018); gemergt (8a6b9d03), der Bench am Board bestanden (38320906796). Danach Zweig `h-fb742-clock` (PR #3): die Uhr des eigenen Kerns auf der Zeitachse von TIM2 und die Tickperiode je Ereignis (FB-742), `angehalten` in der Bilanz (FB-530); am Board bestanden (38331645022), main nachgeholt, wartet auf den Merge. Danach T1, die präfigierten Haken des Trace (Schritt 16) |
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
7. **Merge über einen Pull Request** (seit 2026-10-13). Die Koordination
   öffnet zum Antrag einen Pull Request gegen `main`. `ci.yml` und
   `analysis.yml` prüfen dort das Ergebnis des Merges, also den Zweig samt
   dem aktuellen `main`: Suite, Clippy, rustfmt und die Zahlen des
   C-Katalogs. Meldet der Pull Request einen Konflikt oder eine falsche Zahl,
   zieht der Strang `main` in seinen Zweig nach und behebt es dort. Ändert
   der Zweig Abbilder, fährt die Koordination die Board-Tests vor dem Merge.
   Gemergt wird bei Grün mit einem Merge-Commit; der Text der Statuszeile
   kommt im selben Zug in `plan/m11.md`. Die anderen Stränge ziehen `main`
   beim nächsten Halt nach.

   **Abbilder für den Board-Lauf** baut nur ein Lauf von `boards.yml` mit
   dem Ereignis `workflow_dispatch`; ein Lauf aus Push oder Pull Request
   fährt nur den Job `zephyr` und überspringt die Abbilder. Bestellt wird
   erst, wenn der Lint-Job der Board-Crates auf dem Kopf grün ist. Nach
   einem Merge von `main` in den Zweig gelten die Abbilder des Zweigs
   weiter: Am Board geprüft werden seine eigenen Änderungen, was `main`
   mitbringt, hatte seine eigenen Board-Läufe, und das Zusammenspiel prüft
   CI auf dem Merge. Neue Abbilder braucht es nur, wenn das Lösen eines
   Konflikts Code ändert, der ins Abbild geht.

   Steht ein grüner Pull Request nur in Plan-Dateien in Konflikt
   (`feedback.csv`, `features.csv`, `definition.md`, `m11*.md`), löst die
   Koordination ihn selbst: main in den Zweig, beide Seiten behalten, eine
   Zeile, die nur eine Seite änderte, in deren Fassung. Gemergt wird, wenn
   der Code-Diff des Zweigs gegenüber seinem vorigen Kopf genau der von
   main ist.

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
- **Stand nach Schritt 3 (2026-10-12, Zweig `b3-fb511-step3`):**
  - Der lange Lauf ohne Stimulus ist eine eigene Quelle für Pfade
    (`paths::from_run`): Der Interpreter rechnet jedes Programm des
    Vergleichs über `cases::MAX_TICKS` Ticks; was dort zum ersten Mal
    faultet oder genommen wird und kein Lauf ohne Pfad erreicht, bekommt
    einen Pfad `lauf-…` bis zwei Ticks danach. Ohne Solver und
    deterministisch: `the_stored_run_paths_are_current` rechnet sie in jedem
    Lauf der Suite neu (8 s im Debug-Bau) und hält sie gegen die
    gespeicherten, `UPDATE_RUN_PATHS` schreibt sie. Ein eigener Opt-in-Lauf
    ist dafür nicht nötig.
  - Elf Stellen bekamen so einen Pfad: 27_every, 37_follows, 41_tunables,
    50_clause_words und 76_stream_views aus FB-613, dazu 42_map,
    52_padding_fields, 53_stream_kinds, 55_frames_with_bytes, 97_fast_math
    und 120_port_writes (Fault bei t = 100 bis 1471).
  - `--depth auto` für die Pfade des Solvers ist nicht übernommen: Die
    Einordnung bei Tiefe 10 und 20 (die Fristen der meisten Programme
    brauchen k = 11) fand nur eine Stelle tiefer, in den großen Programmen
    scheitert schon Tiefe 10 am Speicher (FB-612), und `after 3 s` bei
    50 µs hieße Tiefe 2000. Stattdessen folgt der Solver einem Pfad, den
    Spacer jenseits der Tiefe sieht, mit BMC in doppelter Tiefe bis
    `takt_prove::DEEPEST` = `MAX_TICKS`, die Frist im Verhältnis der Tiefen
    (13.3): `99_exit_fault` faultet nach elf Faults von außen im Tick 72
    (BMC der Tiefe 80 in 106 s; `UPDATE_PATHS` für das Programm 145 s im
    Debug-Bau). Im Vergleich der drei Ausführer fand dieser Pfad FB-615: Das
    Modell meldete von zwei Faults einer Maschine in einem Tick — ein Fault
    von außen, dann einer in `enter:` des Ziels — nur den letzten; es führt
    sie jetzt hintereinander.
  - `UPDATE_PATHS` läuft als Opt-in-Lauf auf CI (`solver.yml`, Art
    `pfade`, Artefakt `pfade`); lokal reicht der Speicher für z3 nicht mehr.
    Für 03_sequences_and_faults, 06_test_harness und 120_port_writes in
    4 min (Lauf 38255550331): die drei Stellen, für die die Einordnung einen
    Pfad fand, haben jetzt einen, die Beweise dieselben Stellen mit k = 1.
    An zwei der neuen Pfade fand der Vergleich FB-616: Der Lauf des Modells
    gab einer gehaltenen `Stale`-Lieferung den Wert des Interpreters, den
    das Modell null hält.
  - Die `ovf`-Stellen an `now + d`: 3 laufen erst nach 2^63 ns über (Gruppe
    `Zeitschranke`); ein Beweis über die Zeitschranke wäre eine Annahme, und
    eine Prüfstelle steht unter keiner (13.3). Die 6 in `87_fault_kinds`
    sind unerreichbar, aber nicht k-induktiv. Was `now` nach 2^63 ns ist,
    sagt die Definition nicht, und Interpreter, Modell und Rahmen rechnen es
    verschieden (FB-614, inzwischen entschieden und behoben, unten).
  - Gruppen als Grund, für Spalte drei (Schritt 5): Jede Zeile von
    `UNREACHED` teilt ihre nie verletzten Stellen nach Gruppen auf
    (`GROUPS`, Test `every_never_failed_site_has_a_group`), aus den
    Einordnungen 38188098609 und 38199208294 und den Befunden dieses
    Schritts. Spalte drei 352 → 337, Programme 99 → 97:

    | Gruppe | Stellen |
    |---|---|
    | Speicher | 99 |
    | Frist | 82 |
    | ohne Teilmodell (FB-604) | 70 |
    | bewiesen, ohne Beweisdatei | 54 |
    | nicht k-induktiv | 12 |
    | Fragment | 12 |
    | ohne Urteil | 3 |
    | Zeitschranke | 3 |
    | jenseits `MAX_TICKS` | 2 |

  - Weiter: Schritt 4 (die 54 bewiesenen Stellen in die Beweisdateien,
    mit `solver.yml`, Art `pfade`), Schritt 2 für `nicht k-induktiv` und
    `Fragment`, Gruppen auch für Übergänge und nie bestandene Stellen
    (Schritt 5).
- **FB-614 (2026-10-12, Zweig `b4-fb614-now`):** Entschieden ist (a): `now`
  bleibt bei 2^63 − 1 ns stehen, wie der Interpreter es schon tat; ebenso
  `time_in_state` und jede Zeit aus Ticks mal Periode (3.3,
  Entscheidungslog). Das Modell sättigt sie und zählt den Tick für
  `last_fault.tick` selbst; beide C-Rahmen rechnen jede Zeit aus Ticks mit
  `takt_ticks_ns` und `takt_add_ns` (`parts::now`), ohne
  vorzeichenbehafteten Überlauf; der erzeugte Code sättigt `time_in_state`,
  `after` und das `now` der Monitore und fragt den Tick mit
  `P_tick_count` statt `now / T0`. Die Runtimes in Rust und der Port für
  Fäden sättigten schon. Drei Tests fahren die Grenze mit T0 = 30 000 Tage
  in der MIR (die Sema lässt Ticks bis 1 s zu): Interpreter gegen Modell,
  gegen Wirtsrahmen und erzeugten Code, und der MCU-Rahmen mit einer Falle
  für jeden Überlauf. Die 3 Stellen der Gruppe `Zeitschranke` bleiben
  erreichbar: Hinter der Grenze faultet `now + d`.
- **Stand nach Schritt 4 (2026-10-12, Zweig `b4-step4`):**
  - Die Pfade des Solvers fragen 60 s je Anfrage wie die Einordnung: Mit
    10 s fiel auf einem Runner ein Beweis aus `71_places` weg, und einer
    aus `46_matrices` kam nicht. Die Beispiele `sim/12_7` und `sim/14_7`
    bekommen Pfade und Beweise wie die Suite `beweiser`.
  - Erzeugt mit `solver.yml`, Art `pfade`, in vier Läufen: 38275739496
    (`11_foc_drive`, 1:25 h), 38275744284 (`46_matrices`, 36 min),
    38275748127 (`sim/14_7`, 55 min), 38275754060 (`sim/12_7`, 04, 36, 71,
    80, 87, 98; 1:41 h). Kein neuer Pfad.
  - Von den 54 Stellen der Gruppe „bewiesen“ führen die Beweisdateien jetzt
    47, dazu 18 weitere, die die Einordnung nicht entschied (13 in
    `11_foc_drive`). `sim/12_7` hat bewiesen nie genommene Übergänge.
    Übrig bleiben 7:
    - 4 nicht nachgerechnet (`36_int_units` `ovf`, `46_matrices` `fin`,
      `87_fault_kinds` `ovf`, `98_last_fault` `range`): Die Einordnung bewies
      sie mit Tiefe 10 oder Spacer und 6 GB, die Pfade des Solvers (Tiefe 5,
      60 s, 4 GB) nicht.
    - 2 außerhalb der Suite `beweiser` (`124_port_test`, Manifest).
    - 1 explizites `check` in `sim/12_7`: Die Beweisdatei führt nur
      implizite Prüfungen (13.3).
  - `UNREACHED`: Übergänge 102 → 101, nie bestanden 72 → 56, nie verletzt
    337 → 286, Programme 97. Der Vergleich der drei Ausführer bestätigt
    jeden neuen Beweis: Der Codegen lässt die Prüfung aus, der Interpreter
    prüft weiter, und kein Lauf weicht ab.
- **Stand nach Schritt 5 (2026-10-14, Zweig `b5-groups`):**
  - Jede Zeile von `UNREACHED` trägt jetzt für alle drei Spalten ihre
    Gruppen als Grund: Übergänge, nie bestandene und nie verletzte
    Prüfstellen. Geprüft wird das wie bisher, jetzt in
    `every_unreached_item_has_a_group`.
  - Das Modell kennt das Bestehen einer Prüfstelle, so wie die Coverage es
    zählt (13.2: ausgewertet ohne Fault, ein `check` ausgewertet):
    `Model::passes`, Ziel `Target::Pass`, `classify_passes`. Die Einordnung
    (`CLASSIFY_UNREACHED`, `CLASSIFY_WHAT`, `solver.yml` Eingabe `what`)
    fragt Übergänge und nie bestandene Stellen wie die Prüfstellen. Für eine
    Stelle, die kein Modell führt, nennt sie den Grund: eine Funktion, die
    keine Maschine ruft, eine bewiesene Range, eine Meldung oder eine
    Beobachtung.
  - Sieht Spacer einen Pfad jenseits der Tiefe, folgt die BMC ihm jetzt auch
    für Übergänge und das Bestehen einer Stelle. Neue Pfade:
    - `40_jobs`: RUN.S0->FAILED bei t = 100.
    - `87_fault_kinds`: RUN.S1->FAILED bei t = 101.
    - `63_scoped_instances`: HIGH->LOW je Instanz p, q und r. Geteilte
      Übergänge tragen die Maschine im Label; vorher blieb nur ein Pfad.
    - `sim/12_7`: drei Übergänge zu SELFTEST und weiter.
  - Die Einordnung fand zwei Fehler im Modell:
    - FB-617: Eine Lieferung eines Enums war im Modell jeder Code. Der
      Interpreter las den Stimulus nicht, die Pfade zu SELFTEST in
      `sim/12_7` blieben unbestätigt.
    - FB-618: Unter seiner Beweisdatei aus Schritt 4 war `sim/14_7` nicht
      mehr kodierbar (`cell_v.min()` direkt am Input).
  - FB-619 (entschieden: Option a, Zweig `b6-fb619`): Die Coverage zählt
    nur, was eine laufende Maschine erreichen kann (13.2). Eine Funktion,
    die keine Maschine ruft, ist kein Pfad des Laufs. In
    `02_units_and_data` fallen die drei Stellen von `shunt_current` weg,
    ebenso die Gruppe „nie gerufen“: nie bestanden 53 → 50, nie verletzt
    286 → 283.
  - Einordnung in den Läufen 38320013325 (alle Programme, Übergänge und
    Bestehen), 38319891273 und 38336280659 (`02_units_and_data`,
    `98_last_fault`, `sim/12_7`, `sim/14_7`, `63_scoped_instances`, alle
    drei Spalten). Pfade aus den Läufen 38319880152 und 38331751010.
  - `UNREACHED`: Übergänge 101 → 93, nie bestanden 56 → 53, nie verletzt
    286. Die Zeilen von `40_jobs` und `63_scoped_instances` entfallen, 95
    Programme. Die Gruppen:

    | Gruppe | Übergänge | nie bestanden | nie verletzt |
    |---|---|---|---|
    | Speicher (FB-612) | 13 | – | 96 |
    | Frist | 27 | 25 | 80 |
    | ohne Teilmodell (FB-604) | 31 | 5 | 70 |
    | Fragment | 16 | 6 | 12 |
    | nicht k-induktiv | 2 | – | 12 |
    | bewiesen, nicht nachgerechnet | 4 | – | 5 |
    | Range bewiesen | – | 6 | – |
    | in einer Meldung | – | 4 | – |
    | faultet stets | – | 3 | – |
    | unbestätigt (`inv`, `sha256`) | – | 1 | – |
    | Zeitschranke (FB-614, FB-694) | – | – | 3 |
    | jenseits `MAX_TICKS` | – | – | 2 |
    | außerhalb `beweiser` | – | – | 2 |
    | bewiesen, explizit | – | – | 1 |

  - Die Gruppen „Frist“ und „nicht k-induktiv“ schwanken von Lauf zu Lauf
    an denselben Stellen (Runner, Fristen von 60 s). Eine Zeile trägt
    deshalb die Gruppe ihres jüngsten Laufs.
  - Weiter: Schritt 2, Invarianten für „nicht k-induktiv“ und „Fragment“,
    dann der Speicher (FB-612); er beginnt, sobald FB-694 auf main ist.
  - **Geplante Abstraktion für „Fragment“ (Schritt 2):** Die Invariantensuche
    (`horn()` in `takt-prove/src/smt.rs`) lehnt heute jedes Ziel ab, in
    dessen Kegel Fließkomma, eine Schiebung oder die Wickelung in eine
    Breite steht. Künftig ersetzt sie jeden größten Teilterm außerhalb des
    Fragments, dessen eigene Sorte im Fragment liegt (ein Vergleich über
    Fließkomma ist ein Wahrheitswert, eine Wickelung eine ganze Zahl), durch
    eine frische Variable der jeweiligen Klausel, frei und unbeschränkt. Zustandsvariablen außerhalb
    des Fragments fallen aus `Inv` heraus und sind in jedem Schritt frei.
    - Soundness: Jeder ersetzte Term ist frei und unbeschränkt, also lässt
      jede Ersetzung nur Abläufe hinzu, auch wenn ein Ziel den Term liest.
      Schlimmstenfalls wird ein Beweis unmöglich, falsch wird keiner. Findet
      Spacer eine Invariante, gilt sie auch für das genaue Modell, und die
      Stelle ist bewiesen unerreichbar. Eine Prüfung auf Antitonie wie bei
      FB-694 braucht es nicht: Dort wurde ein Bit fest gesetzt, hier wird
      ein Term frei.
    - Findet Spacer einen Pfad, kann er an der Abstraktion liegen. Dann
      folgt die BMC am genauen Modell (`deeper`); bestätigt der Interpreter
      keinen Pfad, bleibt die Stelle unentschieden, mit dem Grund
      „Abstraktion“ statt „Fragment“.
    - BMC, Pfade und Houdini sehen weiter das genaue Modell, wie beim
      offenen Ende aus FB-694 (`proving`).
- **Stand nach Schritt 2, Teil 2 (2026-10-21, Zweig `b8-fragment`):**
  - Die Invariantensuche lässt kein Ziel mehr aus, nur weil Fließkomma,
    eine Bitoperation, eine Schiebung oder die Wickelung in seinem Kegel
    steht (13.3). Sie ersetzt jeden größten Teilterm außerhalb ihres
    Fragments, der selbst ein Wahrheitswert oder eine ganze Zahl ist,
    durch eine frische Variable, frei und unbeschränkt; Zustand außerhalb
    ist in jedem Tick frei. Eine gefundene Invariante gilt damit für das
    genaue Modell. Einem Pfad der Suche folgt `takt prove` nicht, denn er
    kann an einer freien Variable liegen (`Search::Abstract`).
  - Anders als geplant folgt die BMC einem Pfad der Suche nicht: Er gilt
    nur für die Abstraktion, und `deeper` liefe ohne Anhalt bis
    `MAX_TICKS`. Die Gruppe bleibt „Fragment“; der Grund sagt, dass der
    Pfad an einer freien Variable liegen kann.
  - Der Grund eines offenen Induktionsschritts nennt, wie die Suche endete:
    im Zeitlimit („Spacer im Zeitlimit (60 s)“), mit einem Pfad, der an
    einer freien Variable liegen kann, oder ungesucht. Die Gruppen bleiben.
  - Einordnung der neun Programme mit Stellen in „Fragment“, Basis main
    7bde65f7 (Lauf 39508468191) gegen den Zweig (39508472131), mit gleichen
    Parametern (Tiefen 10 und 20, 6 GB, zwei Fäden). Tiefe 20 fragt, was
    Tiefe 10 nicht bewies:

    | Gruppe | Basis, Tiefe 10 | Zweig, Tiefe 10 | Basis, Tiefe 20 | Zweig, Tiefe 20 |
    |---|---|---|---|---|
    | Fragment | 36 | 11 | 34 | 11 |
    | nicht k-induktiv | – | 20 | – | 18 |
    | Frist | 5 | 5 | 5 | 5 |
    | bewiesen | 3 | 8 | 2 | 2 |

  - Neu bewiesen, und die Pfade des Solvers (Lauf 39520803797) bestätigen
    es: in `114_for_pairs` zwei `range` und zwei `ovf` (vorher außerhalb:
    eine Schiebung), in `14_latency` der Übergang RUN->SAFE (vorher: `press`
    ist Fließkomma). Neu sind `114_for_pairs.takt-proof` und
    `14_latency.unfired`; beide Zeilen fallen aus `UNREACHED`.
  - 18 Stellen gehen von „Fragment“ nach „nicht k-induktiv“: Spacer läuft
    jetzt und endet im Zeitlimit. 17 davon sind der Nennablauf von
    `hotfire` (`03_sequences_and_faults`, 15) und `rig`
    (`06_test_harness`, 2), erreichbar, aber tiefer als BMC und die Läufe
    (Fristen bis k = 3001 bzw. 9001). Sie brauchen einen Pfad, keine
    Invariante (FB-803). Die achtzehnte ist `valid` in
    `04_blocks_and_multirate`.
  - 11 Stellen bleiben „Fragment“: Spacer sieht über die freien Variablen
    einen Pfad. Das sind `17_nested` (2) und `61_requirements` (6) an
    `p.x`, `53_stream_kinds` (1) und `88_capture_segments` (2) an der
    Wickelung.
  - Gefunden: FB-802, Spacer braucht für einen Rest über `div` das
    Tausendfache (am Prüfprogramm 112 s gegen 0,1 s).
  - Kosten: Der längste Teil der Einordnung brauchte 39 statt 10 Minuten,
    weil Spacer auf den Stellen von `hotfire` bis ans Zeitlimit läuft.
  - `UNREACHED`: Übergänge 93 → 92, nie bestanden 50, nie verletzt
    283 → 279; 93 Programme. „Fragment“ 16 / 6 / 12 → 3 / 3 / 5,
    „nicht k-induktiv“ 2 / 0 / 12 → 14 / 3 / 15 (Übergänge, nie bestanden,
    nie verletzt). `fin` in `04_blocks_and_multirate` steht jetzt unter
    „Frist“ statt „Speicher“, wie beide Läufe es einordnen (BMC im
    Zeitlimit, 6 GB): nie verletzt Speicher 96 → 95, Frist 80 → 81. Die
    übrigen Gruppen bleiben.
  - Weiter: Pfade für FB-803 (ein Stimulus, der die Eingänge frisch hält
    und `start` gibt, oder tiefes BMC), der Rest über `div` (FB-802), dann
    der Speicher (FB-612).
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
  - Spacer kennt kein Fließkomma; die Suche macht es frei (13.3), und was
    dort offen bleibt, ist ein Grund, kein Fehler.

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
  unter Linux. Statt Docker aus `tools/Dockerfile.linux` prüft das seit
  Schritt 34 CI auf `ubuntu-24.04`.
- **Mitgeführt:** FB-501 bleibt offen, bis ein Gerät der Klasse da ist; der
  Lauf unter qemu ist dafür kein Ersatz.
- **Stand (Strang D, 2026-10-12):** Acht Instanzen von
  `examples/rust-host/takt/instance.takt` (Eingang, Job je Runde, Schlaf)
  rechnen in acht Fäden in logischer Zeit, jede mit eigenem Startwert gleich
  dem Interpreter (`tests/instances.rs`). Der Port der Form `linux` ist
  `takt_rt_linux::port`: `spawn` legt den Tick-Faden an (Stack des Programms
  plus `HOST_STACK_BYTES`), bindet ihn und sperrt den Speicher, bevor das
  Programm im Faden entsteht; `serve` wartet mit `clock_nanosleep` auf jede
  Frist, die Jobs rechnen auf einem Job-Faden auf den übrigen Kernen, und ein
  fertiger Job weckt den Tick-Faden vor der Frist (Portvertrag 3,
  `RealtimeClock::wait_until_or`). Vier Programme der Form `linux` laufen
  zugleich in ihren Tick-Fäden gleich dem Interpreter, die Zusage im Kopf
  (`examples/rust-host/tests/linux_rt.rs`, allein aus
  `crates/takt-cli/tests/linux_rt.rs`). Befunde FB-680 bis FB-683; offen
  FB-683 (Stack-Reserve der Form `linux` gemessen) und FB-501.

### 19 — Zephyr

- **Zuschnitt:** 19a Modul und Port, 19b Devicetree-Adapter und Partition,
  19c CMSIS-RTOS2. Der Port ist kein eigener: Er ist der Port mit Fäden
  (`crates/takt-embed-c/ports/rtos`, Strang A) mit einem Adapter für Zephyr.
- **Abnahme 19a:** `examples/zephyr` fährt das Prüfprogramm auf
  `blackpill_f401cc` und `esp32c6_devkitc/esp32c6/hpcore`, und das Urteil
  der Portprüfung nimmt beide Läufe an. Was ohne Board geht, belegt der Bau.
- **Stand (Strang F, 2026-10-12):**
  - **Modul:** `zephyr/module.yml`, `zephyr/Kconfig` (`CONFIG_TAKT`,
    `CONFIG_TAKT_TARGET` mit Vorgaben für Cortex-M4 mit FPU und RV32IMAC,
    `CONFIG_TAKT_STEP_PRIORITY` unter den kooperativen,
    `CONFIG_TAKT_JOB_PRIORITY` unter den verdrängbaren), `zephyr/CMakeLists.txt`
    baut `ports/rtos/takt_rtos.c` mit `ports/zephyr/takt_os.c` und stellt
    `takt_add_program` aus `tools/cmake/TaktConfig.cmake` mit der Form `rtos`
    und dem Tripel aus Kconfig als Vorgabe (`TAKT_FORM`, `TAKT_TARGET`). Das
    CMake-Paket kennt dafür auch `BUILD sim|hw`.
  - **Adapter:** Ereignisse sind `k_sem` mit Grenze 1; Uhr und Fristen in
    Systemticks ab `takt_os_init`, die Frist aufgerundet: nie vor ihr,
    höchstens einen Systemtick danach, und genau auf ihr, wenn die Periode
    ein Vielfaches des Systemticks ist (das Beispiel nimmt 10 µs). Nicht
    `k_cycle_get_64`: Auf dem ESP32 haben Zyklen und Ticks verschiedene
    Ursprünge. Fäden mit `K_FP_REGS`, nicht `K_ESSENTIAL` (sie enden mit dem
    Lauf); `CONFIG_TAKT` wählt `FPU_SHARING`. Takt-Priorität `p` ist Zephyr
    `K_LOWEST_APPLICATION_THREAD_PRIO + 1 - p`, ein kooperativer Jobfaden ist
    `-EINVAL`. Der Wirt gibt das ganze Stackobjekt; der Adapter legt den Faden
    mit `bytes - K_THREAD_STACK_RESERVED` an, so misst der Port bis zum
    oberen Ende. `INIT_STACKS` und `STACK_SENTINEL` schließt Kconfig aus,
    weil sie die Farbe des Ports überschreiben.
  - **Beispiel:** Leitung über das UART-Gerät `takt,line` aus dem
    Devicetree, gesendet aus dem Interrupt, ein Paket je Interrupt, XON/XOFF
    (F401: USART1 mit 921600 Baud; C6: USB-Serial-JTAG). `TAKT` gibt das
    Board zurück: der F401 springt in den DFU-Bootloader des ROM wie das
    Bring-up, der C6 startet neu. Der F401 liegt ab Sektor 4 hinter dem
    Sprungbrett; der C6 bekommt 4 MB Flash (DevKitM-1-N4). Journal im RAM
    wie im POSIX-Beispiel (FB-732), Watchdog ausdrücklich ohne.
  - **Geprüft ohne Board:** `crates/takt-cli/tests/zephyr_example.rs`
    (`--ignored`, `TAKT_WEST`) baut für beide Boards und prüft Konfiguration,
    Symbole und die Lage des Abbilds; bestanden lokal in 186 s. Die
    Board-Tests `the_zephyr_port_passes_the_port_test` in
    `board_stm32f401.rs` und `board_esp32c6.rs` bauen mit
    `<target>/release/takt`, schreiben (F401 über DfuSe wie jedes Abbild, C6
    `zephyr.bin` mit `probe-rs --binary-format bin` an
    `CONFIG_FLASH_LOAD_OFFSET`) und urteilen.
  - **Befunde:** FB-730 (Enum-Größe clang gegen GCC auf ARM, Warnung beim
    Link), FB-731 (der Adapter fehlt in der Prüfung des C-Teils), FB-732
    (Journal im RAM), FB-733 (Zeitgeber ist der Systemtick, nicht die
    `counter`-API des Entwurfs).
- **Stand 19b (Strang F, 2026-10-12):**
  - **Partition:** `CONFIG_USERSPACE` im Beispiel; Schritt- und Jobfaden
    bleiben Supervisor-Fäden auf Kernel-Stacks (`K_KERNEL_STACK_DEFINE`,
    der Adapter legt sie mit `K_KERNEL_STACK_RESERVED` an), die Arena liegt
    in der eigenen Partition `takt_arena` (`K_APP_BMEM`), die keine Domäne
    eines Fadens im Benutzermodus enthält. Vor `takt_core_init` schreibt
    eine Probe im Benutzermodus in die Arena; `k_sys_fatal_error_handler`
    hält die Verweigerung fest und lässt nur die Probe beenden, jeden anderen
    Fehler hält er an. Die Kopfzeile vor der Marke sagt
    `takt schutz partition verweigert`, `offen` oder `ungeprüft`; der
    Board-Test verlangt `verweigert`. F401: Partition 16 KiB an
    0x2000_0000 (Zweierpotenz für die MPU), RAM 52,5 von 64 KiB. Die
    Kopfzeile `Schutz: Partition` im Lauf-Header gehört zu Schritt 23.
  - **gperf** für `CONFIG_USERSPACE`: 3.1 aus den GNU-Quellen nach
    `D:\takt-tools\gperf` gebaut (Prüfsumme und Signatur geprüft, README
    und Aktivierung nachgezogen); auf CI per apt.
  - **Devicetree-Adapter:** Format und Entscheidung in der Referenz (8.10
    „Treiber aus dem Devicetree“, Prüfung 60, Entscheidungslog, Anhang A;
    FB-734): `driver = "zephyr-gpio"`, `port` als Label, Prüfung und
    Erzeuger nur mit `--zephyr`. UART mit dem Ring aus Schritt 22, ADC
    nach einer Entscheidung über die Skalierung (FB-735). Der Erzeuger
    `P_zephyr.c` folgt nach Schritt 9 (Strang D berührt dieselben Dateien).
- **Stand 19c (Strang F, 2026-10-12):**
  - **Vertrag** (60aa91d0, mit der Koordination abgestimmt): `takt_os_now`
    ohne `const`; nur der Schrittfaden ruft `now` und `take` mit Frist, ein
    Adapter erweitert eine 32-Bit-Uhr ohne Atomik und wartet ferne Fristen
    in Stücken unter dem halben Umlauf. Malen und Messen sind Primitive des
    Adapters (`takt_os_paint` vor dem Anlegen, `takt_os_depth` nach dem Ende
    und vor `join`, Fehler statt Zahl, wenn er nicht messen kann); die
    Vorgabe `takt_rtos_paint`/`takt_rtos_depth` rufen POSIX, Zephyr und die
    Vorlage. Der Schrittfaden wartet am Ende auf die Endmeldung des
    Jobfadens; `takt_rtos_join` misst erst, dann wartet es ab, und lehnt ab,
    solange der Schrittfaden läuft. Katalog des C-Teils unverändert.
  - **Adapter** `ports/cmsis-rtos2`: ein binäres `osSemaphore` je Ereignis
    (die Thread-Flags rundet Zephyrs Schicht auf Millisekunden ab, FB-736),
    die Uhr aus `osKernelGetTickCount` auf 64 Bit erweitert, Fristen
    aufgerundet in Ticks, Fäden mit `osThreadNew` (joinable, Stack des
    Wirts, Priorität von Takt gleich `osPriority_t`, 0 `osPriorityNormal`),
    Ende mit `osThreadExit`, Messung mit `osThreadGetStackSpace` (Tiefe
    `bytes` minus frei). Ein schon beendeter Faden gilt als abgewartet
    (Zephyrs `osThreadJoin` lehnt ihn ab, FB-736).
  - **Modul:** `CONFIG_TAKT_PORT` wählt den Adapter (`TAKT_PORT_ZEPHYR` mit
    `!INIT_STACKS`, `TAKT_PORT_CMSIS_RTOS2` mit `CMSIS_RTOS_V2`).
  - **Beispiel** `examples/cmsis-rtos2` (F401): Leitung und Rückgabe aus
    `examples/zephyr`, Stacks `K_THREAD_STACK_DEFINE` mit
    `K_THREAD_STACK_SIZEOF`, ohne Partition. `zephyr_example.rs` baut es
    (`the_cmsis_rtos2_example_builds_for_the_f401`), der Board-Test
    `the_cmsis_rtos2_port_passes_the_port_test` fährt die Portprüfung.
- **Stand 19d (Strang F, 2026-10-12, Zweig `f-19d-flash`):**
  - **Journal im Flash (FB-732):** `ports/zephyr-nvm` (`takt_zephyr_nvm_open`,
    `CONFIG_TAKT_JOURNAL_FLASH`) über `flash_area` auf der Partition
    `takt_journal`, zwei Slots je die Hälfte, synchron, `blocking_ns` aus der
    Hardware-Konfiguration. F401 Sektor 2 und 3 wie die Bring-ups; C6 zwei
    Sektoren zu 4 KiB ab 0x3b_0000, weil das Abbild (Simple Boot ab 0x0)
    über 0x9000 reicht, wo die Bring-ups ihr Journal halten. Nicht NVS/ZMS:
    ein zweites Journal darunter, und ihre Speicherbereinigung löschte zu
    Zeiten, die Prüfung 32 nicht kennt. Beide Zephyr-Beispiele nutzen es;
    `TAKT_FRESH_JOURNAL` (CMake) löscht es für die Portprüfung, die mit dem
    Interpreter ohne gespeicherte Werte vergleicht. Board-Tests
    `the_zephyr_journal_survives_a_reset` (C6) und
    `the_zephyr_journal_survives_a_reset_and_a_new_image` (F401): Der zweite
    Lauf beginnt mit `kept` des ersten plus eins.
- **Stand 19e (Strang F, 2026-10-12, Zweig `f-19g-zephyr-ci`):**
  - **C-Teil unter Zephyr, cppcheck (FB-731 (B)):** `c_analysis.rs` bindet
    `port_test` zweimal wie die Beispiele unter Zephyr (Zephyr-Adapter und
    CMSIS-RTOS2, je mit `zephyr-nvm`), nur cppcheck, `unix32`, mit
    `tools/c-analysis/zephyr.cfg` statt der Köpfe; im Katalog der Teil
    „Zephyr“. Behoben, was es fand: Stacks, Ring und Gerät ohne externe
    Bindung, in der Funktion, die sie braucht (8.4, 8.7, 8.9), Casts
    zusammengesetzter Ausdrücke (10.8).
  - **Zephyr auf CI (FB-731 (A)):** Job `zephyr` in `boards.yml` statt eines
    eigenen Workflows: Bei einem Push auf Adapter, Beispiele, Modul, cfg oder
    Katalog baut er alle Beispiele (`zephyr_example.rs`) und fährt clang-tidy
    über ihre `compile_commands.json` gegen den Katalog (Teil „Zephyr-Bau“,
    samt `handback_stm32f401.c`). Zephyr v4.4.2 und SDK 1.0.1 kommen aus dem
    Cache (`.github/actions/zephyr`). Ein Lauf der Board-Abbilder richtet
    Zephyr ebenso ein, und die Zephyr-Tests der Suiten legen ihre Abbilder
    in den Zwischenspeicher der Harness (`zephyr-<tripel>-<stand>`): Sie
    kommen im selben Artefakt, und `board-run.ps1 -FromCi` fährt sie ohne
    `west`.
- **Stand 19f (Strang F, 2026-10-13, Zweig `f-19f-dt-drivers`):**
  - **Treiber aus dem Devicetree (FB-734):** `takt build --emit embed
    --zephyr` schreibt `P_zephyr.c` und `P_zephyr.h` (`takt_frame::zephyr`):
    je Kanal an `zephyr-gpio` eine `gpio_dt_spec` mit der Zusicherung
    `#if !DT_NODE_HAS_PROP … #error`, die Treiber in der Signatur aus `P.h`
    (ein Eingang, dessen Lesen scheitert, ist `BAD` mit Grund `DRIVER`),
    `P_alive_<gerät>` für reine Pin-Geräte und `P_zephyr_init` mit dem
    `safe`-Wert jedes Ausgangs. Prüfung 60 unter `--zephyr`
    (`check_zephyr`, Prüfungskorpus SC-60). Das Modul gibt `--zephyr` bei
    `HARDWARE` und übersetzt `P_zephyr.c` in `app`. `examples/zephyr-gpio`
    fährt die Messschleife auf den Drahtbrücken (F401 PA0→PA1, C6
    GPIO7→GPIO17), Board-Tests `the_zephyr_gpio_wire_follows_its_pin`. Im
    C-Teil die fünfte Lieferform `wire`: cppcheck mit dem Modell von
    `gpio.h` in `zephyr.cfg`, clang-tidy über die Baue.
  - **Fault-Handler (SDK 3):** Nach `takt_core_init` meldet ein
    abgewiesener Zugriff eines Fadens im Benutzermodus `Runtime(Hardware)`
    (`partition_guard`, `takt_core_hardware_fault`). Board-Tests
    `the_zephyr_partition_turns_an_access_in_the_run_into_a_fault` mit
    `TAKT_PROBE_IN_RUN`.
  - **Harness (FB-737, FB-739):** Zwei Bauten desselben neuen Stands
    räumten einander das Verzeichnis weg (F401-Abbild im Lauf 38321047944
    verloren); `enter_stand` räumt nur noch frühere Stände. Die Probe des C6
    las den Tick über JTAG nur an den Symbolen der Bring-ups; ein Abbild
    unter Zephyr nennt ihn über Arena (`arena`) und `tick_at` des Manifests
    (`zephyr::Image::tick_address`, Manifest im Zwischenspeicher). Der
    Bau-Test verlangt das für jedes Beispiel und Board. Offen: warum am C6
    unter Zephyr kein `takt end` auf der Leitung kam (erster Board-Lauf aus
    38320906796); das zeigt der nächste Lauf.
- **Stand 19g (Strang F, 2026-10-13, Zweig `f-19i-counter`):**
  - **Der Zähler als Uhr (FB-733):** Wählt der Devicetree `takt,timer`,
    rechnet der Zephyr-Adapter über die `counter`-API
    (`CONFIG_TAKT_TIMER_COUNTER`, hängt an `COUNTER`, das die Anwendung
    setzt; ohne es scheitert der Bau an einer Zusicherung): die Uhr auf
    64 Bit erweitert, die Frist als absoluter Alarm in Stücken von einem
    Viertel des Umlaufs, gelöscht nach dem Aufwachen. `examples/zephyr`
    wählt TIM2 (F401) und Timer 0 (C6) mit 1 MHz; die Kopfzeile `takt uhr
    counter 1000000 Hz` verlangt die Portprüfung auf beiden Boards. Die
    übrigen Beispiele bleiben beim Systemtick und nennen ihn ebenso.

### 23 — Speicherschutz neben fremdem Code

- **Stand 23 (Strang F, 2026-10-14, Zweig `f-23-protection`):**
  - **Kopfzeile:** `  Schutz: Fenster|Partition|keiner` vor der Marke,
    eingerückt wie `  Form …`. Rust: `takt_rt_core::protection::Case`,
    `takt_rt_baremetal::Protection::CASE`; die Bring-ups schreiben den Fall
    ihres Schutzes. C: `TAKT_OS_PARTITION` je Adapter,
    `TAKT_RTOS_PROTECTION` (`ports/rtos/takt_rtos.h`) und
    `TAKT_IRQ_PROTECTION` (`ports/irq/takt_irq.h`); jeder C-Wirt schreibt
    sie in seinem Kopf mit einem Aufruf. Nach T1 (Senke als Argument der
    Trace-Haken) ziehen nur diese Aufrufe nach.
  - **Urteil und Bericht:** `port::judge` verlangt die Kopfzeile, nimmt die
    letzte vor der Marke und prüft sie gegen den Nachweis der Probe;
    `PortEntry::protection`, Bericht Version 5.
  - **Board-Tests:** `the_port_passes(board, form, Case)`,
    `zephyr_verdict(…, Case, …)`; der Fall je Port wie in `plan/cert.md`.
    Der Zugriff der TCB zwischen zwei Ticks läuft in jeder Form des F401.
  - **Offen:** ESP-IDF (FB-756), das Fenster der Interruptform in C
    (FB-755). `examples/zephyr-gpio` (nach PR #4) nennt `keiner`, und
    `the_zephyr_gpio_wire_follows_its_pin` verlangt es.
- **Stand FB-735 (Strang F, 2026-10-20, Zweig `f-735-dt-adc`):**
  - **ADC:** `zephyr-adc` in `takt_frame::zephyr` neben `zephyr-gpio`:
    `adc_dt_spec` aus dem Knoten mit `io-channels` (Bindung
    `takt,adc-input`, `zephyr/dts`, `dts_root` in `zephyr/module.yml`), eine
    Wandlung je Tick, Mikrovolt mit `adc_raw_to_microvolts_dt`, ein
    differentieller Kanal als Zweierkomplement in 16 Bit; `linear(a, b)`
    als Literale in Fließkomma, ohne Kalibrierung `int[uV]` mit
    Bereichsprüfung des Typs. Prüfung 60 (`check_zephyr`) hält Eingang,
    Rohtyp `i32` und Typ fest; `table(…)` lehnt sie ab (FB-780).
  - **Beispiel:** `examples/zephyr-adc` (Pin PA0 an PA1 = ADC1 Kanal 1 am
    F401, GPIO7 und ADC-Kanal 0 am C6), Schutz `keiner`. Bau-Test
    `the_zephyr_adc_example_builds_for_both_boards`, Board-Test
    `the_zephyr_adc_follows_its_pin` nur am F401 (der C6 hat die Brücke
    an GPIO17, keinem ADC-Pin); C-Analyse bindet das Beispiel mit.
  - **Offen:** Board-Lauf am F401; UART nach dem Ring aus Schritt 22a
    (Strang B).

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
