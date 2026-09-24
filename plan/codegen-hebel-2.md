# Codegen-Hebel, zweite Runde: was noch geht, und wo Takt sich absetzt

Stand 2026-09-24, nach `plan/codegen-hebel.md`. Ausgangslage am UART-Stapel (`riscv32imac`, Zielflags): Takt 9 652 Byte, C 2 782, Rust mit Indexprüfungen 4 851, mit Überlaufprüfungen 6 091. Die 9 652 Byte nach Posten:

| Posten | Bytes | Anteil |
|---|---|---|
| Schrittfunktionen: Baum, Handler-Fenster, Übergänge, 26 Fault-Trampoline | 4 418 | 46 % |
| `loop:`-Rümpfe | 2 258 | 23 % |
| Eintritt und Tick 0 | 1 110 | 12 % |
| Gerüst je Maschine | 662 | 7 % |
| reine Funktionen | 346 | 4 % |
| geteilt | 326 | 3 % |

Dazu in der IR: 72 Fault-Sprünge, 232 Vergleiche, 42 Stromaufrufe, 24 Meldestellen. Die Logik ist ein Viertel; die Hälfte ist Semantik, die C nicht hat.

## 1. Die Hebel, gewogen

Gewinn geschätzt am UART-Stapel; Aufwand in Arbeitstagen einer Person, die den Codegen kennt.

### Struktur (Codegen)

| | Hebel | Gewinn | Aufwand | Risiko | Kern |
|---|---|---|---|---|---|
| S1 | **Fault-Trampolin je Ziel statt je Blatt** | −0,9 bis −1,2 KB | 1–2 | mittel | Blätter mit gleichem Fault-Ziel, gleicher `exit`-Menge und gleicher `enter`-Kette teilen einen Trampolin; das Blatt geht als Wert in `takt_fault(m, blatt)`. `uart_link`: sechs Trampoline werden einer. Differenzsuite und Fuzzer decken Faults dicht ab. |
| S2 | **Übergang als Daten** (QP/`QMsm`-Weg) | −0,5 bis −0,8 KB | 2–3 | mittel | `enter_leaf` steht heute an jeder Übergangsstelle: Timer und Zähler zurücksetzen, `saved`, `conf`, Entry-Tick. Eine Funktion je Maschine `<m>_switch(st, …, i8 von, i8 nach)` mit Tabellen (Timer je Zustand, Zähler je Zustand) und einem `switch` auf die Entry-Tick-Funktion. Mit S1 zusammen: die ganze Wechselmechanik einmal je Maschine. |
| S3 | `examined` einmal je Schritt | −0,2 KB, Laufzeit | 0,5 | klein | `takt_stream_examined` wird je Element gerufen; das Maximum steht ohnehin im Zustand (`examined`). Der Rahmen liest es nach dem Schritt einmal je Strom (er kennt den Versatz, wie bei `pc`). |
| S4 | `minsize` auf kalte Funktionen | −0,2 bis −0,4 KB | 0,2 | keins | `_init`, `_enter`, `_entryN`, `_init_vars`, `_deadline`, `_persist_*`, `_exit_all` bekommen das LLVM-Attribut `minsize`; der heiße Schritt bleibt `-Os`. Das ist `-Oz` nur dort, wo es keine Laufzeit kostet. |
| S5 | Handler-Fenster als Funktion je Strom | −0,3 KB | 1 | mittel | Die Schleife `count`, Element holen, `examined`, Kette steht je Zustand und Strom. Eine Funktion je (Maschine, Strom) mit dem Zustand als Argument, wie B1 für `loop:`. Lohnt bei vielen Zuständen mit Handlern auf demselben Strom. |
| S6 | Rahmen: `takt_mcu_init_with` und Sende-Commit als Tabellen | −0,3 KB | 0,5 | klein | 686 Byte für Initialwerte und Wiederherstellung je Slot; eine Tabelle `{ Versatz, Größe, Wert }` wie bei `takt_mcu_dump` (FB-231). |

### Darstellung

