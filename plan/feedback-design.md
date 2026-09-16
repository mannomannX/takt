# Entwurf der offenen Rückmeldungen

Stand 2026-09-11. Bezug: `plan/feedback.csv` (43 Einträge, 12 offen),
`plan/definition.md` v0.2.8, `plan/plan.md`.

Dieses Dokument entwirft die neun offenen Befunde, die *jetzt* entworfen
gehören — nicht, weil sie jetzt gebaut werden, sondern weil ihre
Entscheidung die Form späterer Meilensteine bestimmt. Drei Befunde
(FB-21, FB-22, FB-23) sind Irrtümer ohne Handlungsbedarf und stehen
nicht hier.

Jeder Abschnitt endet mit einer Festlegung. Was die Referenz oder die
Inventur berührt, steht in Abschnitt 11 gesammelt.

---

## 0. Einordnung: was wann entschieden werden muss

| Befund | Thema | Stufe | Warum jetzt entwerfen |
|---|---|---|---|
| FB-11 | Zugriffsarten an Bitfeldern (`w1c`) | v1.2 | Korrektheit, nicht Ergonomie: RMW über W1C löscht ungesehene Ereignisse |
| FB-16 | Aktiv-low-Bits | v1.2 | Dieselbe Struktur wie FB-11; zusammen entworfen ist es eine Änderung statt zwei |
| FB-10 | Zulassungskriterium der Treiberstufe | v1.2 | Bestimmt, ob die Treiberstufe überhaupt tragfähig ist |
| FB-18 | Lint für vergessenes `follows` | M6 | Gehört in Prüfung 33, die M6 ohnehin anfasst |
| FB-38 | Lint für `follows` in der Abort-Phase | M6 | Dieselbe Prüfung, derselbe Durchlauf |
| FB-14 | Muster über dekodierten Sichten | v1.1 | Entscheidet, ob nichttriviale Rahmung Handler-Dispatch behält |
| FB-13 | Tabellengetriebene Gerätekonfiguration | v1.1 | Grammatikfrage: Segment-Default oder `until` als Statement |
| FB-15 | `peek` auf Strömen | v1.1 | Berührt Lemma 9.6.1 — die Frage muss vor dem Bau beantwortet sein |
| FB-17 | Entdimensionierung benennen | v1.1 | Kleine Bibliotheksfrage, aber sie berührt 3.2 |
| FB-12 | `signal` mit Nutzlast | v1.1 | Nach FB-06 neu zu bewerten |

**Zwei Befunde werden abgelehnt** (FB-12, FB-17), einer wird *verkleinert*
(FB-13), die übrigen sechs werden gebaut. Begründungen unten.

---

## 1. FB-11 und FB-16: Zugriffsarten und Polarität an Bitfeldern

Beide sitzen an `BitfieldDef` (`crates/takt-mir/src/types.rs`), beide
ändern Lesen und Schreiben eines benannten Bitfelds, beide sind v1.2
(Treiberstufe). Sie zusammen zu entwerfen ist nicht Bequemlichkeit: Wer
sie nacheinander baut, ändert dieselbe Senkung zweimal und muss die
Wechselwirkung (ein `active_low`-Feld, das zugleich `w1c` ist) hinterher
klären statt vorher.

### 1.1 Der Befund

Ein Statusregister mit W1C-Bits (*write one to clear*) verliert
Ereignisse, wenn man es mit Read-Modify-Write beschreibt:

```
# Heute, ohne Zugriffsarten — falsch auf echter Hardware:
sr.overflow = true        # liest das ganze Register, setzt ein Bit, schreibt zurueck
                          # -> jedes andere W1C-Bit, das seit dem Lesen kam, wird geloescht
```

Das ist kein Ergonomieproblem. Es ist die Fehlerklasse, die
Treiberentwickler kennen und die in C durch Disziplin vermieden wird —
also genau das, was eine Sprache mit dem Anspruch „if it compiles it
cannot crash" strukturell ausschließen sollte.

