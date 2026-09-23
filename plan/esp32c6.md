# Board 2: ESP32-C6

Stand 2026-09-17. Entscheidung aus dem Gespräch vom selben Tag: Der
ESP32-C6 ist Board 2 (RV32IMAC) aus `plan/m5.md` 2.1 und zugleich das
erste Board der Profilfamilie `xip_flash` (12.3). Er ersetzt Board 1
nicht — die FPU-Probe und die Zeitmessungen aus 7.5 bleiben beim
STM32F401 (`plan/m5.md` 4, Punkt 2).

## 1. Warum dieses Board, und was es nicht leistet

**Dafür.** Der Chip ist genau die Zielklasse `Mcu32NoFpu`
(`takt-llvm/src/target.rs`, `RISCV32IMAC`): `f32` kommt aus `libtaktm`,
und der Differentialtest Interpreter ≡ x86-64 ≡ riscv32imac wird damit
zum ersten Mal auf einem Mikrocontroller geführt. USB-Serial-JTAG liegt
im Chip: ein Kabel liefert den Trace-Rückkanal *und* den Debugger; der
TTL-Adapter, der Board 1 blockiert, fehlt hier nicht. `probe-rs` 0.32
kennt den Chip (`esp32c6`), das Rust-Target `riscv32imac-unknown-none-elf`
ist installiert, `clang` übersetzt den C-Rahmen für RV32.

**Dagegen, und darum nur Board 2.** Code läuft aus dem SPI-Flash über
einen Cache. Zeitmessungen (Jitter für Prüfung 28/29, Kosten hinter SC-12,
Präzision aus 7.5) beschreiben dann das Cache-Verhalten, nicht das
`baremetal`-Profil; sie gehören zu Board 1. Funk bleibt aus (12.8). Die
RGB-LED der Espressif-DevKits hängt an einem WS2812 (IO8), nicht an
einem Pin — sie bekommt ihre 24 Bit über den RMT-Baustein (`led.rs`).

| | |
|---|---|
| Kern | RV32IMAC, 160 MHz, ohne FPU (`f32` und `f64` aus `libtaktm`) |
| Flash | 4 MB im Modul, XIP über Cache — Profilfamilie `xip_flash` (12.3) |
| RAM | 512 KB HP-SRAM; `esp-hal` richtet 451 600 Byte ein, aus denen Daten *und* RAM-residenter Code kommen (`corpus-try/hw/esp32c6.hw`) |
| Tick | SYSTIMER (52-bittig, 16 MHz), Alarm auf Vergleichswert |
| Zyklen | CSR `mcycle` (statt DWT) |
| Telemetrie | USB-Serial-JTAG (kein Adapter), verlustfrei mit Host: eigener Schreiber auf dem `esp-hal`-Treiber (FB-200) |
| LED | WS2812 an IO8, über RMT-Kanal 0 |
| Flashen | `probe-rs` über USB-Serial-JTAG oder `espflash`; Abbild im ESP-Format vom ROM-Bootloader geladen |

## 2. Schnitt

Dieselbe Trennung wie beim STM32 (`plan/m5.md` 2.2): `takt-rt-core` und
`takt-rt-baremetal` bleiben unberührt; das Board erfüllt ihre vier Traits
(`TickSource`, `HardwareWatchdog`, `StackGuard`, `Sleep`); die rechnende
Hälfte (`takt-board-support`: Perioden, Zyklen in Zeit) ist bereits
architekturneutral und wird wiederverwendet.

- `crates/takt-board-esp32c6` — Registerzugriff und sonst nichts, eigener
  Workspace wegen `unsafe` (13.4 wie beim F401-Crate). Module wie beim
  F401: `tick` (SYSTIMER), `cycles` (`mcycle`), `uart` (USB-Serial-JTAG),
  `program` (die `takt_mcu_*`-Symbole), `guard`.
- `crates/takt-bringup-esp32c6` — `build.rs` wie beim F401 mit
  `--target riscv32imac` und `--target=riscv32-unknown-none-elf -march=rv32imac -mabi=ilp32`
  für den C-Rahmen; `takt.toml` nennt das Programm; Binärprogramme
  `minimal`, `tick`, `takt`.
