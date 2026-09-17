# Blockierendes NVM: Was die Sprache dafür braucht

Stand: 2026-09-17. Bezug: `plan/esp32c6.md` Schritt 7 (FB-197, FB-201),
`plan/definition.md` 5.9, 7.3, 8.10, 9.9, 10 (Prüfung 32), 12.3, 16;
`crates/takt-rt-core/src/{journal,loopcore}.rs`, `takt-mir/src/hardware.rs`,
`takt-sema/src/calibrated.rs`, `takt-cli` (`size`, `check`, `build --emit
consts`), Board 2.

## 0. Die Frage und die Antwort

Board 2 hat gezeigt: Ein Journal-Schreibvorgang kostet auf einem Chip mit
einem Flash für Code und Daten rund 25 Perioden zu 10 ms, und weder
RAM-Residenz noch freie Interrupts ändern daran viel, weil die
ROM-Routine synchron auf das Flash wartet. Die Frage ist, was davon
Sache der Sprache ist.

**Die Grenze.** Treiber, Linkerskripte, Cache und ROM-Routinen sind
Rust-HAL und Board; die Sprache sieht sie nicht. Was die Sprache sieht,
ist dreierlei, und genau das legt dieser Plan aus:

1. **Die Tatsache** — dass ein Ziel beim Schreiben blockiert und wie
   lange. Sie gehört in die Hardware-Konfiguration (8.10), wie `guard`,
   `jitter`, `fifo_depth` und `c_target` vor ihr. Der Ingenieur misst
   oder liest sie einmal je Board ab; danach urteilt das Werkzeug je
   Programm. Das ist die Arbeitsteilung, die Takt überall hat: Zahlen
   vom Board, Urteile vom Compiler.
2. **Das Urteil** — Prüfung 32 rechnet, was ein Schreibvorgang den Tick
   kostet, und sagt unter `overrun = fault`, ob das Programm damit
   überhaupt laufen kann. Ein Fault, der bei jedem Speichern sicher
   eintritt, gehört zur Übersetzungszeit gemeldet, nicht ins Feld.
3. **Die Wahl des Programms** — 7.3 nennt die Overrun-Policy
   „konfigurierbar", aber die Grammatik hatte keinen Schlüssel dafür;
   die Runtime bekam sie als Konstruktorargument. `system: overrun =
   fault | alert` gibt dem Programm das Wort. Und die Runtime schreibt
   von sich aus dann, wenn es nichts kostet: im Schlaf eines
   `idle`-Zustands, dessen Fenster länger ist als der Schreibvorgang.
   Dafür braucht das Programm keine neue Syntax — `idle` gibt es, und
   Satz 9.9.1 deckt die Zeit.

**Gibt es mehr solcher Fälle?** Ja, und sie haben alle dieselbe Form:
eine physische Größe, die das Tick-Modell nicht sieht. Die Referenz
behandelt sie schon so — `guard` und `jitter` für geplante Ausgaben,
`latency_ns` für Treiber, `tick_tolerance` für die Uhr, `fifo_depth`
und `byte_rate` für gepollte Geräte (Prüfung 59), `stack_reserve` und
`stack_margin` für den Stack, `t_io` für den Rand des Ticks. Das
Muster ist immer: Zahl in 8.10, Prüfung im Gate, ausdrückliche Freigabe
im Programm, Fault zur Laufzeit. Der Ingenieur muss die Zahl kennen,
nicht ihre Folgen. Das NVM war die Größe, die dieses Muster noch nicht
hatte; nach diesem Plan hat sie es. Weitere Kandidaten, die bei den
nächsten Boards auffallen werden: Cache-Effekte auf die WCET (Board 1
misst sie), DMA-Latenz für `samples`, die Aufwachzeit aus Light-Sleep.
Sie kommen denselben Weg, wenn eine Messung sie zeigt.

## 1. Befunde