| | Hebel | Gewinn | Aufwand | Risiko | Kern |
|---|---|---|---|---|---|
| R1 | **Bereichsganzzahlen schmal gespeichert** | RAM −4 Byte je `int`-Variable, Code −5 %, Laufzeit | 1–2 | klein | Die Intervallanalyse (M3) verengt Ausdrücke, die Variablen im Zustand sind trotzdem `i64` (`int in 0..1_000_000` liegt als acht Byte). Speichern in der kleinsten Breite, die den Bereich fasst; rechnen wie heute in `i64`, laden mit `sext`. Exakt, weil die Bereichsprüfung bei der Zuweisung den Wert schon eingrenzt. Auf RV32 spart jeder Zugriff ein Register und eine Anweisung. |
| R2 | 32-Bit-Rechnung, wo die Analyse es beweist | Laufzeit, Code −3 % | 1 | klein | Folgt aus R1 und der Intervallanalyse: `i32`-Addition statt `i64`, wenn Operanden und Ergebnis nachweislich passen. Bitgleich per Konstruktion. |
| R3 | `bytes`-Bindungen als Zeiger in einen Bip-Puffer | Laufzeit (keine 1-KB-Kopie je Element), Code −0,3 KB | 3–4 | mittel-hoch | D3 aus der ersten Runde: Bindungen sind unveränderlich, Elemente bis zum Commit stabil. Braucht einen Ring, in dem kein Element über die Naht liegt, und Zeigerfelder in Bindungsrecords. Erst, wenn die Laufzeit der Elemente drückt; heute sind es 24 µs je Tick für fünf Maschinen. |

### Prüfungen und Beweise (der SPARK-Weg)

Jede Prüfung ist heute `icmp` und `br` zum Trampolin, mehr nicht. Was bleibt, ist ihre Zahl und der Fault-Pfad dahinter. Der einzige legitime Weg, sie loszuwerden, ist der Beweis — kein Schalter (4.1, 9.4.4).

