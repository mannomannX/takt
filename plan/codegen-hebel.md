# Codegen-Hebel: Größe, RAM, Laufzeit

Stand 2026-09-23, gemessen am UART-Stapel (`crates/takt-bringup-esp32c6/programs/test_uart_c6_hw.takt`) gegen eine handgeschriebene C-Fassung derselben fünf Schichten (`plan/codegen-hebel/uart_stack.c`, ohne Bereichs- und Indexprüfungen, ohne Meldungen). Beide mit `clang -Os`, rv32imac, `-ffunction-sections`.

## 1. Ausgangslage

| | Takt | C | Faktor |
|---|---|---|---|
| Code des Stapels (Objekt) | 13 782 | 2 830 | 4,9 |
| Code gelinkt, mit Relaxation | 13 258 | – | |
| Rahmen (`takt_mcu_*`, Ringe, Ausgabe) | 2 177 | im C enthalten | |
| statisches RAM | 33 104 | 20 829 | 1,6 |
| größter Stack-Rahmen | 3 216 | 1 056 | 3,0 |

Wohin die 11 KB gehen (aus der IR gezählt):

| Posten | Bytes (Schätzung) | Art |
|---|---|---|
| `loop:`-Rumpf je Blatt dreimal (`_step`, `_entryN`, `_enter`) | ≈ 3 500 | Codegen |
| Prüfungen und Fault-Pfade (95 Fault-Sprünge, 25 `takt_fault`) | ≈ 2 000 | Sprache |
| Meldungen (20 `log`, 5 `alert`, formatierte Argumente) | ≈ 1 300 | Sprache, verlagerbar |
| Gerüst je Maschine (`publish`, `deadline`, `advance`, `init_vars`) | ≈ 1 500 | Codegen |
| Stromprotokoll (`count`/`at`/`examined`/`send`, Elementkopien) | ≈ 1 000 | Codegen |
| 64-Bit-Dauern und -Zähler auf RV32, `push` mit Rückgabe | ≈ 1 000 | Darstellung |

## 2. Was andere tun

- **Rust/`defmt`** formatiert nicht auf dem Gerät: Die Vorlagen liegen in einer ELF-Sektion, die nicht ins Flash kommt, das Gerät sendet Index und Rohargumente, der Host setzt den Text zusammen. Genau das passt zu 13.4 (stabile IDs je Stelle) und zum Trace-Vergleich, der nach dem Ausformulieren am Host unverändert bleibt.
- **Zig** schaltet in `ReleaseFast`/`ReleaseSmall` die Laufzeitprüfungen ab. Das ist für Takt kein Weg: 4.1 verspricht Totalität, Satz 9.4.4 die Gleichheit mit dem Interpreter. Prüfungen fallen nur, wenn sie bewiesen sind.
- **Ada/SPARK** geht diesen Weg: erst Beweis der Abwesenheit von Laufzeitfehlern, dann `-gnatp`. Takt hat den Anfang davon (M3 entfernt bewiesene `Checked`-Knoten); der Hebel ist ein stärkerer Beweiser, kein Schalter.
- **RISC-V-Toolchain:** `-msave-restore` ersetzt Prolog und Epilog durch Millicode (60 auf 14 Byte je Funktion), der Machine-Outliner faltet wiederkehrende Befehlsfolgen (im LLVM-Testsatz im Mittel 13 %), `--icf=all` faltet identische Funktionen (wenige Prozent). Zcmp (`push`/`pop`) hat der C6 nicht.
- **Statechart-Generatoren (QP/C `QMsm`)** tauschen Tabellen gegen Code: Übergangsketten liegen als Daten im ROM, der Dispatcher ist einmal da. **Lustre/SCADE** erzeugen je Knoten eine Schrittfunktion über einem Zustands-Struct, ohne Duplikate — das Modell, das Takt ohnehin hat; die Duplikate sind ein Detail unseres Emitters, nicht des Modells.

## 3. Hebel

Gemessen heißt: an diesem Programm ausprobiert. Geschätzt heißt: aus der Zählung in 1 abgeleitet.

### A. Toolchain — sofort, ohne Änderung an Sprache oder Codegen