- **Start, Speicherkarte und Abbildformat kommen aus `esp-hal`**
  (`esp-riscv-rt`, die Linker-Skripte, das Cache-Mapping). Das ist die
  eine bewusste Abweichung von „PAC statt HAL": Die Speicherkarte eines
  XIP-Chips mit ROM-Bootloader selbst zu schreiben wäre Bring-up-Arbeit
  ohne Erkenntnis. Das Zeitmodell der HAL wird *nicht* benutzt — der Tick
  liest den SYSTIMER über die PAC, wie der F401 den TIM2.

## 3. Schritte

| # | Schritt | Beleg |
|---|---|---|
| 1 | Werkzeugkette: `probe-rs` sieht das Board (`probe-rs info`), Rust-Target und `clang` für RV32 vorhanden, `takt build --target riscv32imac` erzeugt ein Objekt aus `29_heartbeat.takt` | Objekt entsteht, `llvm-nm` zeigt `takt_mcu_tick` |
| 2 | `takt-bringup-esp32c6/minimal`: Start über `esp-hal`, Zeile über USB-Serial-JTAG, Endlosschleife | Zeile auf dem Host |
| 3 | `takt-board-esp32c6`: `SystimerTick` (Alarm-Interrupt, `on_timer_interrupt`), `cycles::now` aus `mcycle`, `Telemetry` | `tick`-Programm: Tickzähler und `verpasste Ticks: 0` über eine Minute |
| 4 | `takt`-Programm: `Generated` + C-Rahmen (`takt_conformance::mcu`) + `TimerClock`; `29_heartbeat.takt` läuft, Trace über USB | Trace-Zeilen bitgleich mit dem Interpreter |
| 5 | Konformität: der Korpus aus `differential.rs` auf dem Board, Trace nachgelagert aus dem RAM-Log (M5 4, Punkt 1); Monitore und `map`-Iteration (M6 Schritt 28) | Interpreter ≡ x86-64 ≡ riscv32imac; Register-IDs mit Blocker „Board 2" |
| 6 | 14.7 auf dem Board: `persist` in einem Flash-Sektor, `idle` mit `Sleep` (Light-Sleep, SYSTIMER weckt) | 14.7-Lauf mit Trace; Satz 9.9.1 auf Hardware |
| 7 | `xip_flash`: RAM-Residenz von Code und tick-gelesenen Konstanten (IRAM), Flash-Schreibzugriff im Lauf ohne Stillstand, `takt size` mit `iram` | PAR-12.3 (Profilfamilie `xip_flash`), SC-39 mit `iram` |
| 8 | Physische Zeit je Tick: Metazeile `time` in Trace und Telemetrie, `TickSource::now_ns`, abgeleitete Kennzahlen statt Zähler (Abschnitt 6) | `takt timing` auf einer Board-Aufzeichnung nennt die Journal-Löschung als späten Tick; Kennzahlen gegen einen eingespielten Stillstand |
| 9 | Innentick-Sicht: `takt sim --steps` im Interpreter, `pc` je Maschine im Rahmen unter `statements` (Abschnitt 6) | jede Anweisung eines Ticks als Zeile; `t=k pc` auf dem Board |

Schritte 1–4 sind der Bring-up (zwei bis vier Tage); 5–7 sind die
Hardwareanteile aus M5 und M6, die dieses Board tragen kann. Was bei Board
1 bleibt: FPU-Probe, Jitter- und Kostenmessung, `rtos` und `boot` auf dem
STM32 — oder, falls sich ESP-IDF anbietet, `rtos` (FreeRTOS) und `boot`
(Partitionstabelle, OTA) ebenfalls hier; das ist eine spätere Entscheidung.

## 4. Offene Punkte

- ~~Welche C6-Platine genau~~ — geklärt 2026-09-17: **ESP32-C6-DevKitM-1-N4**
  (Modul ESP32-C6-MINI-1, 4 MB Flash, RGB-LED WS2812 an GPIO8, BOOT an
  GPIO9; zwei USB-Buchsen: „USB" ist der native USB-Serial-JTAG, „UART"
  ein CP2102 an UART0). Die Pins stehen in den Bring-up-Programmen (`GPIO8`).