Aktiv-low ist dieselbe Stelle mit anderer Wirkung: Ein CiA-402-Controlword
trägt Bits, deren logische Bedeutung invertiert ist. Heute muss der
Programmierer beim Bauen ein Bit *weglassen*, um es logisch zu setzen —
die Stelle, die der Servo-Bericht „einem Techniker am wenigsten erklären
möchte".

### 1.2 Abwägung: wo steht die Zugriffsart?

| | am Bitfeld | am Trägerfeld | am ganzen Record | im `port`-Decl |
|---|---|---|---|---|
| trifft die Granularität der Hardware | ja | nein: ein Register mischt `rw` und `w1c` | nein | nein |
| lesbar an der Fundstelle | ja | teilweise | nein | nein |
| Aufwand | `BitfieldDef` + Senkung | gering, aber zu grob | gering, zu grob | groß |
| Wechselwirkung mit `layout` (Drahtformat) | keine: nur `port`-Records tragen sie | dieselbe | dieselbe | — |

**Festlegung: Die Zugriffsart steht am Bitfeld**, als optionaler Modifikator
hinter Position und Typ:

```
port sr : StatusReg @ mmio(0x4000_0C00)

record StatusReg layout little:
    flags : u32 with bits:
        ready    : bool at 0  ro          # nur lesen
        overflow : bool at 1  w1c         # schreiben einer 1 loescht
        enable   : bool at 2  rw          # Default, darf entfallen
        fault_n  : bool at 3  rw active_low
        _rsvd    : u8   at 4..7 rsvd      # reserviert: lesen liefert 0, schreiben verboten
```

Erlaubte Arten: `rw` (Default), `ro`, `wo`, `w1c`, `w0c`, `rsvd`.
`active_low` ist ein *unabhängiger* Modifikator und darf mit jeder Art
außer `rsvd` zusammenstehen — Polarität und Zugriffsart sind orthogonal,
und sie in einem Schlüsselwort zu vermischen erzeugt eine Kombinatorik,
die niemand liest.

### 1.3 Was der Compiler daraus macht

| Zugriffsart | Lesen | Schreiben |
|---|---|---|
| `rw` | Bit aus dem Träger | RMW, wie heute |
| `ro` | Bit aus dem Träger | **Fehler** (Prüfung 46) |
| `wo` | **Fehler** — der gelesene Wert ist nicht der geschriebene | ohne RMW: übrige Bits 0 |
| `w1c` | Bit aus dem Träger | **kein RMW**: nur das eine Bit als 1, alle übrigen 0 |
| `w0c` | Bit aus dem Träger | **kein RMW**: nur das eine Bit als 0, alle übrigen 1 |
| `rsvd` | **Fehler** | **Fehler** |

Der entscheidende Punkt ist die dritte Zeile von unten: Eine Zuweisung an
ein `w1c`-Feld senkt **nicht** auf Read-Modify-Write, sondern auf einen
einzelnen Schreibvorgang mit einer Maske, die genau dieses Bit trägt. Damit
ist das Löschen ungesehener Ereignisse strukturell unmöglich, nicht
Gegenstand von Disziplin.

Zwei Regeln fallen zusätzlich an:

1. **Ein Träger, der mindestens ein `w1c`/`w0c`-Feld enthält, darf nicht
   als Ganzes geschrieben werden** (`sr.flags = x` ist ein Fehler). Sonst
   entstünde der RMW über die Hintertür.
2. **`active_low` invertiert transparent an genau zwei Stellen**: beim
   Senken eines Lesezugriffs und beim Senken eines Schreibzugriffs. Der
   Träger selbst bleibt roh — wer `sr.flags` liest, sieht die
   Hardware-Bits. Das ist die einzige konsistente Wahl: Die Sicht ist
   benannt und invertiert, der Träger ist Rohspeicher.

### 1.4 Wirkung auf `layout`-Records

Zugriffsarten sind für `port`-Records (v1.2, MMIO) gedacht. An einem
gewöhnlichen `layout`-Record — einem Drahtformat — haben `w1c` und `ro`
keine Bedeutung, weil es keinen Baustein gibt, der „schreiben" von
„senden" unterscheidet.

