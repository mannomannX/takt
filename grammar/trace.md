# Takt — Trace-Format (Stimulus, Golden-Trace, lesbare Aufzeichnung)

Normative Spezifikation des zeilenorientierten Textformats in
`crates/takt-interp/src/trace.rs` (Referenz 12.5, 13.1; Entwurf `plan/m1.md`,
Abschnitt 1.8). Dasselbe Format trägt drei Rollen:

- **Stimulus**: was von außen in einen Lauf hineingeht (Inputs, Commands, Abort).
- **Golden-Trace**: was ein Lauf beobachtbar tut (Outputs, Zustände, Faults,
  Beobachtungen, Verdikt).
- **Lesbare Aufzeichnung**: die Textform der Binäraufzeichnung (12.5, ab M4).

Ein Lauf ist deterministisch (Satz 9.4.1), also ist sein Trace bei gleichem
Stimulus zeichengleich. Die Zeilen eines Ticks stehen in kanonischer Ordnung,
damit der Vergleich unabhängig von der Schrittreihenfolge ist: erst die
Eingaben, dann je Maschine in Indexreihenfolge ihre Ereignisse in
Ausführungsreihenfolge, zuletzt die Ausgaben in Channel-Indexreihenfolge.

## Notation der Vektoren

Ein Block ```` ```trace ```` enthält Zeilen, die geparst und wieder
geschrieben werden; das Ergebnis muss zeichengleich sein (`tests/trace.rs`).

## T1 Zeilen

Eine Zeile ist `t=<tick> <art> <rest>`; `<tick>` ist die Tick-Nummer ab 0.
Leerzeilen und Zeilen ab `#` sind Kommentar. Felder trennt genau ein
Leerzeichen, außer im Rest einer Meldung.

| Art | Rolle | Form |
|---|---|---|
| `in` | Stimulus | `in <channel> <wert>` oder `in <channel> <qualität> [reason=<grund>] [age=<dauer>]` |
| `cmd` | Stimulus | `cmd <command>` — ein Tick lang wahr (8.5) |
| `abort` | Stimulus | `abort` — Operator-Abort für alle Maschinen (5.4) |
| `out` | Golden | `out <channel> <wert>` — beim Commit, nur bei Änderung und in Tick 0 |
| `state` | Golden | `state <maschine> <pfad>` — Blattpfad mit `.`, nur bei Änderung und in Tick 0 |
| `pub` | Golden | `pub <maschine> <variable> <wert>` — bei Änderung |
| `signal` | Golden | `signal <maschine> <name>` — im Tick des Pulses (5.8) |
| `fault` | Golden | `fault <maschine> <art> "<meldung>" -> <ziel>` |
| `log` | Golden | `log <maschine> "<text>"` |
| `alert` | Golden | `alert <maschine> on\|off "<text>"` — nur Flanken (5.6) |
| `measure` | Golden | `measure <maschine> <name> <wert>` |
| `verify` | Golden | `verify <maschine> ok\|fail "<text>"` |
| `verdict` | Golden | `verdict <maschine> pass\|fail ["<text>"]` |
| `stream` | Golden | `stream <name> dropped=<n> overflowed=<n> malformed=<n>` — bei Änderung (8.6) |
| `verdict-final` | Golden | `verdict-final PASS\|FAIL\|INCONCLUSIVE` — letzte Zeile (13.5) |

## T2 Werte

Werte stehen in der Literalschreibweise der Sprache (2.3): `true`, `42`,
`2.5`, `45 bar`, `150 ms`, `OPEN`, `"text"`, `none`, `[1, 2]`,
`CanFrame(1, 2)`. Eine Dauer wird in der größten Einheit geschrieben, in der
sie ganzzahlig ist (3.3). Ein Fließkommawert trägt immer einen Dezimalpunkt,
damit er sich von einer Ganzzahl unterscheidet.