| Hebel | Gewinn | Kosten | Stand |
|---|---|---|---|
| A1 `-Oz` statt `-Os` | −704 (5 %) gemessen | wenige Prozent Laufzeit, am Board zu messen | offen |
| A2 `-msave-restore` | −672 (5 %) gemessen | ein Aufruf je Prolog | offen |
| A3 Machine-Outliner (`-mllvm -enable-machine-outliner=always`) | −882 (6 %) gemessen; A1+A2+A3 zusammen −2 068 (15 %) | Aufrufe in ausgelagerte Folgen | offen |
| A4 `--icf=all` beim Linken | −120 (1 %) gemessen | keine | offen |
| A5 LTO über Programm und Rahmen | zu messen: Ringzugriffe würden inline | Bauzeit | offen |
| A6 `cold` auf Fault-, Log- und Alert-Pfaden | 0 gemessen (+28) | – | verworfen |
| A7 Rahmen mit `-Oz` | −194 gemessen | wie A1 | offen |

Empfehlung: A1 bis A4 und A7 als Voreinstellung für `--build hw` auf MCU-Zielen, nach einer Laufzeitmessung am Board (`took` in der Zeitzeile).

### B. Codegen — größte Hebel, keine Änderung der Semantik

| Hebel | Gewinn | Entwurf | Prüfstein |
|---|---|---|---|
| B1 `loop:`-Rumpf einmal je Zustand | ≈ −3 000 (25 %) geschätzt | Rumpf als `internal`-Funktion `<m>_loop_<s>(st, in, par, out, i1 entry)`; `->` wird `br i1 %entry`, Fault-Trampolin über `fault_<m>_any` mit Blatt als Wert (gibt es seit FB-222); `_step`, `_entryN`, `_enter` rufen sie. `noinline`, sonst legt LLVM sie zurück. | 5.2 Regel 4, Differenzsuite |
| B2 Fault-Stelle gepackt | ≈ −500 geschätzt | `takt_fault(site << 8 \| kind)` statt zwei Konstanten; der Trampolin merkt `pending` selbst, der Block je Stelle wird ein `li` und ein Sprung | 5.3, Trace unverändert |
| B3 Gerüst je Maschine datengetrieben | ≈ −500..−1 000 geschätzt | `deadline`, `advance`, `init_vars` als Laufzeitfunktionen über Tabellen (Timer je Zustand, Initialwerte), wie `takt_mcu_dump` seit FB-231 | 9.9, `takt size` |
| B4 Große Felder ans Ende des Zustands-Structs | ≈ −300 geschätzt | Skalare vor Overlay und Puffer, damit Versätze unter 2 048 bleiben (RISC-V: sonst `lui`+`addi`; 815 Konstantenladungen im Objekt) | keiner |
| B5 Stromelemente ohne Kopie | ≈ −300 Code, −1 000 Stack je Handler, Laufzeit | Bindung als Zeiger in den Ring; Elemente sind bis zum Commit unveränderlich und Bindungen nicht zuweisbar. Ring als Bip-Puffer: ein Element, das nicht mehr ans Ende passt, beginnt vorn — kein Element liegt je über der Naht | 8.6, 9.6, FB-214 C6 |
| B6 Ringzugriffe in der IR | ≈ −300, Laufzeit | `takt_int_count/at/send` als `internal`-Funktionen im Modul statt im C-Rahmen, damit LLVM sie einbettet; A5 leistet dasselbe über LTO | 8.6 |
| B7 Dauern mit Bereich als Tickzähler | ≈ −300..−500 geschätzt, Laufzeit | `Duration in a..b` im Zustand als i32 Ticks; Vergleich mit einem ns-Parameter über `ceil(p / T0)` einmal beim Laden — exakt, weil `n·T0 < p ⇔ n < ceil(p/T0)` | 3.4, 3.3, Bitgleichheit bleibt, da nur Ganzzahlen |
| B8 `publish` nur geänderte Felder | klein | Kopie je `pub var` bleibt; nicht lohnend | – |

### C. Meldungen als Bauprofil

Heute liegt jede `log`-, `alert`- und `check`-Meldung als Text und Formatiercode im Ziel. Drei Stufen, wählbar beim Bau (`--diagnostics`):