**Festlegung: `rw`, `ro`, `wo`, `w1c`, `w0c` und `rsvd` sind nur an
Records erlaubt, die über einen `port` gebunden sind; an einem
`layout`-Record ohne `port` sind sie ein Fehler mit Hinweis.**
`active_low` dagegen ist überall erlaubt — ein invertiertes Bit in einem
Drahtformat ist ein gewöhnlicher Kodierungsfall (viele Feldbusse haben
welche).

### 1.5 Umfang

| Schicht | Arbeit |
|---|---|
| Grammatik | `bitfield_decl` um `[ access ] [ "active_low" ]` erweitern; sechs kontextuelle Wörter |
| AST | zwei Felder an der Bitfeld-Deklaration |
| MIR | `BitfieldDef` um `access: Access` und `active_low: bool`; Formatversion +1 |
| Sema | Senkung von Lesen und Schreiben je Art; Prüfung 46 um die vier Fehlerfälle |
| Interpreter | nichts Neues — die Senkung erzeugt vorhandene Bitoperatoren |
| Referenz | 3.7 (Zugriffsarten), 15 (v1.2-Liste), Prüfung 46 in 10.1 |

Der Interpreter bleibt unberührt, weil die Arten beim *Senken* aufgelöst
werden und nicht zur Laufzeit. Das ist der Grund, die Entscheidung am
Bitfeld und nicht im Interpreter zu treffen.

---

## 2. FB-10: Zulassungskriterium der Treiberstufe

### 2.1 Der Befund

Ein `driver machine` (v1.2) pollt ein Gerät im Tick. Das geht nur, wenn
das Gerät zwischen zwei Ticks nicht überläuft:

```
FIFO-Tiefe / Byterate  >  T0 + Jitter
```

Bei 921 600 Bd und 16 Byte FIFO sind das 16 · 10 / 921 600 ≈ 173 µs gegen
einen 100-µs-Tick — 73 µs Reserve. Der Treiber läuft dann in 99,9 % der
Ticks und verliert im Rest Daten. Genau die Fehlerklasse, die die Sprache
ausschließen soll, und dazu steht heute **keine Zeile** in der Referenz.

### 2.2 Abwägung: Wer entscheidet, ob ein Gerät pollbar ist?

| | Laufzeitprüfung | Konvention in der Doku | statische Prüfung aus der Hardware-Konfiguration |
|---|---|---|---|
| findet den Fehler vor dem Einsatz | nein | nein | ja |
| braucht Eingaben, die es gibt | — | — | ja: `driver-test` misst Byterate und FIFO-Tiefe ohnehin (13.8) |
| Ergebnis für den Entwickler | Fault im Feld | „hätte man wissen müssen" | „dieses Gerät ist nicht pollbar, es gehört in die TCB" |
| passt zur Linie der Sprache | nein | nein | ja |

**Festlegung: eine statische Prüfung, Kandidat SC-59, mit dem Urteil
„nicht pollbar".**

Die Prüfung ist nur möglich, weil die Hardware-Konfiguration (8.10) die
Zahlen ohnehin führt: `driver-test` misst je Input die Latenz der
Abtastung und schreibt sie dort hin (13.8). Die Prüfung liest, was schon
gemessen wird — sie verlangt keine neue Eingabe vom Programmierer.

### 2.3 Die Regel

Für jede `driver machine` m und jedes von ihr gepollte Gerät d:

```
fifo_depth[d] / byte_rate[d]  >=  P_m + jitter[tick_source] + wcet_poll[d]
```

- `fifo_depth[d]` und `byte_rate[d]`: aus der Hardware-Konfiguration (8.10)
- `P_m`: die Periode der Treibermaschine (1.3)
- `jitter[tick_source]`: gemessen (7.1, 13.8)
- `wcet_poll[d]`: das Aktivierungsbudget von m, über `c_target` in Zeit
  umgerechnet (9.4.3) — die Rechnung, die M3 bereits gebaut hat