- **Die Löschung ist unteilbar und synchron.** Ein Sektor kostet auf dem
  C6 zweistellige bis dreistellige Millisekunden; die ROM-Routine kehrt
  erst danach zurück. Eine asynchrone Form gibt es nur mit eigenem
  SPI-Treiber (Kommando anstoßen, Status pollen) und vollständiger
  RAM-Residenz des Ticks samt Runtime und Konstanten.
- **Der `Nvm`-Trait ist asynchron entworfen** (`begin_*`, `poll`), die
  Referenz nimmt es an (12.3, Absatz 2). Ein blockierender Treiber
  erfüllt den Trait formal und bricht die Annahme still.
- **Die Schleife pollt das Journal nach dem Schritt, in der Tickzeit.**
  Ein 250-ms-Vorgang passt in keinen 10-ms-Tick, gleich wo der Code
  liegt.
- **Die Overrun-Policy hat keine Syntax.** `Policy::Fault` steht als
  Konstante im Bring-up.
- **Der Slot wird je Schreibvorgang gelöscht**, auch wenn er noch Platz
  hätte: Verschleiß und Stillstand je Eintrag statt je Sektor.

## 2. Abwägungen

### 2.1 Wo die Tatsache steht

| Option | Für | Gegen |
|---|---|---|
| **8.10: `nvm_erase_ns`, `nvm_program_ns`, `nvm_blocking`** | dasselbe Muster wie Kalibrierung und Kanäle; das Gate liest es; `takt bench` kann es messen | ein Board ohne Konfiguration bleibt ununterscheidbar von einem asynchronen |
| Der Treiber meldet es (`Nvm::blocking_ns`) | die Wahrheit liegt beim Treiber; die Runtime braucht sie ohnehin | zur Übersetzungszeit unbekannt |
| Im Programm (`persist … with blocking = …`) | explizit | die Sprache müsste die Hardware kennen |

**Entscheidung: beides, mit einer Richtung.** Die Konfiguration ist die
Quelle für das Gate, der Treiber meldet den Wert der Runtime; `takt
build --hardware` exportiert ihn als Konstante, damit beide dieselbe Zahl
tragen. Fehlt `nvm_blocking` auf einem Ziel mit `iram`, meldet Prüfung
32 „nicht entscheidbar", wie 59 bei fehlendem `fifo_depth`.

### 2.2 Wie das Programm wählt

**Entscheidung: `system: overrun = fault | alert`**, Default `fault`.
Kein Logikanteil (wie `target`): Die Semantik erzeugt nie einen Overrun,
die Policy ist die Reaktion auf ein physisches Ereignis. Ein zweiter
Schalter nur für das Journal wäre eine zweite Wahrheit für dieselbe
Sache; wer den Stillstand annimmt, nimmt Overruns an.

Die Fälle, die Prüfung 32 unterscheidet, wenn das Ziel blockiert:

| `overrun` | `idle`-Zustand | Urteil |
|---|---|---|
| `fault` | keiner | **Fehler**: jeder Schreibvorgang wäre ein Fault, und die Runtime schriebe nur noch vor `reboot`/Deep Sleep — dann trägt `persist` nichts |
| `fault` | vorhanden | Hinweis mit der Zahl: Das Journal schreibt nur in Schlaffenstern, die länger sind als `erase + program`, und beim Flush |
| `alert` | beliebig | Warnung mit der Zahl: jeder Schreibvorgang ist ein Overrun-Alert |

### 2.3 Wann die Runtime schreibt

**Entscheidung: im Fenster vor dem nächsten Tick.** Nach `tick()` weiß
die Schleife, wie lange sie bis zur nächsten Frist warten wird —
einen Tick, oder bei Schlaf viele. Ist das Fenster größer als
`Nvm::blocking_ns()`, läuft die Journal-Phase darin; die Wartezeit ist
ohnehin verloren. Unter `alert` läuft sie immer. Das ändert am Trace
nichts (Satz 9.9.1: geschlafene Ticks sind leere Schritte), an der
Runtime nur die Reihenfolge, und es braucht keine neue Schnittstelle:
`next_deadline`, `sleep_allowed` und `deadline` gibt es. Verworfen:
verlorene Ticks still als virtuelle buchen — das wäre Schlaf ohne
`idle`, und 9.9 verbietet ihn.