- UART0 und seine Pins (2026-09-23): Ein `port @ mmio(…)` schreibt nur
  Register; die Verbindung zum Pad macht die GPIO-Matrix, und die legt
  `takt-board-esp32c6/src/pins.rs` — ohne den HAL-Treiber, der CONF0 und
  Teiler selbst setzen und die Zuordnung beim Fallenlassen zurücknehmen
  würde. RX liegt auf GPIO17 (U0RXD), TX zurzeit auf GPIO7, weil dort die
  Schleifenbrücke des Aufbaus steckt; der Standard und die CP2102-Brücke
  wären GPIO16 (U0TXD). Gefunden mit einer Suchprobe, die einen Pin
  treibt und zählt, welcher folgt: GPIO7 gegen GPIO17 traf 81 von 81,
  jede andere Paarung die Hälfte.
- ~~Der Tick verliert Perioden, während das Journal einen Sektor löscht~~
  — geklärt in `plan/nvm.md`: Die Löschzeit (49,9 ms) steht in
  `esp32c6.hw`, Prüfung 32 urteilt, die Runtime schreibt im Schlaffenster
  oder unter `overrun = alert`, das Journal löscht nur noch bei vollem
  Slot. Ein asynchroner Treiber (Flash-Suspend) bleibt Board-Arbeit.
- Ob der Trace-Rückkanal über USB-Serial-JTAG schnell genug ist, um jede
  Zeile mitzuschreiben, oder ob der Korpuslauf aus dem RAM-Log
  nachgelagert liest (M5 4, Punkt 1). Schritt 4 schreibt alle 100 Ticks
  ohne verpasste Ticks; jede Zeile mitzuschreiben ist noch nicht gemessen.
- Der MCU-Rahmen trägt keine geplanten Ausgaben (`at`, 7.5), keine
  Systemkanäle (`sys/reboot`, `sys/jump`) und keine Jobs; die vier Programme
  bleiben dem Linux-Vergleich vorbehalten, bis der Rahmen sie hat.
- ~~Eingänge vom Board~~ — **erledigt 2026-09-22.** Der MCU-Rahmen hat
  jetzt `takt_in_*` als Gegenstück zu `takt_out_*`: Aus `hw("ui/button")`
  wird `takt_in_ui_button(&value, &quality)`, schwach gebunden wie die
  Ausgänge, vom Linker geprüft. Der Treiber liefert Wert *und* Qualität,
  weil 12.6 Eingänge degradieren lässt; antwortet er nicht, bleibt der
  Eintrag `Bad` (3.5). Belegt am BOOT-Taster (IO9, entprellt):
  `a_board_input_reaches_the_process_image`. Damit ist 14.7 nicht mehr am
  Rahmen blockiert, sondern nur noch an seinen eigenen Treibern (AFE,
  Ladegerät als Wake-Quelle).
- Watchdog: `esp-hal` hält RWDT und MWDT beim Start an; die Schleife läuft
  mit einem leeren `Watchdog` (12.3 verlangt einen echten).
- `build.rs` nimmt das `takt`-Werkzeug aus dem Release-Verzeichnis, auch
  wenn es älter ist als der Compiler (FB-193): Erster Bau des Servo-Objekts
  kam aus einem Stand vor M6 und ließ die Maschinenfunktionen fehlen. Bis
  das Skript den Stand prüft: `cargo build -p takt-cli --release` vor dem
  Bring-up.

## 6. Nachtrag: die physische Seite des Ticks (Schritte 8 und 9)

Die logische Taktgenauigkeit ist belegt: Trace je Tick in kanonischer
Ordnung, Permutationslauf, Differentialtest bis auf den Chip, Hashkette,
Replay. Die physische Seite — lief Tick k zur Zeit k·T₀ — misst die
Runtime (`Tick { took, drift, overrun, slept }`), aber niemand schreibt
sie je Tick mit, und der Zähler der verpassten Ticks in `TimerClock`
zählte falsch, ohne dass ein Test es sah (FB-203). Zwei Schritte
schließen das; beide sind Werkzeug und Runtime, keine Sprache.

### 6.1 Physische Zeit je Tick (Schritt 8)