Verletzung ist ein **Fehler**, nicht eine Warnung, mit drei benannten
Auswegen: Periode senken, Gerät in die TCB geben (Treiber in Rust,
12.6), oder DMA statt Polling verwenden.

**Der Grenzfall ist bewusst streng.** Steht eine der vier Größen nicht in
der Konfiguration, ist das kein Freibrief: Die Prüfung meldet dann „nicht
entscheidbar" und verlangt entweder die Messung oder eine ausdrückliche
Freigabe (`with polling = unchecked`), die im Lauf-Header erscheint. Ein
unbewiesener Treiber darf laufen, aber nicht unbemerkt.

### 2.4 Warum das die Treiberstufe trägt statt sie zu bremsen

Der Befund liest sich zunächst als Einschränkung. Tatsächlich ist er die
Antwort auf die Frage, die über die Treiberstufe entscheidet: *Welche
Geräte gehören überhaupt hinein?* Ohne das Kriterium ist „Treiber in
Takt schreiben" ein Versprechen, das bei schnellen Geräten still bricht.
Mit ihm ist es eine überprüfbare Zusage mit einer benannten Grenze — und
die Grenze ist genau die, die 12.6 ohnehin zieht (was nicht pollbar ist,
ist TCB).

---

## 3. FB-18 und FB-38: die beiden `follows`-Lints

Beide sitzen an Prüfung 33, beide werden mit `follows` in M6 gebaut, beide
brauchen dieselben Angaben (Leser, Schreiber, Perioden, Fault-Pfade). Sie
gemeinsam zu entwerfen spart nicht nur Arbeit — sie sind zwei Hälften
derselben Frage: *Weiß der Programmierer, in welchem Tick er liest?*

### 3.1 FB-18: das fehlende `follows`

Liest Maschine A eine `pub var` von B, ohne `follows B` zu deklarieren,
bekommt sie den Wert des **vorigen** Ticks (Unit-Delay, 7.2). Das
übersetzt fehlerfrei und macht das Programm still um einen Tick träger.

Der Compiler hat alle Angaben: Er kennt Leser, Schreiber und Perioden.

**Festlegung: Warnung, wenn A eine `pub var` von B liest, B im selben Tick
aktiv ist (Perioden und Phasen erlauben es) und `A follows B` fehlt.**
Text: „A liest `B.x` mit einem Tick Verzögerung, obwohl B im selben Tick
läuft (`follows B` ergänzen, wenn die frische Größe gemeint ist)".

Warnung, nicht Fehler: Der Unit-Delay ist manchmal *gewollt* (ein
Regelkreis, der bewusst entkoppelt). Eine Warnung nennt die Stelle; wer
sie will, schreibt `follows` nicht und lebt mit der Meldung — oder
unterdrückt sie mit einem ausdrücklichen Attribut.

### 3.2 FB-38: die stille Bedeutungsänderung in der Abort-Phase

Prüfung 33 verbietet heute *frische Lesevorgänge in Aktionsblöcken der
Abort-Phase*. Sie sagt nichts über den Fall, den der DMX-Bericht gefunden
hat: Ein Follower liest eine gefolgte Größe, die auch auf seinem
Fault-Pfad vorkommt. In der Schrittphase ist sie frisch, in der
Abort-Phase gilt Ψ_k (7.2) — dieselbe Zeile bedeutet zwei verschiedene
Dinge, je nach Phase.

**Festlegung: Warnung, wenn eine gefolgte Größe sowohl im regulären Pfad
als auch auf dem Fault-Pfad derselben Maschine gelesen wird.** Text: „`B.x`
ist in der Schrittphase frisch, auf dem Fault-Pfad aber der Wert des
vorigen Ticks (5.4)".

### 3.3 Warum beide Warnungen und keine Fehler

Beide beschreiben legitimen Code mit einer Falle. Ein Fehler zwänge zu
einer Umformulierung, die nicht immer besser ist; eine Warnung nennt die
Stelle und lässt die Entscheidung beim Programmierer. Das entspricht der
Warnpolitik aus 3.4: warnen, wo der Compiler eine Absicht *vermutet*,
fehlschlagen, wo er eine Verletzung *beweist*.