Ein Input kann statt eines Werts eine Qualität tragen (3.5): `bad`, `stale`,
`suspect`, jeweils mit optionalem `reason=` (`Stale`, `OutOfRange`,
`Implausible`, `Driver`, `Node`) und `age=`. Ohne Angabe gilt `Good` mit
Alter 0.

```trace
t=0 in tank_p 45 bar
t=0 in ready true
t=0 in mode OPEN
t=1 in tank_p bad reason=OutOfRange
t=2 in lox_temp 90 K stale age=120 ms
t=3 cmd start
t=4 abort
```

Ein Strom trägt kein Latch, sondern ein *Element* je Zeile (8.6): der Wert
steht in der Literalform des Elementtyps. Mehrere Zeilen eines Ticks liefern
mehrere Elemente in ihrer Reihenfolge; ein Element vom Rand ist sofort
sichtbar, während ein interner Stream den Unit-Delay aus 9.6 behält. Ein
oversampelter Kanal (8.9) trägt sein Tick-Array als Liste; sie darf kürzer
als `N` sein, weil fehlende Samples der Normalfall sind.

```trace
t=1 in dut_log "Boot v2.1"
t=2 in can_rx CanFrame(0x7E8, [0x02, 0x10])
t=2 in dut_log "Update complete"
t=3 in i_dut [0.5 A, 1.0 A, 2.0 A, 1.2 A]
```

## T3 Ausgaben

```trace
t=0 out fuel_main CLOSED
t=0 state hotfire ARMED.IDLE
t=1 log hotfire "starting hotfire"
t=2 pub battery_cycle count 3
t=2 signal hotfire done
t=3 alert hotfire on "LOX warming"
t=4 measure hotfire boot_time 1500 ms
t=5 verify hotfire fail "supply not off"
t=6 fault hotfire Timeout "no pressure" -> SAFE
t=7 verdict hotfire pass "recovery ok"
t=8 verdict-final FAIL
```

Ein Ausgabestrom erscheint als `out`, sobald der Treiber Bytes abgeholt hat
(8.8: er leert `max_rate * T0` Bytes je Tick). Anders als ein Latch steht
jedes Element da, auch ein wiederholtes. Die Zähler eines Stroms sind
beobachtbar und erscheinen als eigene Zeile, wenn sie sich ändern (8.6).

```trace
t=7 out dut_tx [0x50, 0x49, 0x4e, 0x47]
t=9 stream dut_log dropped=2 overflowed=0 malformed=1
```

## T4 Halten und Wiederholen

Ein Input behält seinen Wert, bis eine neue Zeile ihn ändert (Halte-Semantik
wie ein Sensor, der weiter liefert). `cmd` und `abort` gelten genau einen
Tick (8.5, 5.4). Ausgaben, Zustände und `pub var` erscheinen nur, wenn sie
sich ändern; in Tick 0 stehen alle Anfangswerte.

Im folgenden Stimulus hält `tank_p` von Tick 0 bis Tick 2 den Wert `45 bar`
und ist ab Tick 3 ungültig; `start` gilt nur in Tick 1.

```trace
t=0 in tank_p 45 bar
t=1 cmd start
t=3 in tank_p bad reason=Driver
```

## T5 Kanonische Ordnung

Innerhalb eines Ticks:

1. `in`, `cmd`, `abort` in Channel- beziehungsweise Command-Indexreihenfolge.
2. je Maschine in Indexreihenfolge: `log`, `alert`, `measure`, `verify`,
   `verdict`, `signal`, `fault`, `state` in Ausführungsreihenfolge.
3. `pub` in Maschinen- und Variablenindexreihenfolge.
4. `out` in Channel-Indexreihenfolge; ein Ausgabestrom steht bei seinem
   Channel.
5. `stream` in Channel-, dann Stream-Indexreihenfolge.

Die Ordnung hängt nicht davon ab, in welcher Reihenfolge die Maschinen
geschritten sind; damit prüft ein Trace-Vergleich die Ordnungsunabhängigkeit
aus Satz 9.4.1 unmittelbar.