### 2.4 Weniger Löschungen: das Log-Journal

**Entscheidung: Einträge anhängen, löschen erst bei vollem Slot.**
Slotformat Version 2: Ein Slot trägt Einträge hintereinander (Kopf 32
Byte plus Nutzlast, auf vier Byte ausgerichtet); gelöscht wird der
*andere* Slot, wenn der aktive voll ist. Das erfüllt 5.9 unverändert:
Ping-Pong der Slots, Sequenznummer und CRC je Eintrag, ein
Schreibvorgang ändert genau einen Slot, beim Start gewinnt die höchste
Sequenznummer. Ein Slot der Version 1 ist ein Slot mit einem Eintrag
und lädt wie bisher. Gewinn: bei 4 KiB und 150 Byte Nutzlast eine
Löschung je 25 Einträge; ein abgebrochenes Anhängen lässt den vorigen
Eintrag gültig. Was 5.9 nachziehen muss: „Schreiben nur mit
Sektorgranularität" wird zu „Löschen nur sektorweise".

### 2.5 Asynchrones NVM: Takt-Seite und Board-Seite

Die Referenz beschreibt den Weg (12.3, Absatz 2), der Trait trägt ihn.
Was der Sprache dafür fehlt, ist der **Nachweis der RAM-Residenz**: Ein
asynchroner Treiber löscht, während der Tick läuft, und jeder Zugriff
des Ticks auf das Flash ist dann ein Stillstand oder Schlimmeres. `takt
size --object` prüft darum je Symbol des Programms, ob es im RAM liegt,
und verlangt auf einem Ziel mit `iram` und `nvm_blocking = false`
vollständige Residenz. Das ist Werkzeugarbeit und kommt hier. Der
Treiber selbst — Kommando am ROM vorbei, Flash-Suspend als Chipfunktion
des C6 — ist Board-Arbeit ohne Sprachanteil und bleibt draußen: Er ist
genau die Kategorie, die nicht Aufgabe der Sprache ist, und er trägt das
Risiko, Daten im Flash zu zerstören, ohne dass ein Test davor warnt.
Der Trait und der Nachweis sind seine Schnittstelle, wenn ein Projekt
ihn braucht.

### 2.6 Hardwarewahl

FRAM und EEPROM schreiben in Mikrosekunden ohne Löschen; Dual-Bank-Flash
löscht eine Bank, während die andere Code liefert; ein externer
Datenflash blockiert den Code nie. Für Geräte, deren `persist` häufig
schreibt, ist das die Empfehlung — ein Absatz in 12.3, keine Zeile Code.

## 3. Arbeit je Werkstück

| Werkstück | Arbeit |
|---|---|
| Referenz 2.3, `grammar/takt.ebnf`, 2.2 | `system_item` um `"overrun" "=" ( "fault" \| "alert" )`; `overrun`, `fault`, `alert` als kontextuelle Wörter; `check_grammar.py --sync` |
| `takt-syntax` | `SystemItem::Overrun`, Parser, Formatter, S-Ausdruck |
| `takt-sema`, `takt-mir` | `Config.overrun` (kein Logikanteil, Schema-Feld 10); `hardware.rs`: NVM-Zeiten, `takt-hw 4`; `persist::journal_cost`; Prüfung 32 in `calibrated.rs` mit den drei Fällen aus 2.2 |
| `takt-cli` | `--emit consts`/`consts-rs`: `OVERRUN_ALERT`, `NVM_BLOCKING_NS`; `size --object`: RAM-Residenz je Symbol, Urteil bei asynchronem NVM |
| `takt-llvm` | `inspect`: Symboladressen, Abschnittsgrenzen, `residency` |
| `takt-rt-core` | `Nvm::blocking_ns` (Default `None`); `step_persisting` mit Fenster; Journal Version 2 (Log); `FakeNvm` zählt Löschungen |
| `takt-rt-linux` | `FileNvm::blocking_ns` bleibt `None` (Hintergrund-Thread) |
| Board 2 | `FlashNvm` misst Lösch- und Schreibzeit und meldet `blocking_ns`; `takt.rs` nimmt die Policy aus den Konstanten und druckt die Messung; `build.rs` reicht die Hardware-Konfiguration durch |
| Korpus | `58_persist_alert.takt` (`overrun = alert`), `59_persist_idle.takt` (`idle` mit Schlaffenster); `corpus-try/hw/esp32c6.hw` mit gemessenen Zeiten; `checks/SC-32/` mit blockierendem Ziel |
| Referenz 5.9, 7.3, 8.10, 12.3, 10 (32), 16 | Abschnitt 6 |