Prüfung 33 wird damit Klasse `F / W` — bereits in Referenz und Inventur
nachgetragen.

---

## 4. FB-14: Muster über dekodierten Sichten

### 4.1 Der Befund

`on s matches Rec(feld = wert)` funktioniert auf einem `stream<Rec>`. Ist
das Element `bytes<N>` — weil die Rahmung nichttrivial ist (COBS, CRC) —,
liegt der typisierte Record erst nach eigener Dekodierung vor, und dann
ist der zustandsweise Dispatch verloren: Der Handler muss alles annehmen
und im Rumpf verzweigen.

Die Zwangswirkung, die der UART-Bericht beschreibt, ist real:
nichttriviale Rahmung wird TCB, oder sie verliert Handler-Dispatch.

### 4.2 Abwägung

| | (a) `on s as e when <guard>:` | (b) Muster über Ausdrücken | (c) `decode` im Muster |
|---|---|---|---|
| löst den Fall | ja: der Guard darf dekodieren | ja | ja |
| neue Grammatik | ein optionales `when` am Handler | Muster werden Ausdrücke — großer Eingriff | `matches decode(E)(…)` — Sonderform |
| Kosten im Budget sichtbar | ja: der Guard ist gewöhnlicher Code | ja | verdeckt: `decode` läuft je Muster |
| erschöpfende Prüfung bleibt | ja (der Guard ist bool, kein Muster) | fraglich | fraglich |
| Wechselwirkung mit dem DFA (8.7) | keine: der Guard läuft *nach* dem DFA | bricht ihn | bricht ihn |

**Festlegung: (a) — ein optionaler Guard am Handler.**

```
on rx as e when decode_kind(e.data) == ACK:
    ...
```

Begründung: Es ist die einzige der drei Formen, die den Muster-DFA aus 8.7
unangetastet lässt. Muster sind dort in Alphabetklassen übersetzt und je
Zustand zu einem DFA verschmolzen; ein Muster, das einen Funktionsaufruf
enthält, zerstört diese Konstruktion. Ein Guard dagegen ist gewöhnlicher
Code, der *nach* dem Musterabgleich läuft — seine Kosten stehen im Budget,
seine Totalität ist die gewöhnliche, und `matches`/`has` bleiben, was sie
sind.

Zwei Regeln:

1. **Der Guard ist seiteneffektfrei** (4.4) — er ist ein Ausdruck, keine
   Anweisung. Damit bleibt die Auswertungsreihenfolge zwischen Handlern
   irrelevant.
2. **Ein Element, dessen Guard `false` liefert, gilt als untersucht** und
   wird konsumiert wie jedes andere (8.6). Alles andere ließe das Fenster
   wachsen und bräche Lemma 9.6.1.

Die zweite Regel ist die eigentliche Entscheidung, und sie ist nicht
offensichtlich: Man könnte ein nicht passendes Element auch stehen lassen.
Das wäre aber ein zweiter Cursor-Begriff neben dem aus 8.6 — und der
Bericht zu FB-06 zeigt, wie teuer ein zweiter Begriff in diesem Bereich
ist.

---

## 5. FB-13: tabellengetriebene Gerätekonfiguration

### 5.1 Der Befund

Zehn nahezu gleiche `send`+`until`-Paare in einer Sequenz. Eine Schleife
über eine Konstantentabelle ist nicht schreibbar, weil `until` ein
`seq_item` ist und kein `stmt` — im Rumpf eines `for` darf es nicht
stehen.

### 5.2 Abwägung

| | (a) Segment-Default `sequence with timeout = …` | (b) `until` als Statement zulassen | (c) `repeat` über eine `const table` |
|---|---|---|---|
| löst den konkreten Fall | teilweise: spart den Timeout, nicht die zehn Paare | ja | ja |
| Grammatikeingriff | klein: ein `with` an der Kopfzeile | groß: `until` in jedem Block, auch in `loop:` | mittel |
| Semantik bleibt klar | ja | **nein**: `until` in einem `loop:` hat keine Bedeutung (6.2 setzt Segmente voraus) | ja |
| statische Schranke bleibt | ja | fraglich | ja: `repeat` hat eine Konstante |
| versteckt etwas | nein — steht in der Kopfzeile | ja | nein |