**Eine Quelle, abgeleitete Zahlen.** Der Fehler in `missed` entstand,
weil die Uhr aus ihrem Zähler Ereignisse zurückrechnete — ein zweiter
Zähler neben dem, was die Schleife ohnehin je Tick weiß. Die Lösung ist,
den zweiten Zähler abzuschaffen: `drift` (Beginn des Ticks minus Frist)
und `took` (Dauer des Schritts) sind je Tick die Wahrheit, und alles
andere folgt daraus:

| Kennzahl | Definition | Bedeutung |
|---|---|---|
| verspätete Ticks | Zahl der Ticks mit `drift ≥ T₀` | Ticks, die mindestens eine Periode zu spät begannen |
| Rückstand | `max drift` | wie weit die Schleife höchstens hinterherlief |
| verlorene Perioden | `Σ max(0, drift_k − drift_{k−1}) / T₀` | Zeit, in der kein Tick begonnen werden konnte — der Zuwachs des Rückstands, nie das Aufholen |
| Überläufe | Ticks mit `took > T₀` | 7.3, unverändert |

`Overrun` in `takt-rt-core` führt die vier Zahlen; `TimerClock::missed`
entfällt. Ein Test auf dem Wirt spielt mit einer Attrappe einen
Stillstand von fünf Perioden und ein Aufholen ein und erwartet genau
fünf; auf dem Board liefert das Journal den bekannten Stillstand
(Löschung 49,9 ms bei 10 ms Tick: fünf Perioden, `58_persist_alert`).

**Feinere Uhr.** `TimerClock::now()` zählt Ticks; `took` ist damit null
oder ein Vielfaches von T₀. `TickSource` bekommt `now_ns()` aus dem
Zählerstand des Timers (SYSTIMER: 62,5 ns), für `took` und `drift`; die
logische Zeit bleibt Ticks × T₀ (7.1).

