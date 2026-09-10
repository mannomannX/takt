Ja. Ich schreibe es aus der Perspektive dessen, der die Datei gerade getippt hat, sortiert nach dem, was mich beim Schreiben tatsächlich aufgehalten hat.

## Der wichtigste Befund zuerst

**Die Treiberstufe hat ein Zulassungskriterium, das nirgends steht.** Ein `driver machine` kann ein Gerät nur bedienen, wenn gilt

```
FIFO_Tiefe / Byte_Rate  >  T0 + Tick_Jitter
```

Bei 921600 Bd, 8N1, 16-Byte-FIFO sind das 173 µs gegen einen 100-µs-Tick. Das sind 73 µs Reserve, also praktisch nichts. Bei 115200 Bd wären es 1,39 ms und alles wäre entspannt. Der Compiler kennt `T0`, und die FIFO-Tiefe steht in der Hardware-Konfiguration (8.10) — er könnte die Ungleichung prüfen und mir sagen „dieses Gerät ist nicht pollbar, es gehört in die TCB". Ohne diese Prüfung schreibt jemand genau den Treiber, den ich oben geschrieben habe, und er funktioniert 99,9 % der Ticks. Das ist die schlechteste Sorte von Fehler für eine Sprache, deren Versprechen „if it compiles it cannot crash" lautet.

Praktische Konsequenz: L0 ist bei dieser Baudrate Rust, und Takt fängt bei L1 an. Das ist die richtige Architektur, aber die Spezifikation sollte es sagen statt es den Nutzer entdecken zu lassen.

## Die Sprache hat zwei Persönlichkeiten

Die Steuer- und Testhälfte (Maschinen, Sequenzen, `check`, Einheiten, Fault-Wald, `scenario`/`campaign`) ist fertig und schön. Ich würde §14.1 morgen einem Prüfstandstechniker geben. Die Daten- und Protokollhälfte (`bytes`, Streams, Muster, `decode`, `T?`/`T!E`, `inout`, reader/writer) ist eine Systemsprache in derselben Syntax, und sie ist spürbar jünger. **Jede Lücke, gegen die ich gelaufen bin, liegt in der zweiten Hälfte.** Das ist genau die Hälfte, an der „wir migrieren unsere C-Firmware" hängt.

## Was ohne Reibung getragen hat

- **fn/block/machine.** Ich habe kein einziges Mal überlegt, wo etwas hingehört. Zwei Bits Entscheidung: Zustand ja/nein, I/O ja/nein. Das ist selten.
- **Fault-Wald.** Fehlerbehandlung zu schreiben war angenehm, was ich fast nie sage. Drei Fehlerklassen, drei Ziele, Azyklizität vom Compiler geprüft. Kein `goto cleanup`, keine Fehlercode-Kaskaden.
- **`follows`.** Der große Gewinn. Unit-Delay als Default ist richtig, und der Ausweg ist ein DAG-Check statt einer Kausalitätsanalyse. Hat die Latenz meines RX-Pfads halbiert, ohne dass ich über Reihenfolge nachdenken musste.
- **Einheiten auf Integern.** Die Baudteilerrechnung ist die Stelle, wo ich sonst `// Hz, nicht mit Baud mischen` schreibe. Hier hält der Compiler es.
- **Stream-Überlauf als Fault mit statischem `MAXPT * n_m <= CAP`.** Hat meine erste Kapazitätsschätzung abgelehnt. Das ist Arbeit, die ich sonst in einer Tabelle mache.
- **`scenario` + `campaign`.** Bitfehler-Injektion mit Sweep in vier Zeilen, deterministisch, replaybar. In C plus pytest ist das ein Tag Harness-Arbeit. Das ist vermutlich das stärkste Produktargument der Sprache und steht recht weit hinten.

## Was anstrengend war, nach Schwere

**1. `bytes<N>` kann nicht zurückschreiben.** Jeder Rahmungs-Codec braucht Backpatching: Platzhalter schreiben, später füllen. COBS, Längenpräfix, CRC am Ende, TLV. `push` allein ist ein Stack, kein Puffer. Ich habe `with_byte` erfunden, weil ich nicht weiterkam. Die Grammatik erlaubt es bereits (`lvalue := IDENT { … | "[" expr "]" }`), 3.9 nennt es nur nicht. Wahrscheinlich eine Doku-Lücke, aber sie blockiert die zentrale Aufgabe der Sprache im digitalen Teil.

**2. Muster komponieren nicht mit `decode`.** `on rx_frames matches LinkHeader(kind = ACK)` geht nicht, weil das Element `bytes<1024>` ist und das Muster ein Record will. Die diskriminierende Information steckt hinter einem Funktionsaufruf, und ein Muster ist kein Ausdruck. Folge: Ich musste den gesamten Empfangspfad in einen Vorfahren-Handler ziehen, und die Kindzustände können nicht mehr auf einzelne Nachrichtentypen reagieren. Damit ist die ganze zustandsweise Dispatch-Maschinerie für jedes Protokoll unbrauchbar, dessen Rahmung nicht schon am Treiberrand typisiert ist.