## 4. Tests

| Aussage | Test |
|---|---|
| `overrun = alert` parst, formatiert, senkt, steht in den Konstanten | Golden `58_persist_alert`, `cli_build.rs` |
| Prüfung 32: die drei Fälle aus 2.2 | `checks/SC-32/bad_persist_blocking.takt`, `ok_persist_idle.takt`, `ok_persist_alert.takt` mit `.hw` |
| Fehlendes `nvm_blocking` auf einem XIP-Ziel ist nicht entscheidbar | `calibrated.rs`-Test |
| Die Schleife schreibt auf blockierendem NVM nur im Fenster | `persist_loop.rs`: `a_blocking_device_writes_only_in_a_sleep_window`, `under_alert_it_writes_at_once`, `without_a_window_only_the_flush_writes` |
| Log-Journal: Anhängen ohne Löschen, Wechsel bei vollem Slot, höchste Sequenz gewinnt, abgebrochenes Anhängen, Version 1 lädt | `journal.rs` |
| Residenz: Symbol im RAM erkannt, im Flash gemeldet; Urteil nur bei asynchronem NVM | `inspect.rs`-Tests, `size`-Test |
| Board: Messung von Löschen und Programmieren; 57 unter `alert` mit Zahl; 59 schreibt im Schlaf ohne verpasste Perioden | `board_esp32c6.rs` |

## 5. Schritte

| Nr. | Schritt | Größe |
|---|---|---|
| 1 | `system: overrun` Ende-zu-Ende, Konstanten, Bring-up | S |
| 2 | NVM-Zeiten in 8.10, `journal_cost`, Prüfung 32, Korpus | M |
| 3 | Runtime: `blocking_ns`, Fenster, Tests; Board misst und meldet | M |
| 4 | Log-Journal Version 2 | M |
| 5 | RAM-Residenz in `takt size` | S |
| 6 | Referenz, Register, Board-Lauf, Stand | S |

## 6. Was Referenz und Register nachziehen

| Stelle | Nachzug |
|---|---|
| 2.2, 2.3 | `overrun`, `fault`, `alert`; `system_item` |
| 7.3 | „Policy konfigurierbar" → `system: overrun = fault \| alert` |
| 8.10 | Zeile NVM: `nvm_sector_bytes`, `nvm_sectors`, `nvm_min_interval`, `nvm_erase_ns`, `nvm_program_ns`, `nvm_blocking` |
| 10, Prüfung 32 | zweite Klausel: blockierendes NVM, die drei Fälle |
| 5.9 | Log-Form; „Löschen nur sektorweise"; `min_interval` begrenzt Verschleiß, nicht Stillstand |
| 12.3 | Absatz 2 bedingt: asynchrones NVM oder Schreiben im Schlaffenster; Residenznachweis; Hardwarewahl |
| 16 | Zeile „Blockierendes NVM" |
| `features.csv` | neue Grammatik-IDs aus der Extraktion; `SC-32`, `PAR-5.9`, `FMT-Journal-Slot` Version 2, `FMT-Hardware` Version 4 |
| `feedback.csv` | FB-202 (Policy ohne Syntax), FB-203 (Journal ohne Log) |

## 7. Stand

| Nr. | Stand |
|---|---|
| 1 | offen |
| 2 | offen |
| 3 | offen |
| 4 | offen |
| 5 | offen |
| 6 | offen |