**Die Metazeile.** `t=<k> time took=<ns> drift=<ns> [slept=<n>]`, von
den nativen Runtimes geschrieben (Sink auf Linux, Telemetrie auf dem
Board), nie vom Interpreter — er hat keine physische Zeit. Sie steht
außerhalb der Hashkette (T6) und des Vergleichs (`compare` überliest
sie wie Kommentare), weil 12.5 Zeitstempel außerhalb der Semantik hält:
Der Trace bleibt bitgleich, die Zeit kommt daneben zu stehen. `takt
timing AUFZEICHNUNG` rechnet die vier Kennzahlen daraus und nennt die
spätesten Ticks mit ihrer Nummer — damit ist ein Überlauf einem Tick
zuzuordnen, nicht nur gezählt. Referenz nachziehen: 7.3 („die physische
Verzögerung wird protokolliert" bekommt die vier Begriffe), 12.5
(`time`-Zeile), `grammar/trace.md` (T1 als Metazeile, T5 am Ende des
Ticks, T6 ausgenommen).

### 6.2 Innentick-Sicht (Schritt 9)

Der Trace zeigt je Tick, was beobachtbar ist; zwischen zwei Zeilen
liegen alle Anweisungen des Ticks. Für die Fehlersuche innerhalb eines
Ticks bekommt der Interpreter `takt sim --steps DATEI`: je ausgeführter
Anweisung eine Zeile `t=<k> step <maschine> <zustand> <zeile>:<spalte>
<anweisung> [= <wert>]` — Zuweisungen mit Ergebnis, Guards mit
Wahrheitswert, Übergänge mit Ziel, Prüfungen mit Ausgang. Der
Interpreter hat die Spannen der MIR-Anweisungen; die Kosten sind eine
Zeile je Anweisung, und die Datei ist kein Trace: kein kanonisches
Format, keine Hashkette, nur für Menschen. Das ersetzt den Debugger,
den das Modell nicht braucht: Ein Lauf ist deterministisch, also ist
der Schrittmitschnitt eines Ticks so gut wie ein Haltepunkt darin.

Auf dem Chip gibt es keinen Interpreter, aber der Codegen führt unter
`statements` (11.2, 12.8) einen Programmzähler `pc` je Maschine im
Zustandsstruct: die Zeile der letzten Anweisung. Der Rahmen gibt ihn am
Tickende als Metazeile `t=<k> pc <maschine> <zeile>` aus, wenn die
Instrumentierung es erlaubt — auf `baremetal` ist der Default `states`,
also aus. Damit sagt ein Board nach einem Fault oder Überlauf, wo die
Maschine stand.

**Was beides nicht ist:** kein Haltepunkt, kein Eingriff in den Lauf,
keine Sprache. Wer im Programm eine Sonde will, hat `log` und `measure`.

## 5. Stand der Umsetzung

| # | Stand |
|---|---|
| 1 | **fertig 2026-09-17.** `probe-rs info` meldet `esp32c6` (ESP JTAG 303a:1001), Target und `clang` da; `takt build --target riscv32imac` liefert 3096 Byte ELF32-RISC-V mit `heartbeat_step` und Geschwistern. |
| 2 | **fertig 2026-09-17.** `minimal` meldet sich über USB-Serial-JTAG (COM4) hinter dem ROM- und dem ESP-IDF-Bootloader, den `probe-rs` mitbringt; das Abbild braucht den App-Deskriptor aus `esp-bootloader-esp-idf`. Stolperstein: die Index-Auflösung nahm `esp-rom-sys` 0.1.1, das `esp-hal` 1.2.1 nicht mehr übersetzt — `cargo update -p esp-rom-sys --precise 0.1.5`. |
| 3 | **fertig 2026-09-17.** `tick`: 1000 Ticks je Sekunde, nominale und gemessene Periode 1 000 000 ns, null verpasste Ticks über die Messdauer; LED blinkt sekündlich. Zwei Befunde: `counts_for` rechnete mit ganzzahligen Nanosekunden je Schritt (62 statt 62,5 bei 16 MHz, acht Promille daneben — FB-192, behoben in `takt-board-support`); der Zyklenzähler des Kerns steht in `wfi`, die gemessene Periode kommt darum aus dem SYSTIMER, der Zähler bleibt für `measure`. Ein `nomem` am `wfi` ließ die Warteschleife den Zähler nicht neu laden — entfernt. |
| 4 | **fertig 2026-09-17.** `takt` mit `29_heartbeat.takt`: Trace `t=100 out led 1`, `t=200 out led 1`, `t=300 out led 1` über USB, bitgleich mit dem Interpreter (`true` an denselben Ticks); LED blinkt im 500-ms-Takt des Programms. Blockierte einmal am veralteten `takt.exe` (FB-193). |
| 5 | **fertig 2026-09-17.** `takt-conformance/tests/board_esp32c6.rs` (nur mit `TAKT_ESP32C6_PORT=COM4`): 37 Programme des Differentialkorpus laufen auf dem Chip, je 60 Ticks, Trace über USB-Serial-JTAG, **0 Abweichungen** gegen den Interpreter — Monitore (47), `map`-Iteration (42), SHA-256 (39), Ströme und `sim`-gekoppelte Modelle (23–27, 49–55) eingeschlossen. Ausgelassen, weil der MCU-Rahmen sie nicht trägt: geplante Ausgaben (28), Systemkanäle (32, 34), Jobs (40). Dafür bekam der Rahmen Natives, Ströme, `sim`-Bindungen, Monitore, Floats/Arrays/vorzeichenlose Werte im Trace und schwache Treiber-Defaults (`takt_out_*`). Befund FB-194: Der Rahmen bemaß seine Zustandspuffer aus einer Schranke statt aus dem Struct des Codegens; `39_sha256` schrieb darüber hinaus, und die Stack-Wache des Boards fing es — auf dem Wirt blieb es unsichtbar. |
| 6 | **fertig 2026-09-17, mit einer Grenze.** Die Schleife ist jetzt `takt_rt_core::Runtime` mit `run_persisting`: das `persist`-Journal liegt in zwei Flash-Sektoren der `nvs`-Partition (`nvm.rs`, `esp-storage`), `idle` schläft als virtuelle Ticks über `TimerClock` (9.9). Belegt in `board_esp32c6.rs`: `35_persist` überlebt einen Reset (der zweite Lauf beginnt mit dem `count` des ersten, `journal: Eintrag`), `56_idle_timer` schläft rund 50 von 60 Ticks und bleibt trace-gleich mit dem Interpreter. Drei Befunde: die Uhr wartete auf das nächste Ereignis statt auf die Frist, nach virtuellen Ticks lief die Schleife der Zeit davon (FB-199, behoben); ein ISR-Zähler verliert Ticks, solange das Flash die Interrupts sperrt — die Tickzahl kommt jetzt aus dem SYSTIMER selbst (FB-198, behoben); und der Preis des Journals: `esp-storage` löscht und schreibt blockierend mit gesperrten Interrupts, ein Schreibvorgang kostet drei bis vier Ticks zu 10 ms (FB-197) — genau der Fall, den 12.3 für `xip_flash` beschreibt, und damit Schritt 7. Die Grenze: 14.7 selbst läuft noch nicht, weil seine Eingänge (AFE, Taster als `Edge`-Strom, Ladegerät als Wake-Quelle) Treiber am Board brauchen, die der MCU-Rahmen heute nicht anbietet — er stellt Ausgänge (`takt_out_*`), aber keine Eingänge. |
| 7 | **fertig 2026-09-17.** Drei Teile. (1) *RAM-Residenz:* Tick-ISR, Zaehler, Wartepfad und Schlaf tragen `#[esp_hal::ram]`; der erzeugte Takt-Code und sein C-Rahmen kommen ueber `rwtext_hook.x` nach `.rwtext` (eingeschaltet mit `ESP_HAL_CONFIG_USE_RWTEXT_LD_HOOK`). Gemessen: 4904 Byte Code und 104 Byte Konstanten wandern aus dem Flash ins RAM. (2) *Flash-Schreibzugriff im Lauf:* `esp-storage` laeuft ohne `critical-section` — es sperrte die Interrupts fuer die ganze Sektorloeschung —, und `FlashNvm` deckt die Nebenlaeufigkeit selbst mit einem Flag. (3) *`takt size` mit `iram`:* `Sections` trennt `.trap`/`.rwtext` von `.text`/`.rodata`, der Bericht hat einen eigenen Posten, und Pruefung 39 vergleicht ihn mit `iram` aus `corpus-try/hw/esp32c6.hw` (FB-201). Die Grenze, die bleibt: Eine Sektorloeschung dauert rund 25 ms und ist unteilbar, bei 10 ms Tick vergehen also Perioden — 400 statt 429 in 16 Schreibvorgaengen. Das ist Hardware, nicht Compiler (FB-197); die logische Zeit bleibt unberuehrt, weil die Tickzahl aus dem SYSTIMER kommt. Belegt mit `57_persist_often.takt` (`min_interval = 0`) in `the_journal_costs_time_but_not_semantics`, und der volle Korpus laeuft mit RAM-Residenz weiter mit **0 Abweichungen** (39 Programme). Nachtrag: Die 400 Perioden waren ein Zählfehler der Uhr (FB-203), wirklich 54; mit dem Log-Journal aus `plan/nvm.md` sind es 4. |
| 8 | **fertig 2026-09-17.** `Overrun` faltet `drift` je Tick zu vier Kennzahlen; `TimerClock::missed` entfaellt, die Uhr liefert die feine Zeit des Timers (`TickSource::now_ns`, SYSTIMER 62,5 ns, TIM2 1 µs). Metazeile `time` in `grammar/trace.md`, ausserhalb von T5, T6 und Vergleich; `takt timing TRACE --tick NS` rechnet nach. Gemessen an `58_persist_alert` auf dem Chip: 60 Zeitzeilen, vier verspaetete Ticks, vier verlorene Perioden, Rueckstand 40,6 ms, laengster Schritt 43 µs — der Journal-Schreibvorgang bei Tick 1, das Aufholen in 2 und 3 zaehlt nicht mit (FB-205). |
| 9 | **fertig 2026-09-17.** `takt sim --steps DATEI`: je ausgefuehrter Anweisung eine Zeile mit Maschine, Zeile, Spalte und geschriebenem Wert; der Interpreter sammelt sie ueber einen Haken im `Outer`-Trait, das Werkzeug rechnet die Byteversaetze in Positionen um. Der Mitschnitt aendert den Trace nicht (Test). Auf dem Chip gibt `takt_mcu_pc` den Programmzaehler je Maschine aus, gebaut mit `TAKT_INSTRUMENT=statements` (11.2; Default auf `baremetal` ist `states`): `pc device 869` und `699` im Wechsel, die zwei Zustaende des Programms (FB-206). |