**Festlegung: (a) jetzt, (c) später prüfen, (b) nie.**

(b) scheidet aus, weil `until` seine Bedeutung aus der Sequenzstruktur
zieht: Es unterteilt ein Segment (6.2), und ein `loop:` hat keine
Segmente. `until` dort zuzulassen hieße, ihm eine zweite Bedeutung zu
geben — die Sorte Doppeldeutigkeit, die die Sprache bisher vermeidet.

(a) ist billig und löst den *Hauptteil* der Klage:

```
sequence with timeout = 2 s -> NET_DOWN:
    send cfg, SET_BAUD
    until ack
    send cfg, SET_MODE
    until ack
    ...
```

Jedes `until` ohne eigenen `timeout` erbt den des Segments. Das spart in
dem Fall des Berichts zwanzig Zeilen und versteckt nichts — der Default
steht sichtbar in der Kopfzeile.

(c) bleibt offen, weil es die Wiederholung wirklich auflöst, aber eine
Frage nach sich zieht, die hier nicht entschieden werden muss: Wie sieht
ein `until` aus, dessen Guard vom Schleifenindex abhängt? Erst wenn ein
zweiter Bericht denselben Fall bringt, lohnt die Antwort.

**FB-13 wird damit verkleinert**: von „Schleife über eine Tabelle" auf
„Segment-Default für `timeout`". Das ist kein Ausweichen, sondern die
Beobachtung, dass der teure Teil der Forderung (die Schleife) einen
kleinen Teil des Schmerzes trägt und der billige Teil (der Timeout) den
großen.

---

## 6. FB-15: `peek` auf Strömen

### 6.1 Der Befund

`s.count`, `.dropped`, `.overflowed` und `.free` lesen ohne zu
konsumieren. Was fehlt, ist der *Wert* des nächsten Elements ohne
Cursor-Fortschritt — für Backpressure-Schleifen, die vor der Entscheidung
hinsehen wollen.

### 6.2 Abwägung

Die Frage ist nicht, ob `peek` nützlich ist, sondern ob es Lemma 9.6.1
(kein Fenster-Wachstum) bricht.

| | `peek` ohne Cursor-Fortschritt | `peek` mit Markierung „untersucht" | kein `peek` |
|---|---|---|---|
| löst den Fall | ja | ja | nein |
| Lemma 9.6.1 hält | **fraglich**: wer nur `peek`t, lässt das Fenster wachsen | ja | ja |
| Kosten statisch | ja (ein Element) | ja | — |
| zweiter Cursor-Begriff | ja | nein | nein |

**Festlegung: `peek` kommt, aber als `s.peek() -> E?` mit der Regel, dass
ein `peek` das Element **untersucht** im Sinne von 8.6 — es zählt für das
Fenster, ohne den Lesecursor zu bewegen.**

Das ist der Unterschied, an dem der Entwurf hängt: 8.6 führt zwei Begriffe,
*untersucht* (für das Fenster, Lemma 9.6.1) und *konsumiert* (für den
Lesecursor). `peek` setzt den ersten, nicht den zweiten. Damit bleibt das
Fenster beschränkt — ein Programm, das nur `peek`t und nie liest, verliert
Elemente durch Überlauf statt den Speicher wachsen zu lassen, was genau
das dokumentierte Verhalten eines vollen Rings ist.

Ohne diese Regel wäre `peek` ein Loch in der zentralen Garantie der
Stromsemantik. Mit ihr ist es eine Zeile in 8.6.

Priorität bleibt niedrig: Für feste Elementgrößen löst `count` den Fall
schon, und der offene Rest (variable Länge in einem Store-and-Forward-Pfad)
ist schmal.

---

## 7. FB-17: Entdimensionierung benennen — abgelehnt

