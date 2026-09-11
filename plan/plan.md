# Takt — Umsetzungsplan (von v0.2.8 zur vollständig implementierten, dokumentierten Sprache)

## 0. Was „fertig" heißt

Die Sprache ist fertig, wenn (1) jeder Eintrag der Feature-Inventur (Abschnitt 6) für die Stufen v1, v1.1 und v1.2 grün ist und die v2-Einträge (verteilte Ausführung, 12.9) mindestens als Grammatik, MIR-Platzhalter und „gilt schon heute"-Prüfungen abgedeckt sind, (2) alle acht Beispiele der Referenz auf Interpreter, x86-64, aarch64, Cortex-M4F und RV32IMAC bitidentische Traces liefern, (3) die Compiler-Prüfungen aus Abschnitt 10 je einen positiven und einen negativen Test besitzen (aus den 58 der Fassung v0.2.8 sind mit den Praxisberichten 63 geworden), (4) die Konformitätssuite (13.8) je Zielklasse veröffentlicht ist und (5) die Referenz und die Implementierung keinen offenen Widerspruch mehr haben. v2/v3 (verteilte Ausführung, VM, qualifizierbarer Codegen) sind nicht Teil dieses Plans.

Die Inventur wurde maschinell aus der Referenz v0.2.8 extrahiert (`plan/features.csv`, erzeugt von `plan/extract_features.py`; heute 1 065 Einträge, anfangs 1 055 auf mehreren Ebenen: 113 Grammatikproduktionen und ihre Alternativen — 17 `system`-Einträge, 23 Channel-Attribute, Typkonstruktoren, Statements, Sequenz-Items, Temporaloperatoren, Generics-Klassen und -Fähigkeiten —, 136 Schlüsselwörter und 24 reservierte Wörter, 56 reservierte Membernamen, 103 Semantikabschnitte und 106 Absatzregeln, 20 `exec`-Regeln und 14 Semantikfunktionen aus §9, 14 Desugaring-Regeln, 58 Compiler-Prüfungen (heute 63), 16 Beweisverpflichtungen und 7 Sätze (heute 8), 13 Fault-Arten, 4 Qualitäten, 26 Bibliothekseinträge, 5 versionierte Formate, Profile, 9 Regeln der verteilten Ausführung, 15 Werkzeugkommandos, 8 Beispiele, 24 Leitentscheidungen und 101 Entscheidungslog-Zeilen). Eine programmatische Gegenprobe bestätigt, dass jeder der 24 Punkte des v0.2.8-Reservierungspakets auf mindestens eine Inventur-ID abgebildet ist. Die Inventur ist der Ausgangspunkt, nicht das Endprodukt: Sie wird einmal kuratiert (Einträge gruppieren, Meilensteine bestätigen) und danach nur noch über Änderungen an der Referenz fortgeschrieben — die Extraktion ist ein Skript und läuft bei jeder Änderung erneut.

---

## 1. Vier Vorgehensweisen, gegeneinander gewogen