| | Stufe | Gewinn | Aufwand | Kern |
|---|---|---|---|---|
| P1 | **Dominanz im Sema, erweitert** (heute: `Checked` fällt, wenn die Bedingung dominiert) | −0,2 KB | 1 | Schleifenmuster `for i in range(N): if i >= s.len: break; s[i]` — der Abbruch dominiert den Zugriff, die Sema sieht es noch nicht. Ebenso `x as u8` nach `x <= 255`. |
| P2 | **Intervallanalyse als Prüfungsentferner** | −0,3 bis −0,8 KB, Laufzeit | 2–3 | Die Analyse aus M3 rechnet Bereiche für Ausdrücke; sie soll auch Prüfungen streichen, deren Bereich sie deckt (Index unter Länge, Zuweisung in Bereich, Division ohne Null). Das ist Frama-C/EVA in klein, auf einer Sprache ohne Zeiger und ohne Rekursion — dort trägt sie weit. |
| P3 | **Beweispflichten an `takt prove`** | bis −2 KB (alle Prüfungen), Laufzeit; vor allem Nachweis | 5–10 | `takt-prove` kodiert die Schrittfunktion schon als Transitionssystem mit k-Induktion. Jede verbliebene Prüfung wird eine Beweispflicht („diese Stelle faultet nie"); bewiesene stehen in einer Datei neben dem Programm (`.takt-proof`, mit Hash von Programm und Werkzeugen, 11.3), und der Codegen lässt sie weg. Das ist SPARKs „Absence of Run-Time Errors, dann `-gnatp`" — mit dem Unterschied, dass der Interpreter die Spezifikation bleibt und der Differenztest jede Streichung gegenprüft. Der Fault-Pfad bleibt, weil Treiber und Ränder zur Laufzeit faulten (12.6). |

Reihenfolge P1, P2, P3: P1 und P2 sind Werkzeuge, die man ohnehin für Fehlermeldungen will („diese Prüfung kann nie scheitern" ist auch eine Warnung); P3 ist der Zertifizierungsweg.

### Was nicht

- Prüfungen abschalten (Zig `ReleaseSmall`): nein.
- `-Oz` global: gemessen, kostet ein Fünftel Schrittzeit.
- Dauern als Tickzähler: nicht exakt (erste Runde B7).
- Programm und Rahmen als LTO-Modul: gemessen größer.

## 2. Zielbild

| Schritt | Objekt | Faktor zu C (2 782) |
|---|---|---|
| heute | 9 652 | 3,5 |
| S4, S3 (billig) | ≈ 9 100 | 3,3 |
| S1, S2 (Wechselmechanik einmal) | ≈ 7 400 | 2,7 |
| R1, R2 (schmale Ganzzahlen) | ≈ 6 800 | 2,4 |
| P1, P2 | ≈ 6 200 | 2,2 |
| P3 (alle Prüfungen bewiesen) | ≈ 5 000 | 1,8 |

Was dann bleibt, ist Semantik: Fault-Wald mit `exit`/`enter`, Entry-Tick, Fenster mit Cursor, Qualität der Eingänge. Das ist der Preis dafür, dass diese Dinge in der Sprache stehen und nicht im Kopf des Programmierers — dieselbe Größenordnung wie Ada mit Prüfungen plus handgeschriebener Zustands- und Fehlerlogik.

## 3. Abgrenzung zu Ada/SPARK und SCADE

Nicht über die Größe: Nach P3 steht Takt dort, wo SPARK nach dem Beweis steht, und SCADE-Code war nie größer als C. Die Unterschiede liegen in dem, was die Sprache verspricht und was das Werkzeug beweist.

**Gegen Ada/SPARK.**

- Ada beweist die Abwesenheit von Laufzeitfehlern eines Programms. Takt beweist zusätzlich die Gleichheit des Verhaltens über Interpreter, Linux-Rahmen und Board (Satz 9.4.4, Differenztest, Board-Trace). Das ist eine Aussage über den *Compiler*, nicht nur über das Programm — SPARK hat sie nicht, weil GNAT nicht die Spezifikation ist.
- Der Tick, der Fault-Wald, die Ströme mit Cursor, Qualität und Alter der Eingänge sind Sprache, keine Bibliothek. In Ada schreibt man Tickschleife, Zustandsmaschine, Fehlerbehandlung und Ringe selbst — und beweist dann sein eigenes Gerüst mit. Takts Gerüst ist einmal bewiesen (Lemma 9.6.1, Satz 9.9.1) und für jedes Programm gleich.
- Schedulability ist statisch (7.2, 9.4.3, `takt size`): Ada braucht dafür externe WCET-Werkzeuge und ein RTOS-Modell (Ravenscar). Takt hat keinen Scheduler, der beweisbar wäre — nur eine Schleife.
- Wo Ada gewinnt: Allgemeinheit, Reife, Zertifizierungsbausteine (DO-178C), Tasking. Takt sollte das nicht nachbauen. Die Botschaft: *Die Steuerschleife ist die Sprache.*

**Gegen SCADE/Lustre.**

- SCADE ist der nächste Verwandte: synchron, eine Schrittfunktion je Knoten, statischer Speicher, qualifizierter Generator. Was SCADE nicht hat und Takt als Semantik führt: den Fault-Wald mit Zielzuständen, Byte-Ströme mit Fenstern und Mustern (Protokolle), Qualität und Alter der Eingänge, Entry-Tick-Regeln, `after`-Fristen als Sprachkonstrukt. In SCADE modelliert man Fehler als Datenfluss von Hand.
- Takt hat Test und Nachweis in der Sprache: `scenario`, `expect`, `campaign`, `property` mit Monitor, `verify`, `req` als Anforderungsbezug. SCADE trägt Tests in getrennten Werkzeugen (SCADE Test) und die Verifikation im Design Verifier — Takt bringt beides mit `takt sim`, `takt prove` und dem Korpus.
- Takt geht bis auf die Hardware: `port @ mmio`, `driver machine`, Bring-up-Crates, Board-Trace. SCADE endet beim erzeugten C; der Treiber ist Handarbeit außerhalb der Semantik.
- Offen, klein, kalibrierbar: `c_target` und `takt bench` (13.8) sollen die Laufzeitgarantie messbar machen statt sie zu kaufen.
- Wo SCADE gewinnt: Qualifikation des Generators (DO-178C Level A), Datenfluss-Notation, dreißig Jahre Werkzeug. Takts Gegenstück ist der Differenztest gegen den Interpreter als Spezifikation — billiger, jederzeit wiederholbar, und für jeden Codegen-Hebel dieser beiden Runden der Prüfstein gewesen.

**Der eine Satz.** Ada beweist Programme, SCADE erzeugt Schrittfunktionen; Takt macht Tick, Fault, Strom und Test zur Sprache und beweist, dass der Chip tut, was der Interpreter tat.

## 4. Empfohlene Reihenfolge

1. S4 und S3 — ein halber Tag, ohne Risiko.
2. S1, dann S2 — die Wechselmechanik einmal je Maschine; größter Rest.
3. R1 — schmale Speicherung; RAM und Code; danach R2.
4. P1, P2 — Prüfungen streichen, die die Analyse deckt; die Warnung „nie scheitern" gleich mit.
5. P3 — Beweispflichten an `takt prove`, `.takt-proof` als Bauzutat.
6. S5, S6, R3 — nach Bedarf, mit Messung.

Jeder Schritt gegen Differenzsuite, Fuzzer, MCU-Rahmen, Größen-Baseline und Board.

## 5. Stand 2026-09-24

Gemessen am UART-Objekt (`test_uart_c6_hw.takt`, `riscv32imac`, `-Os`, Flags aus `toolchain::object_flags`), verifiziert gegen Differenzsuite, Fuzzer, MCU-Rahmen, Größen-Baseline und Board.

| Schritt | Objekt | Befund |
|---|---|---|
| Ausgang (erste Runde) | 9 652 | |
| S4 `minsize`, S3 `examined` einmal je Schritt | 9 432 | −220 Byte; FB-240 |
| S1 Fault-Trampoline je Gruppe | 9 358 | 17 Blätter in 9 Gruppen; −74 Byte; FB-241. Der Schlüssel ist der erzeugte Code (Ziel, `exit:`-/`saved`-Zustände, `enter:`-Kette), nicht die Kette der verlassenen Zustände — die ist je Blatt verschieden. |
| S2 Übergang als Daten | — | **Gemessen, verworfen** (FB-245). Die Schrittfunktionen des UART-Programms enthalten 6 Übergangsstellen; eine Stelle sind zwei bis vier komprimierte Stores (`sb`, `sw zero`), ein Tabelleneintrag kostet dasselbe. Gleiche Enden über alle Stellen: 39 IR-Instruktionen, die LLVM ohnehin zusammenlegt. Der Rest des Schritts ist Nutzercode (Handler, Decode, `memmove`). |
| P1, P2 Prüfungen als Knoten, Intervallanalyse als Beweiser | 9 416 | +58 Byte, weil der Codegen vorher **Prüfungen ausließ** (FB-242): Überlauf in schmalen Typen, Division durch null, Schiebebetrag, `as`-Konversion und der Index einer Zuweisungsstelle standen nur im Interpreter; ein variabler Schiebebetrag erzeugte ungültige IR (`shl i16 x, i64 y`). Jetzt trägt die MIR jede Prüfung als Knoten, die Analyse beweist, was sie kann, und der Codegen setzt den Rest um. Funktionsrümpfe wurden vorher nie analysiert (FB-243). |
| R1 Bereichsganzzahlen schmal gespeichert | 9 014 | −402 Byte; FB-248. `int in 0..1_000_000` liegt als `i32`, `Duration in 0 s..10 ms` als `i32`, `int in 0..8` als `i8`; gerechnet wird weiter in `i64`, `sext` beim Laden, `trunc` beim Schreiben. Ψ, Journal und C-Rahmen behalten ihre Breiten. `takt size` rechnet mit derselben Regel. |

### Was P1/P2 konkret gebracht haben

- Analyse (FB-244): De Morgan an `or`/`and` (`if a or b: break` verfeinert beide), `len`/`count` ≤ Kapazität, `>>` und `&` mit Schranken, `tick` als Konstante, Division mit Null-Ausschluss (der Quotient deckt die beiden Seiten neben der Null), Konversion, Schiebebetrag, Überlauf und Divisor als Beweisziele. Ein Überlauf in 64 Bit warnt nicht (Prüfung 4).
- Bewiesene Range-Prüfungen bleiben als `RangeOrigin::Proven` in der MIR: Der Interpreter prüft sie weiter und meldet einen Verstoß als Fehler des Compilers (`Bug`), nicht als Fault; der Codegen lässt sie aus. Damit prüft die Differenzsuite jeden Beweis mit — dieselbe Absicherung, die P3 braucht.
- `takt check --report --checks` nennt jede verbliebene Prüfung mit Ursache und Stelle; das ist die Liste der Beweispflichten für P3.
- Prüfung 9 (Guards aus `FAULTED` ohne implizite Prüfung) läuft nach der Analyse, sonst schlüge sie bei beweisbarer Arithmetik an.
- Kennzahl: `Valid`/`Missing` zählten als `Arith` (FB-247); jetzt zählen nur die vier Ursachen aus 3.4.

UART nach P2: 42 Prüfungen — 29 `Declared` (Zähler `x += 1` in `0..1_000_000`, Tick-Rand-Invarianten ohne beweisbare Schranke), 12 `Index` (Ringindizes gegen die Laufzeitlänge, 10 davon relational), 1 `Arith` (`tx_stalled_for += tick`, i64), 0 `Convert`. Das ist die Arbeitsliste für P3: Zähler brauchen eine Invariante über Ticks (k-Induktion), Ringindizes eine Relation (`i < s.len`, Oktagon oder SMT).

### Zielbild, korrigiert

Die Tabelle in 2 nahm an, dass P1/P2 Prüfungen *entfernen*; tatsächlich fehlten dem Codegen Prüfungen, und die Analyse hat die neuen sofort wegbewiesen, wo es ging. Der ehrliche Stand: 9 416 Byte, Faktor 3,4 zu C — mit vollständigen Prüfungen. Die verbleibenden 56 Fault-Zweige (31 Range, 7 Überlauf-Instanzen, 4 Konversion, 12 Index, Rest Streams) sind P3-Material; jeder kostet 6–12 Byte.

### Offen

- `NonFinite` und `Domain` für Gleitkomma stehen als Knotenart bereit, aber weder Sema noch Codegen erzeugen sie; der Interpreter faultet bei nicht endlichem Ergebnis (4.2). Dieselbe Lücke wie FB-242, für `float`; die Warnpolitik aus 3.4 passt dort nicht (jede Gleitkommaoperation wäre eine Prüfung ohne Beweisweg).
- R1, R2, S5, S6, P3, R3 wie in 4.