Der Servo-Bericht nennt `round(x / (1 inc/s)) as u32` ein Ritual und
schlägt `x.raw(U) -> int` vor: dieselbe Semantik, benannte Absicht.

**Festlegung: abgelehnt.**

Begründung: Der Vorschlag führt einen zweiten Weg für eine Operation ein,
die bereits einen hat, und der Gewinn ist Lesbarkeit an einer Stelle, die
selten steht. Vor allem aber ist die Division durch ein Einheitenliteral
*nicht* nur Ritual — sie nennt den Umrechnungsfaktor explizit, und genau
darin liegt die Sicherheit: `x / (1 inc/s)` sagt, in welcher Einheit der
Rohwert gilt. `x.raw(inc/s)` sagt dasselbe, spart aber nichts außer vier
Zeichen.

Käme `raw`, stünden beide Formen nebeneinander, und jeder Leser müsste
wissen, dass sie dasselbe tun. Das ist teurer als die vier Zeichen.

**Stattdessen**: Ein Satz in 3.2, der die Division durch ein
Einheitenliteral als *die* vorgesehene Entdimensionierung benennt. Der
Bericht zeigt, dass sie als Behelf gelesen wird, nicht als Idiom — das
ist ein Doku-Befund, kein Sprachbefund.

---

## 8. FB-12: `signal` mit Nutzlast — abgelehnt

Der Servo-Bericht nennt es seine Nummer-eins-Ergänzung: ein Signal, das
einen Wert trägt.

**Festlegung: abgelehnt, mit Verweis auf den vorhandenen Weg.**

Nach der Klarstellung aus FB-06 ist der Hauptgrund entfallen: Jede
Maschine hat ihren eigenen Cursor, jeder Interessent kann einen internen
Strom direkt lesen. Ein `stream<E>` mit `capacity = 1` leistet genau, was
der Bericht will — ein Ereignis mit Wert, das jeder Leser einmal sieht —
und existiert seit M2.