| Stufe | Ziel sendet | Text | Trace |
|---|---|---|---|
| `text` (heute) | fertige Zeile | im Flash | identisch |
| `ids` (Vorschlag als Default für `hw`) | Stellen-ID und Rohargumente (`i64`, `f64`-Bits, Enum-Diskriminante) | in einer Tabelle neben dem Objekt (`.takt-sites`, wie `.defmt`), Host formuliert aus | nach dem Ausformulieren identisch — der Vergleich bleibt |
| `none` (Produktion, 12.3 „reduziert") | nichts | – | ohne Meldungen; Faults und `pending` unverändert |

Was gleich bleibt: Faults entstehen, wirken und stehen im Trace, `check` prüft, `last_fault` trägt Art und Stelle. Was `ids` braucht: die Stellen-ID aus 13.4 (gibt es), eine Tabelle Stelle → Vorlage und Argumenttypen, den Ausformulierer im Trace-Leser (`takt-trace-serial`, `compare`). `last_fault.message` als Laufzeittext bleibt nur, wenn das Programm ihn liest; die Sema sieht das. Gewinn: ≈ −1 300 Code und alle Zeichenketten, dazu die Laufzeit des Formatierens weg vom Tick.

Nicht vorgesehen: ein Schalter für Prüfungen. Zig hat ihn, SPARK nicht — und Takt verspricht mit 4.1 und 9.4.4, was SPARK verspricht.

### D. RAM

| Hebel | Gewinn | Entwurf |
|---|---|---|
| D1 Telemetrie-Ring 8 auf 2 KiB | −6 144 | Konfiguration im Board-Crate; seit dem Trace nur bei Änderung reichen 2 KiB für 40 ms Aussetzer |
| D2 Deskriptor 16 auf 8 Byte | −2 816 | `tick` als u32 (Ticks seit Start, 24 Tage bei 500 µs), `off`/`len` als u16 bei `capacity_bytes ≤ 65 535`; sonst 12 Byte |
| D3 Bip-Puffer statt Deskriptorring | −5 632 | Länge vor den Bytes wie in C; `.t` und `.seq` je Element bleiben nötig (8.6), also 4 Byte Kopf statt 16 — schließt B5 ein |
| D4 Schattenlatch nur für `--diagnostics` ≠ `none` | −112 | trivial |

Zustandspuffer (`acc`, `enc`, `out`, `pending`, `scratch`) sind Programmentscheidungen, kein Hebel des Codegens.

### E. Laufzeit

Messgröße ist `took` in der Zeitzeile (heute 21 µs Mittel, 64 µs Maximum bei 500 µs Tick). Hebel in Reihenfolge des Gewinns: B5 (keine 1-KB-Kopie je Element), C `ids` (kein Formatieren im Tick), B6/A5 (Ringzugriffe inline), B7 (32-Bit-Dauern), B1 (kleinerer Code hält den Cache). A1 bis A3 kosten Laufzeit im niedrigen Prozentbereich; vor der Voreinstellung messen.

## 4. Zielbild

Vom Objekt mit 13 782 Byte, alles geschätzt, gerundet:

| Schritt | danach |
|---|---|
| A Toolchain | 11 700 |
| B1 Rumpf einmal | 8 700 |
| C `ids` | 7 400 |
| B2 Fault-Stellen | 6 900 |
| B4, B5, B6, B7 | 5 900 |
| B3 Gerüst | 5 400 |

Rund 1,9× der nackten C-Fassung, mit allen Prüfungen, Faults und Meldungen. Darunter geht es nur mit Beweisen (SPARK-Weg, M3 erweitert), nicht mit Schaltern.

## 5. Reihenfolge

1. A1–A4, A7 als Voreinstellung, Laufzeit am Board messen (ein Nachmittag).
2. B1 — der größte Einzelhebel, Differenzsuite und Board als Prüfstein.
3. C `ids` mit Tabelle, Ausformulierer und `--diagnostics`; Referenz 12.3/13.4 um das Profil ergänzen.
4. B2, B4 (klein, sicher).
5. B5 mit D3 als ein Schritt (Bip-Puffer, Bindung als Zeiger).
6. B6 oder A5, B7, B3.
7. Größen-Baseline je Korpusprogramm als Test, damit nichts davon still zurückkommt (noch nicht im Register).

## 6. Quellen

- defmt: Interned strings, `.defmt`-Sektion nicht im Flash — https://defmt.ferrous-systems.com/ser-istr , https://ferrous-systems.com/blog/defmt/
- Zig-Baumodi: Prüfungen aus in `ReleaseFast`/`ReleaseSmall` — https://zig.guide/language-basics/runtime-safety/
- SPARK: Beweis der Abwesenheit von Laufzeitfehlern, dann Prüfungen aus — https://docs.adacore.com/spark2014-docs/html/ug/en/usage_scenarios.html
- RISC-V `-msave-restore`, Zcmp — https://reviews.llvm.org/D62686 , https://docs.riscv.org/reference/isa/v20260120/unpriv/zc.html
- Machine-Outliner — https://www.linaro.org/blog/reducing-code-size-with-llvm-machine-outliner-on-32-bit-arm-targets/
- lld ICF — https://manpages.ubuntu.com/manpages/focal/man1/ld.lld-10.1.html
- QP/C `QMsm`, Tabellen statt Code — https://www.state-machine.com/qpc/srs-qp_sm.html
- Lustre/SCADE, eine Schrittfunktion je Knoten — https://www.di.ens.fr/~pouzet/bib/tase17.pdf