Der Spezifikation nach ist die Antwort „dann dekodiert der Treiber" (8.6, Record-Streams mit `layout`). Aber COBS + CRC + Sequenz im Treiber heißt: TCB, heißt Rust. Das ist eine erhebliche architektonische Zwangswirkung, und sie sollte ausgesprochen werden: **nichttriviale Rahmung ist entweder TCB oder man verliert Handler-Dispatch.**

**3. Kein `peek`.** „Untersucht heißt konsumiert" ist für den Leser die richtige Regel. Aber in jeder Backpressure-Schleife will ich vor der Entscheidung hinsehen. Bei fester Elementgröße geht es (Platz zählen, dann exakt so viele konsumieren). Bei `bytes<N>` kann ich nicht wissen, ob das nächste Element passt, ohne es konsumiert zu haben. Für jeden Store-and-Forward-Pfad ist das ein echtes Loch. Ein `s.peek() -> E?` mit Kosten eines Elements und ohne Cursor-Fortschritt wäre eine Fußnote wert.

**4. `port`-Schreibzugriffe und W1C.** Das ist kein Ergonomie-, sondern ein Korrektheitsproblem. Wenn `uart_ct = uart_ct.with_bit(4, true)` zu Read-Modify-Write des ganzen Registers wird und ein anderes Bit W1C ist, habe ich gerade ein Ereignis gelöscht, das ich nie gesehen habe. Die Referenz sagt „sofort und in Programmordnung", aber nichts über Feld- gegen Ganzregisterzugriff. Vorschlag: Registerfelder tragen eine Zugriffsart (`rw`, `ro`, `wo`, `w1c`, `rsvd`), der Compiler erzwingt sie und synthetisiert den korrekten Zugriff. Das ist die Stelle, an der Takt auf Treiberebene *tatsächlich* sicherer als C wäre statt nur gleich sicher.

**5. Polarität von `check … for d` gegen `alert … for d`.** Ich bin darauf hereingefallen und habe die Stall-Erkennung von Hand gebaut (`tx_stalled_for += tick`), obwohl `check not (pending and nichts_gesendet) for TX_STALL_LIMIT` genau das Konstrukt ist. 5.6 nennt die entgegengesetzte Polarität ausdrücklich und begründet sie gut. Trotzdem: Wenn ich beim zweiten Lesen darüber stolpere, stolpert der Techniker auch.

**6. Implizite Prüfungen in Parsern.** Jeder Slice mit dekodierter Länge braucht ein vorgeschaltetes `if`. Das Idiom aus 3.4 ist richtig, Oktagone (v1.1) räumen das meiste ab. Aber die Kennzahl „Anzahl impliziter Prüfungen" (3.4) wird bei Protokollcode katastrophal aussehen, und Leute werden ihren Code für schlecht halten, wo die Analyse ungenau ist. Der Report sollte trennen: *vom Programmierer geschriebene* Checks gegen *wegen Präzisionsverlust am Slice eingefügte*.

**7. Kleinkram, der summiert nervt.** Kein `str.as_bytes()`. Kein `wrap_u8` (3.10 nennt nur `wrap_u16`). Sechsmal `var _p = b.push(...)`, weil ein ignoriertes `bool` keinen Ort hat — die Grammatik erlaubt `expr` als Statement, also ist es vermutlich schon legal, aber die Referenz sollte sagen, dass das Verwerfen eines `push`-Ergebnisses nach geprüfter Kapazität das Idiom ist. Drei fast gleiche Textpuffertypen (`str<N>`, `line<N>`, `bytes<N>`), bei denen ich jedes Mal nachschlagen musste, welcher `.truncated` hat.

## Was ich als Nächstes tun würde

Wenn ich priorisieren müsste: **1** (Index-Schreiben auf `bytes`) ist ein Einzeiler in der Referenz und schaltet einen ganzen Anwendungsbereich frei. **4** (Registerfeld-Zugriffsarten) entscheidet, ob die Treiberstufe in v1.2 ein Feature oder eine Falle ist. **2** (Muster über dekodierte Sichten, oder ein `on s as e when <guard>`) ist die strukturell teuerste Lücke und verdient eine eigene Entscheidung im Log, weil beide Auswege — Handler-Guard oder Dekodierung am Rand — je einen Preis haben.

Und der Lesbarkeitsbefund: Die behauptete Zugänglichkeit ist für §14.1-Code echt und für §14.6/§14.8-Code nicht. Das spricht für zwei Dokumente statt eines, nicht für eine einfachere Sprache.