| Ansatz | Kern | Stärken | Schwächen | Doppelarbeit |
|---|---|---|---|---|
| **A Schichtweise** (Parser → Sema → MIR → Interpreter → Codegen → Runtime) | jede Schicht vollständig, dann die nächste | klare Verantwortung, wenig Umbau innerhalb einer Schicht | Integration erst am Ende; semantische Fehler zeigen sich spät; keine Hardware-Rückmeldung über Monate | gering, aber späte Erkenntnisse erzwingen Rückbau |
| **B Vertikale Scheiben** („Walking Skeleton") | kleine Teilmenge Ende-zu-Ende (Parser bis Hardware), dann verbreitern | frühe Integration, echte Rückmeldung, Toolchain-Risiken früh sichtbar | Gefahr, IR und Runtime für die Teilmenge zu bauen und später umzubauen, wenn Streams, Jobs, `follows` kommen | hoch, wenn die IR nicht von Anfang an für die volle Referenz entworfen ist |
| **C Semantik zuerst** (vollständiger Referenzinterpreter vor jedem Codegen) | Interpreter = ausführbare Spezifikation; Codegen als validierte Übersetzung | maximale Wiederverwendung (Orakel, Const-Auswertung, Simulationskern), die Semantik ist getestet, bevor LLVM sie festzementiert | Performance- und Hardware-Rückmeldung spät; Runtime und Treiber warten | gering |
| **D Beispielgetrieben** (bauen, was 14.1–14.8 braucht) | pragmatisch | schnelle sichtbare Fortschritte | strukturlos, Features zwischen den Beispielen fallen durch; genau das Vergessensrisiko, das vermieden werden soll | mittel |

Keiner der reinen Ansätze reicht. A und C sind zu spät bei der Hardware, B baut ohne Vorsorge zweimal, D vergisst. Der tragfähige Ansatz kombiniert **C als Rückgrat, B als Taktgeber und die Inventur als Vergessensschutz**, mit einer Vorentscheidung, die alle drei erst möglich macht: die MIR wird vor dem ersten Codegen für die *volle* Referenz entworfen.

---

## 2. Der Ansatz: MIR-zentriert, interpretergetrieben, inventurgeführt

**Prinzip 1 — Ein Werkstück für alle: die MIR.** Jede Komponente (Interpreter, Codegen, Analysen, Simulation, `takt size`, Orakel, Importer) arbeitet auf derselben mittleren IR. Sie wird in Meilenstein 0 gegen die vollständige Referenz entworfen: für jede Grammatikproduktion und jeden Semantikabschnitt der Inventur gibt es einen MIR-Knoten oder eine Desugaring-Regel — auch für Konstrukte, die erst in v1.1 implementiert werden (`job`, `follows`, interne Streams, `tunable`, `persist`, `port`). Nicht implementiert heißt dann: der Knoten existiert, der Interpreter meldet „nicht unterstützt". Das ist der Unterschied zu B: nichts wird für die Teilmenge gebaut und später umgebaut.

**Prinzip 2 — Der Interpreter ist die Spezifikation.** Der Referenzinterpreter (13.1) implementiert 9.x wörtlich (ASCII-Regeln → Rust-Funktionen mit denselben Namen: `exec`, `step_m`, `resolve_m`, `dispatch`, `tick`). Er ist zugleich Simulationskern der ersten Phasen, Const-Auswerter (11.3) und Orakel für den Codegen. Jede Semantikfrage wird zuerst im Interpreter entschieden und als Test festgehalten; LLVM übersetzt danach etwas, das bereits definiert und getestet ist.

**Prinzip 3 — Differentielles Testen als Abnahme.** Eine Sprachfunktion ist fertig, wenn Interpreter und nativer Code auf allen Zielklassen bitidentische Traces liefern (Satz 9.4.4 als Test). Der Korpus: die Beispiele der Referenz, die Konformitätstests je Inventur-ID, und zufällig erzeugte wohlgeformte Programme (Grammatik-Fuzzer).

**Prinzip 4 — Eine Quelle je Artefakt.** Was mehrfach gebraucht wird, existiert einmal: die Grammatik als Datei (Parser, Formatter, Fuzzer, Dokumentationsschnipsel); die Standardbibliothek in Takt selbst (Blöcke wie `pid`, `lowpass` werden vom Interpreter *und* vom Codegen aus derselben Quelle verarbeitet); native Funktionen und `libtaktm` als ein `no_std`-Crate, das Host-Interpreter und Targets teilen; die Runtime als Kern-Crate mit Profil-Aufsätzen; Treiber als Traits, deren Simulationsimplementierung der Sim-Backend ist (die Sim/HW-Umschaltung ist ein Treiberwechsel, kein zweiter Programmpfad).

**Prinzip 5 — Die Inventur regiert.** Jeder Commit referenziert Inventur-IDs; ein Dashboard zeigt je ID Parser/Sema/Interpreter/Codegen/Runtime/Test-Status; eine Referenzänderung ist erst gültig, wenn die Inventur nachgezogen ist (ADR-Prozess). Kein Feature ist „fertig", das nicht in der Inventur grün ist, und kein Feature ist „vergessen", das in der Inventur steht.

---

## 3. Werkstücke und Wiederverwendung

| Werkstück (Crate) | Inhalt | Genutzt von |
|---|---|---|
| `takt-syntax` | EBNF als Datei, Tokenizer (INDENT/DEDENT), Parser, AST, Formatter, Grammatik-Fuzzer | Sema, LSP, Importer, Doku-Tests |
| `takt-sema` | Namen, Typen (Einheiten, Ranges, Qualität, `T?`/`T!E`, dimensionierte Matrizen), Flussanalysen, Muster → DFA, Maschinen-Wohlgeformtheit, Prüfungen 1–58 | MIR-Erzeugung, LSP, Importer |
| `takt-mir` | MIR-Typen, Desugaring (Sequenzen, `->` in Segmenten), Scheduling-Zähler, Kostenmodell (Klassen), Speicherbudget (`takt size`), Darstellungsverengung | Interpreter, Codegen, Analysen, Importer |
| `takt-interp` | ausführbare Semantik 9.x über MIR; Const-Auswertung; Orakel-Rahmen | Simulation, Tests, Codegen-Abnahme, Importer |
| `takt-llvm` | MIR → LLVM IR (strikte FP), Targets, Instrumentierung, Debug-Info | `takt run`, Profile |
| `takt-rt-core` | Tick-Schleife, Prozessabbild (Doppelpuffer), Streams (Byte-Ringe), Fault-Wald, Abort-Phase, Zähler, `sched`, Jobs, Recording-Schnittstelle — `no_std`, ohne Allokation | alle Profile |
| `takt-rt-linux`, `takt-rt-baremetal`, `takt-rt-rtos`, `takt-rt-boot` | Profilaufsätze: Threads/ISRs, Treiberbindung, Telemetrie, NVM-Journal, Schlaf | Ziele |
| `takt-hal` | Treiber-Traits (Skalar, Stream, geplante Ausgabe, Flash-Gerät), Rand-Selbstprüfungen (12.6), Simulationstreiber | Runtime, Simulation, `driver-test` |
| `libtaktm` + `takt-native` | korrekt gerundete Mathematik beider Breiten, `fma`, Schnellvarianten, Chunk-Natives, Jobs — ein `no_std`-Crate | Interpreter (Host) und Targets |
| `takt-stdlib` (in Takt) | Blöcke, Funktionen, Protokollpakete, Simulationsmodelle (`flash_model`, Plant-Modelle) | jedes Programm |
| `takt-conformance` | Testkorpus je Inventur-ID, Golden-Traces, Kalibrierung (`c_target`, `guard`, `jitter`), Subnormal-Vektoren, `takt bench` | CI, Zielklassen |
| `takt-cli`, `takt-lsp`, `takt-import-c` | Kommandos, Editor, C-Frontend mit Klassifikation und Orakel-Modus | Nutzer |

Die Namen und Zuschnitte sind deckungsgleich mit Abschnitt 11.1 der Referenz; die Spalte „Genutzt von" ist die Zutat des Plans. Die Matrix macht die Doppelarbeit sichtbar, die der Plan vermeidet: Es gibt keinen zweiten Interpreter für die Simulation, keinen zweiten Compiler für Blöcke, keine zweite Mathematikbibliothek, keinen zweiten Programmpfad für Simulation.

---

## 4. Abhängigkeitsfolge und Meilensteine

```
M0 Fundament ──► M1 Kernsemantik ──► M2 Ströme/Protokolle ──► M3 statisches Gate ──► M4 Codegen Linux
                     │                                                                     │
                     └────────────── M5 Embedded (nach M4) ◄──────────────────────────────┘
                                          │
                                          ▼
                                    M6 v1.1-Vertikalen ──► M7 Werkzeuge/Migration ──► M8 v1.2
```
Innerhalb eines Meilensteins gilt B (Scheiben Ende-zu-Ende bis zur jeweils höchsten vorhandenen Schicht); zwischen Meilensteinen gilt die Reihenfolge, weil jede Stufe die Schnittstelle der nächsten festlegt.

| Meilenstein | Inhalt | Abnahmekriterium (Exit) |
|---|---|---|
| **M0 Fundament** | Workspace; Inventur kuratiert und mit Meilensteinen versehen; Grammatik als Datei — einschließlich der reservierten Produktionen späterer Stufen (`generic_vars` mit `type`/`const`, `node_decl` und Platzierung, `instance_decl` im Zustand, `resume`, `capture`, `arm_stmt`, `property_decl`/`tprop`, `enum … open`, `language`); Tokenizer, Parser für die *vollständige* Grammatik, Formatter; AST; Fehlerrahmen mit Positionen und Vorschlägen; **Edition** (`language`, Warnung bei Fehlen, Hash-Anteil), Prüfung reservierter Wörter und Membernamen, Regel der offenen Enums; **MIR-Entwurf gegen die volle Referenz** (Dokument + Rust-Typen) mit Platzhalterknoten für gescopte Instanzen, `resume`-Pfade, Trigger-Handles, Captures, Eigenschaftsmonitore und Knotenplatzierung; versionierte MIR-Serialisierung; CI mit Inventur-Dashboard | alle acht Beispiele und alle Grammatik-Schnipsel der Referenz parsen und überstehen den Formatter-Roundtrip; jede Grammatik-ID hat einen Parse-Test; Konstrukte späterer Stufen werden geparst und mit „ab v1.1/v1.2/v2" beantwortet; MIR-Review: jede SEM-/G-/PAR-ID ist einem Knoten oder einer Desugaring-Regel zugeordnet; Prüfungen 49–51 |
| **M1 Kernsemantik** | Sema für Skalare, Einheiten (inkl. affin), Ranges, Qualität, `T?`, Records, Enums, `match`; MIR v1: Maschinen, Zustände, Transitionen, Checks, `enter`/`exit`/`loop`, Sequenz-Desugaring, Timer, Fault-Wald, Abort-Phase mit Latch, Zähler-Scheduling, Multirate, `pub var`, Signale; Interpreter für 9.2–9.5, 9.9-Vorbereitung; Skalarkanäle mit `sim`-Bindung, Modelle als Maschinen; `takt check`, `takt sim` | 14.1–14.5 laufen im Interpreter mit Golden-Traces; Lemma 9.3.1, Sätze 9.4.1/9.4.2 als Tests (Ordnungsunabhängigkeit: Schrittreihenfolge permutieren, Trace gleich); Prüfungen 6–12, 14–16 |
| **M2 Ströme und Protokolle** | Streams mit Cursor und Byte-Ring, Muster → DFA (Alphabetklassen, ein DFA je Zustand), Handler, `until … matches`, Ausgabeströme, interne Streams, `layout` inkl. Bitfelder/Diskriminanten/`len_field`, `T!E`, Bereichsmuster, `at`/`pulse`/`cancel`, `samples` | 14.6 läuft im Interpreter gegen ein UART-Modell; Lemma 9.6.1 als Test; Prüfungen 17–21, 43, 45–47 |
| **M3 Statisches Gate** ✔ | Intervallanalyse mit impliziten Prüfungen und Warnpolitik, Dominanz, Definite Assignment je Eintritt, Single-Writer, `follows`-DAG, Kostenmodell nach Klassen, `takt size` mit Overlay/Scratch/Byte-Ringen, Darstellungsverengung als MIR-Annotation, Schedulability; die „gilt schon heute"-Prüfungen der verteilten Ausführung (58: `follows` knotenlokal, `hops` aus der Topologie — mit einem Knoten trivial). **Hinzugekommen:** Safe-State-Latenz mit `within` (Satz 9.4.5, Prüfung 61), Budget je Maschine (Prüfung 62), zwei Lints (Prüfung 63) | jede der 58 Prüfungen ist `fertig` oder `definiert`, keine `offen`: `fertig` mit positivem *und* negativem Test, `definiert` mit getesteter Ablehnung, solange ihr Konstrukt eine Stufe meldet (19 Prüfungen); die vier Prüfungen, deren *Eingabe* fehlt (28, 29, 32, 39 brauchen die Hardware-Konfiguration aus 8.10 bzw. die Kalibrierung aus 13.8), sind nach M6 verschoben, wo beide entstehen — M3 baut für sie die Rechnung (`takt size`, die Budgetvektoren), nicht das Urteil. Kennzahl impliziter Prüfungen im Report, nach Ursache aufgeschlüsselt (3.4); Lemma 3.4 als Test (Verengung ändert keinen Trace); Entwurf und Begründung in `plan/m3.md` |
| **M4 Codegen Linux** | `takt-llvm` für x86-64/aarch64, strikte FP, IEEE-Modus, `fma`; `libtaktm` beider Breiten mit Konformitätsvektoren; `takt-rt-core` + `linux_rt` (PREEMPT_RT, Doppelpuffer, Treiber-Threads, Rand-Selbstprüfungen); Simulationstreiber als HAL-Implementierung; Record/Replay; `takt run`, `takt replay` | differentielles Testen: Interpreter ≡ nativ (x86-64 ≡ aarch64) auf Korpus und Fuzzer; 14.1–14.6 in Echtzeit auf der Box; Replay reproduziert Läufe bitgenau |
| **M5 Embedded** | `baremetal` auf je einem Cortex-M4F- und RV32IMAC-Board; Tick-Quelle, Timer-Compare für `at`, ADC-DMA für `samples`, Stack-Zusammensetzung mit Schutzbereich, XIP-Regeln, MPU-Regionen; `takt bench`, Kalibrierung `c_target`/`guard`/`jitter`; `driver-test` | 14.7 auf Hardware mit HIL-Checks; Traces bitidentisch zu Interpreter und Box; Konformitätsbericht je Zielklasse |
| **M6 v1.1-Vertikalen** (jede Ende-zu-Ende: Sema → Interpreter → Codegen → Runtime → Konformität) | Jobs und Chunk-Natives; Konstantenvariablen in Generics `[const N]` (3.12); `persist` mit Journal und Stromausfall-Kampagne; `idle`/Systemschlaf; `tunable`; `follows`; Szenarien und Kampagnen; dimensionierte Matrizen; Oktagone; Einheiten auf Integern; Geräteprofile (8.10); System-Channels; Profile `rtos` und `boot`; Projekt-Natives; `property` als beschränkte Temporallogik mit Monitoren in Simulation und Hardware sowie `takt prove` (k-Induktion/BMC); `map<K, V, N>` mit deterministischem Hash; Operator-Metadaten; Leser für ältere Aufzeichnungs- und Konfigurationsformate | 14.8 auf Hardware mit Flash-Modell-Kampagne; Satz 9.9.1 als Test (Trace mit und ohne Schlaf gleich); Eigenschaftsmonitore bitidentisch zwischen Interpreter und Hardware; `map`-Iteration bitidentisch über Zielklassen; Inventur v1.1 grün |
| **M7 Werkzeuge und Migration** | LSP mit Live-Zustandsanzeige; `takt import-c` (Klassifikation, Abbildung, Check-Einfügung); Orakel-Modus; Protokollpakete | ein realer C-Baustein migriert und per Orakel abgenommen |
| **M8 v1.2** | Gescopte Instanzen (Lebenszyklus in `switch`, Spitzenlast über Konfigurationen, Overlay), `resume` (tiefe History, `saved` außerhalb des Overlays), Trigger mit `arm`/`disarm`/`fired`/`armed` (Knotenregel, Simulation mit `bound`), `capture<T, N>` als Stream-Element mit Armierung, Generics über Typen (Monomorphisierung, Fähigkeiten, azyklischer Instanziierungsgraph), Treiberstufe `port`, Anforderungsreferenzen, beschränkte QP-Löser; Prüfungen 52–55 aktiv | Inventur v1.2 grün; Konformität aktualisiert; Satz 9.4.1 mit gescopten Instanzen als Test (Aktivität aus der Konfiguration zu Tick-Beginn) |
| **M9 v2 (außerhalb dieses Plans, vorbereitet)** | Verteilte Ausführung nach 12.9: Knotenticks, `hops`-Verlauf von Ψ, Abort über Knoten, Verbindungsverlust als Degradation/Fault, Aufzeichnung je Knoten, atomares Deployment; Bytecode-VM als Verbraucher der versionierten MIR | MIR-Platzhalter, Grammatik und Prüfung 58 existieren seit M0; keine Änderung an v1-Programmen nötig |

Warum diese Reihenfolge und keine andere: M0 enthält das gesamte Reservierungspaket (Editionen, reservierte Namen, Grammatik und MIR-Platzhalter aller späteren Konstrukte), weil jede dieser Reservierungen später nur noch als Breaking Change nachholbar wäre; M1 vor M2, weil Streams auf dem Tick-, Fault- und Cursor-Modell aufsetzen; M3 vor M4, weil die Darstellungsverengung und das Kostenmodell den Codegen steuern und ohne Intervallanalyse jede Stelle einen Laufzeit-Check bekäme — der Codegen würde später umgebaut; M4 vor M5, weil die Runtime-Kern-Schnittstelle (Prozessabbild, HAL-Traits) auf Linux mit Simulationstreibern billiger stabilisiert wird als auf Hardware; M6 als Vertikalen, weil diese Features voneinander unabhängig sind und parallel laufen können.

---

## 5. Parallelisierung

**Zuordnung der Kommandos (2026-09-10).** Die Inventur führte alle fünfzehn Werkzeuge unter „M3/M4",
was die Zuordnung offenließ. Sie folgt jetzt der Schicht, die das Kommando braucht: `check`, `size` und
`fmt` nach M3 (nur Sema und Analyse); `sim`, `run`, `replay`, `graph` nach M4 (Interpreter beziehungsweise
Codegen und Runtime); `test`, `campaign`, `bench`, `driver-test`, `tune` nach M5/M6 (Hardware,
Kalibrierung, Szenarien); `prove`, `migrate`, `import-c` nach M6/M7. `check` und `fmt` sind damit fertig,
`sim` läuft, bleibt aber offen, bis Szenarien und `--golden` dazukommen (13.6).

**Korrektur (2026-09-10).** `reader`/`writer` standen in M2, gehören aber nach v1.1: Beide sind nach
11.4 Blöcke über `bytes<N>` und brauchen dafür Konstantenvariablen in Generics (`block reader[const N]`,
3.12) — die Referenz führt sie in Abschnitt 15 selbst unter v1.1, zusammen mit „Standardbibliothek
vollständig (11.4)". Sie waren in M2 also nicht baubar. `plan/features.csv` hatte es bereits richtig
(LIB-reader, LIB-writer unter M3/M4); die Zeile hier war der Fehler. Praktische Folge für den
Protokollteil: Bis dahin sind `decode`/`encode` mit `layout` (3.7) der Weg, Cursor-Bausteine kommen
mit v1.1 nach (plan/feedback.csv, FB-04).

Nach M0 (MIR eingefroren) laufen fünf Stränge mit geringer Kopplung: (1) Sema und Analysen, (2) Interpreter, (3) `takt-rt-core` mit HAL und Simulationstreibern, (4) `libtaktm`/Natives mit Konformitätsvektoren, (5) LLVM-Backend gegen MIR-Fixtures. Die Stränge treffen sich in M4. Mit drei bis vier Ingenieuren ist das der kritische Pfad; mit weniger wird M0–M2 sequenziell. Die Standardbibliothek in Takt kann ab M1 von jemandem geschrieben werden, der die Sprache nur benutzt — sie ist zugleich der beste Test der Ergonomie.

### 5.1 Wann kommt v1.1? — und was daraus folgt

Die Frage stellt sich beim Planen von M3 mit Nachdruck, weil dort sichtbar
wird, wie viel an v1.1 hängt: **19 der 31 offenen Prüfungen** können nicht
scharf werden, weil ihr Konstrukt eine Stufe meldet, und vier weitere
brauchen die Hardware-Konfiguration aus 8.10 — ebenfalls v1.1. Auch der
Praxisbericht (`plan/feedback.csv`) landet mehrfach dort: `reader`/`writer`
(FB-04), Segment-Timeouts (FB-13), Handler-Guards (FB-14), der
`follows`-Lint (FB-18).

**Der Fahrplan sagt: nach v1.** Abschnitt 8 veranschlagt M0–M5 mit 9–12
Monaten, M6 (die v1.1-Vertikalen) mit weiteren 4–6. v1.1 liegt damit etwa
**15–18 Monate** nach Beginn, und M6 ist kein einzelner Block, sondern ein
Bündel unabhängiger Vertikalen, die parallel laufen.

Das ist keine Verlegenheitsentscheidung, sondern folgt aus der
Abhängigkeitsfolge: Jede M6-Vertikale ist Ende-zu-Ende definiert (Sema →
Interpreter → Codegen → Runtime → Konformität) und braucht damit einen
Codegen (M4) und eine Runtime (M4/M5). Ein `persist var` ohne Journal, ein
`idle` ohne Systemschlaf, ein `follows` ohne Schedulability wären halbe
Features — genau das, was Abschnitt 1 an Vorgehen B kritisiert.

**Drei Kandidaten könnten trotzdem früher kommen**, weil sie keinen Codegen
brauchen und in M3 spürbar entlasten:

| Kandidat | Warum früher möglich | Was es löst |
|---|---|---|
| Konstantenvariablen `[const N]` (3.12) | reine Sema-Arbeit, Monomorphisierung im Lowering | `reader`/`writer` und die halbe Standardbibliothek (FB-04); Prüfung 52 wird prüfbar |
| Einheiten auf Integern (3.2) | Typregeln, keine neue Laufzeit | Prüfung 38; der Registerteil beider Praxistreiber |
| Hardware-Konfiguration (8.10), nur Lesen | eine Datei parsen und validieren | Prüfungen 28, 29, 32, 39 werden vom Bericht zum Urteil |

Alle drei sind in M3 oder unmittelbar danach machbar, ohne die Reihenfolge
zu brechen — sie sind Sema und Analyse, nicht Codegen. **Empfehlung: nach
M3 bewerten, nicht jetzt entscheiden.** M3 zeigt an der Kennzahl und an der
Zahl der `definiert`-Prüfungen, wie teuer das Warten wirklich ist; vorher
wäre es geraten. Was M3 dafür tut, ist billig und steht in `plan/m3.md`:
Die Prüfungen bekommen ihre Ablehnungstests jetzt, sodass ein Vorziehen
später nur noch den Testfall austauscht statt ihn zu erfinden.

Unverändert bleibt: Nichts an v1.1 ist ein Breaking Change. Das
Reservierungspaket aus M0 (Grammatik, MIR-Platzhalter, reservierte Namen,
Editionen) hält alle diese Konstrukte offen, und jedes v1-Programm bleibt
gültig.

---

## 6. Nichts vergessen: die Mechanik

1. **Inventur aus der Referenz.** Die CSV wird maschinell erzeugt (`python plan/extract_features.py`; bestehende Status-Werte bleiben erhalten, entfallene IDs werden gemeldet); die Extraktion arbeitet auf mehreren Ebenen (Grammatikproduktionen *und* ihre Alternativen, Schlüssel- und reservierte Wörter, reservierte Membernamen, Semantikabschnitte *und* ihre Absatzregeln, `exec`-Regeln und Semantikfunktionen, Desugaring-Regeln, Prüfungstabelle, T1–T16, Sätze, Fault-Arten, Bibliothek, Formate, Profile, Regeln der verteilten Ausführung, Kommandos, Beispiele, Leit- und Einzelentscheidungen) und wird bei jeder Referenzänderung erneut ausgeführt; neue IDs erscheinen als offen. Eine Gegenprobe pro Referenzänderung prüft, dass jeder Punkt des Änderungspakets auf mindestens eine ID abgebildet ist — für v0.2.8 sind es 24 von 24.
2. **Ein Test je ID.** Grammatik-IDs: Parse- und Formatter-Roundtrip (auch für reservierte Konstrukte: parsen, dann „ab Stufe X"). SEM-/PAR-IDs: Interpreter-Tests aus den ASCII-Regeln. RULE-/FN-IDs: ein Test je `exec`-Regel und Semantikfunktion. DESUGAR-IDs: Zustände zurücklesen. SC-IDs: positiv und negativ. OBL-/THM-IDs: Eigenschaftstests (Ordnungsunabhängigkeit, Terminierung, Verengung, Schlaf-Äquivalenz). KW-/MEM-IDs: Ablehnungstests für reservierte Bezeichner. LIB-IDs: Golden-Traces. FMT-IDs: Leser älterer Versionen. PROF-IDs: Konformitätsbericht. EX-IDs: Beispiel als Golden-Test auf jeder Zielklasse. DEC-/PRINC-IDs: keine Tests, aber Referenz für ADRs.
3. **Dokumentation als Test.** Jeder Codeblock der Referenz wird in CI geparst; Beispiele werden ausgeführt; die Grammatikdatei `grammar/takt.ebnf` ist die Quelle des Grammatikabschnitts 2.3 (`check_grammar.py --sync` schreibt ihn). Die Lexer-Spezifikation `grammar/lexer.md` definiert die Tokens mit Testvektoren, die der Tokenizer in `takt-conformance` erfüllen muss. `grammar/parse_corpus.py` führt die Grammatik als Interpreter aus und ist das Orakel für den Parser: Der Korpus in `corpus-try/` muss dort und im Parser dasselbe Ergebnis liefern; `grammar/diff_parse.py --mutate` prüft das zusätzlich auf Varianten jeder Zeile (Wort gelöscht oder verdoppelt), und `corpus-try/ast/*.sexpr` halten den Baum jeder Korpusdatei als Golden-Dateien fest. Der Formatter (`grammar/format.md`, Entwurf in `plan/formatter.md`) wird über seine Vektoren, den Roundtrip auf Korpus und Schnipseln und den kanonischen Korpus (`format(s) == s`) geprüft; `takt fmt --check` (Crate `takt-cli`) meldet nicht kanonische Dateien. `grammar/fuzz_grammar.py` (Entwurf `plan/fuzzer.md`) erzeugt wohlgeformte Programme aus der EBNF und prüft Parser, Orakel und Formatter an fünf Fassungen je Programm (kanonisch, minimaler und maximaler Leerraum, geklammert, verrauscht) samt Abdeckung der Grammatikalternativen. Die Prüfungen aus Abschnitt 10 haben je ein Verzeichnis `corpus-try/checks/SC-n/` mit `ok_`- und `bad_`-Dateien; Zeilenanmerkungen `#~ SC-n` nennen die erwarteten Diagnosen (`crates/takt-sema/tests/checks.rs`, Entwurf `plan/sema-m0.md`). Die MIR (`crates/takt-mir`, Entwurf `plan/mir.md`) ist gegen die Inventur abgebildet: `plan/mir_map.csv` nennt je SEM-, G-, PAR-, RULE-, FN-, DESUGAR- und MEM-ID ihren Knoten, ihre Regel, Prüfung oder Runtime-Komponente, und `plan/check_mir_map.py` prüft, dass jede ID vorkommt und jeder Knoten im Code existiert; das Dateiformat `grammar/mir-format.md` hat Testvektoren wie `lexer.md`. Der Entwurf für M1 (Lowering in `takt-sema`, Referenzinterpreter `takt-interp`, Stimulus- und Trace-Format) steht in `plan/m1.md`; sein Abschnitt 9 hält den Stand je Schritt und die beim Umsetzen gefundenen Fehler fest. Das Lowering prüft sich am Korpus: `crates/takt-sema/tests/corpus.rs` verlangt, dass die Beispiele 14.1 bis 14.5 fehlerfrei in die MIR übersetzen und keine Korpusdatei den Elaborator zum Absturz bringt. Die ausführbare Semantik prüft `crates/takt-sema/tests/semantics.rs` gegen die Regeln aus 9.2 bis 9.4; das zeilenorientierte Trace-Format steht mit Vektoren in `grammar/trace.md`. Die Golden-Simulationen liegen in `corpus-try/sim/` — je Beispiel ein Programm mit Plant-Modell (8.3), je Szenario ein Stimulus und ein aufgezeichneter Trace, zusammengehalten von `manifest.csv`; `crates/takt-sema/tests/golden_sim.rs` vergleicht sie und wiederholt jeden Lauf mit permutierter Schrittreihenfolge (Satz 9.4.1), `takt sim --golden` erzeugt denselben Vergleich auf der Kommandozeile. Die Sätze aus Abschnitt 9 sind Tests: `crates/takt-sema/tests/theorems.rs` prüft Lemma 9.3.1 an einem Fault-Wald der Tiefe 3 und Satz 9.4.2 an Zufallsstimuli, `grammar/fuzz_grammar.py --sim` lässt zusätzlich erzeugte Programme durch `takt check` und `takt sim` laufen. Jede Prüfung aus Abschnitt 10 bekommt ihr Verzeichnis, sobald sie auslösbar ist; `plan/m1.md` Abschnitt 9.0 hält fest, welche das mit M1 sind und warum die Prüfungen 6 und 12 dort noch keinen Fall haben. Der Entwurf für M2 (Ströme, Muster, Handler, geplante Ausgaben) steht in `plan/m2.md`; seine Abschnitte 1 und 9 halten die Abwägungen und die Punkte fest, an denen die Referenz nachzuziehen ist.
4. **Status und Dashboard.** Die Inventur führt je ID genau eine Status-Spalte mit fünf Werten: `offen`, `definiert`, `teilweise`, `fertig`, `rationale`. `definiert` heißt: die Syntax ist in `grammar/takt.ebnf` erfasst und `grammar/check_grammar.py` bestätigt Vollständigkeit gegen 2.2 und die Inventur; es gibt noch keinen Test. Was `fertig` heißt, legt die Kategorie über die Abnahme aus Punkt 2 fest — eine Grammatik-ID ist fertig, wenn Parse- und Formatter-Roundtrip grün sind; eine SEM-, PAR-, RULE- oder FN-ID, wenn ihr Interpreter-Test grün ist und ab M4 zusätzlich der differentielle Test gegen den nativen Code; eine SC-ID mit positivem *und* negativem Test; eine EX-ID, wenn das Beispiel auf jeder bis dahin unterstützten Zielklasse denselben Trace liefert. `teilweise` ist der ehrliche Zwischenstand, wenn eine Schicht steht und eine andere fehlt; welche, sagt der Commit, nicht die CSV. `rationale` tragen dauerhaft die Einträge, die nichts zu implementieren haben (CH, PRINC, DEC, STUFE) — sie sind Referenz für ADRs und werden nie abgehakt. Meilenstein-Exit = alle IDs des Meilensteins auf `fertig`. Abgehakt wird ausschließlich, was durch einen laufenden Test belegt ist.
5. **Änderungsdisziplin.** Jede Abweichung, die bei der Implementierung auffällt, wird als Entscheidung dokumentiert: Referenz ändern (mit Inventur-Update) oder Implementierung korrigieren. Die Referenz bleibt die Quelle der Wahrheit; es entsteht nie ein Zustand, in dem der Code etwas definiert, was die Referenz nicht sagt.

---

## 7. Risiken und Gegenmaßnahmen

| Risiko | Gegenmaßnahme |
|---|---|
| MIR-Umbau nach M2/M6 | M0-Review: jede Inventur-ID auf einen MIR-Knoten abbilden, bevor Code entsteht; Jobs, Streams, `follows`, `port` als Knoten vorsehen |
| Bit-Identität scheitert an FP-Details | ab M4 differentielles Testen über zwei Architekturen; `libtaktm`-Vektoren; IEEE-Modus-Prüfung in der Konformität |
| Warnrauschen der Intervallanalyse | Kennzahl ab M3 messen; Idiom range-typisierte Zwischengröße in der Bibliothek vorleben; Oktagone in M6 |
| Runtime-Jitter auf Linux | ab M4 messen; `Overrun` als Fault früh im Betrieb sichtbar |
| Umfangswachstum | Stufen-Gates; nichts außerhalb der Inventur ohne Referenzänderung |
| Semantische Lücken, die erst die Implementierung zeigt | Interpreter zuerst; jede Lücke wird ADR + Test, nicht stiller Code |

---

## 8. Grobe Größenordnung

Bei drei bis vier erfahrenen Ingenieuren: M0 4–6 Wochen (davon die Hälfte MIR-Entwurf und Inventur-Kuratierung), M1 8–10 Wochen, M2 6–8 Wochen, M3 6–8 Wochen, M4 8–10 Wochen, M5 6–8 Wochen — v1 auf vier Zielklassen nach etwa 9–12 Monaten. M6 als parallele Vertikalen 4–6 Monate, M7 3–4 Monate, M8 danach. Die größten Unsicherheiten sind die Intervallanalyse (Präzision vs. Rauschen) und die Treiberarbeit auf Hardware; beides ist im Plan früh sichtbar.

---

## 9. Finaler Vorschlag: die ersten zehn Schritte

1. Inventur kuratieren (anfangs 1 055 IDs → gruppiert, Meilensteine bestätigt) und das Dashboard aufsetzen. ✔
2. Grammatik als Datei aus 2.3 ableiten — mit allen reservierten Produktionen späterer Stufen; Tokenizer und Parser für die volle Grammatik; Formatter; Edition, reservierte Wörter und Membernamen, offene Enums; alle Referenz-Schnipsel als Parse-Tests.
3. MIR-Entwurf als Dokument gegen die Inventur — mit Platzhalterknoten für gescopte Instanzen, `resume`, Trigger, Captures, Eigenschaften und Knotenplatzierung —, Review, dann Rust-Typen und versionierte Serialisierung — Freeze.
4. Interpreter-Skelett mit den 9.x-Funktionen unter ihren ASCII-Namen; erste Tests für `exec`, `step_m`, `resolve_m`, `tick`.
5. Sema für Einheiten, Ranges, Qualität, `T?`; Maschinen-Wohlgeformtheit und Fault-Wald (Prüfungen 8, 9).
6. Sequenz-Desugaring (6.2 vollständig, inkl. `->` in Segmenten) mit Tests, die die Zustände zurücklesen.
7. Skalarkanäle, `sim`-Bindung, Modelle als Maschinen — 14.1 bis 14.5 als Golden-Traces.
8. Streams, Byte-Ringe, Muster → DFA, Handler — 14.6 gegen ein UART-Modell.
9. Intervallanalyse und Kostenmodell; `takt size`; Darstellungsverengung als Annotation.
10. LLVM-Backend gegen MIR-Fixtures parallel ab Schritt 4; erste differentielle Abnahme mit 14.1.

Damit steht nach M4 eine Sprache, die auf Linux vollständig läuft, deren Semantik zweifach implementiert und gegeneinander geprüft ist, und deren Weg auf Hardware nur noch Treiber und Kalibrierung braucht — nicht neue Sprachentscheidungen.