Der Rest der Forderung („ohne Stream-Deklaration") ist Bequemlichkeit, und
die Kosten wären hoch: `signal` ist heute ein reiner Puls mit trivialer
Semantik (5.8). Eine Nutzlast brächte die Frage nach Kapazität, Überlauf
und Lebensdauer — also genau die Fragen, die `stream<E>` bereits
beantwortet.

**Stattdessen**: Ein Satz in 5.8, der auf den internen Strom als Weg für
„Ereignis mit Wert" verweist. Auch hier ist der Befund ein Doku-Befund.

---

## 9. Was sich daraus für die Reihenfolge ergibt

Die neun Befunde verteilen sich auf drei Zeitpunkte:

**Jetzt (Referenz, ohne Code):** FB-17 und FB-12 werden zu je einem Satz in
3.2 und 5.8. Beide sind abgelehnt, aber beide zeigen eine Doku-Lücke, und
die zu schließen ist billiger als der nächste Bericht, der denselben
Wunsch äußert.

**M6, mit `follows` und 8.10:** FB-18 und FB-38 als die beiden Warnungen an
Prüfung 33; FB-13(a) als Segment-Default; FB-14 als Handler-Guard; FB-15
als `peek`. Alle fünf sind Sema-Arbeit ohne Codegen-Anteil.

**M8, mit der Treiberstufe:** FB-11, FB-16 und FB-10 — die drei, die MMIO
voraussetzen. Sie zusammen zu bauen ist die Empfehlung aus Abschnitt 1:
dieselbe Struktur, dieselbe Senkung, dieselbe Prüfung.

Eine Ausnahme ist zu erwägen: **FB-10 könnte früher entworfen und später
gebaut werden**, weil sein Ergebnis („welche Geräte gehören in die
Treiberstufe") den Zuschnitt von M8 beeinflusst. Der Entwurf steht in
Abschnitt 2; er braucht keine Zeile Code, bis `port` existiert.

---

## 10. Die drei verschobenen Prüfungen

SC-28, SC-32 und SC-39 standen auf `offen` in M3, obwohl M3 für sie alles
gebaut hat, was ohne Messwerte baubar ist: `takt size` gibt die Posten aus,
`Machine::budget` die Vektoren, und die Schedulability-Formel steht. Was
fehlt, ist die *Eingabe* — `jitter` je Output, `c_target[c]` je Target,
`ram`/`flash`/`iram` je Profil.

Diese Werte entstehen an genau zwei Stellen: `driver-test` misst sie
(13.8, M5), und die Hardware-Konfiguration führt sie (8.10, M6).

**Festlegung: alle vier calibrierungsabhängigen Prüfungen (28, 29, 32, 39)
stehen in M6.** SC-29 stand bereits auf `definiert`, weil `campaign` eine
Stufe meldet und die Ablehnung testbar ist; die anderen drei haben kein
Konstrukt, das eine Stufe melden könnte, und bleiben bis dahin ehrlich
`offen`.

Damit hat **M3 keine offenen Einträge mehr**: 38 fertig, 18 definiert,
2 teilweise (SC-12 und SC-25, beide mit dokumentiertem Grund).

Der Plan nennt in Abschnitt „Kandidaten zum Vorziehen" das Lesen der
Hardware-Konfiguration als eine der drei Arbeiten, die M3 spürbar
entlasten würden, und empfiehlt, „nach M3 zu bewerten". Die Bewertung
fällt so aus: **nicht vorziehen.** Eine Konfigurationsdatei zu lesen ist
billig, aber ohne gemessene Werte enthält sie nichts — und die Messung
kommt mit M5. Die Prüfungen früher zu aktivieren hieße, sie gegen
geschätzte Zahlen laufen zu lassen; das ist schlechter als sie ehrlich
offen zu lassen.

**Korrektur (2026-09-16).** Die Bewertung galt für die Kalibrierung und
war dort richtig. Für die *anderen* Felder aus 8.10 galt sie nicht:
Kanäle (Adresse, Richtung, Einheit, Range, `safe`, Anschluss) und Speicher
(`ram`, `flash`) stehen im Datenblatt, nicht in einer Messung. Sie sind
jetzt Format 3 der Konfiguration; SC-60 und SC-39 urteilen damit, SC-28
sobald ein Jitter gemessen ist. Siehe `plan/m5.md` 3.6.

| Stelle | Änderung | Wann |
|---|---|---|
| 3.2 | Division durch Einheitenliteral als *das* Idiom der Entdimensionierung benennen (FB-17) | sofort |
| 5.8 | interner `stream<E>` mit `capacity = 1` als Weg für „Ereignis mit Wert" (FB-12) | sofort |
| 3.7 | Zugriffsarten `rw ro wo w1c w0c rsvd` und `active_low` an Bitfeldern (FB-11, FB-16) | mit v1.2 |
| 8.6 | `peek` als untersuchend, nicht konsumierend (FB-15) | mit v1.1 |
| 8.7 | Handler-Guard `on s as e when …` (FB-14) | mit v1.1 |
| 6.2 | Segment-Default `sequence with timeout = …` (FB-13) | mit v1.1 |
| 10.1 Prüfung 33 | zwei Warnungen (FB-18, FB-38) — **bereits nachgetragen** | erledigt |
| 10.1 Prüfung 46 | vier Fehlerfälle der Zugriffsarten (FB-11) | mit v1.2 |
| 10.1 Prüfung 59 (neu) | Zulassungskriterium der Treiberstufe (FB-10) | mit v1.2 |
| 15 | Zugriffsarten und `active_low` in der v1.2-Liste | mit v1.2 |
| features.csv | SC-28/29/32/39 nach M6; SC-59 als neue Prüfung | **SC-28/29/32/39 erledigt** |

Die beiden `sofort`-Zeilen sind der einzige Teil dieses Dokuments, der
ohne weiteren Meilenstein umgesetzt werden kann. Alles andere wartet auf
`follows` (M6) oder `port` (M8) — nicht aus Bequemlichkeit, sondern weil
die Konstrukte, an denen die Regeln hängen, dann erst existieren.
