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
- Der Tick verliert Perioden, während das Journal einen Sektor löscht
  (rund 25 ms, unteilbar). Schritt 7 hat alles entfernt, was darüber
  hinaus blockierte; was bleibt, ist die Löschzeit selbst. Wer sie nicht
  verlieren darf, wählt `min_interval` groß genug — oder ein Ziel mit
  getrenntem Programm- und Datenflash.
- Ob der Trace-Rückkanal über USB-Serial-JTAG schnell genug ist, um jede
  Zeile mitzuschreiben, oder ob der Korpuslauf aus dem RAM-Log
  nachgelagert liest (M5 4, Punkt 1). Schritt 4 schreibt alle 100 Ticks
  ohne verpasste Ticks; jede Zeile mitzuschreiben ist noch nicht gemessen.
- Der MCU-Rahmen trägt keine geplanten Ausgaben (`at`, 7.5), keine
  Systemkanäle (`sys/reboot`, `sys/jump`) und keine Jobs; die vier Programme
  bleiben dem Linux-Vergleich vorbehalten, bis der Rahmen sie hat.
- Eingänge vom Board: Der MCU-Rahmen hat `takt_out_*` für Ausgänge, aber
  keinen Weg, einen `hw`-Eingang mit Wert und Qualität zu stellen (12.1,
  12.6). Ohne ihn bleiben 14.7 und jeder echte Treiber am Board Simulation.
- Watchdog: `esp-hal` hält RWDT und MWDT beim Start an; die Schleife läuft
  mit einem leeren `Watchdog` (12.3 verlangt einen echten).
- `build.rs` nimmt das `takt`-Werkzeug aus dem Release-Verzeichnis, auch
  wenn es älter ist als der Compiler (FB-193): Erster Bau des Servo-Objekts
  kam aus einem Stand vor M6 und ließ die Maschinenfunktionen fehlen. Bis
  das Skript den Stand prüft: `cargo build -p takt-cli --release` vor dem
  Bring-up.

## 5. Stand der Umsetzung

| # | Stand |
|---|---|
| 1 | **fertig 2026-09-17.** `probe-rs info` meldet `esp32c6` (ESP JTAG 303a:1001), Target und `clang` da; `takt build --target riscv32imac` liefert 3096 Byte ELF32-RISC-V mit `heartbeat_step` und Geschwistern. |
| 2 | **fertig 2026-09-17.** `minimal` meldet sich über USB-Serial-JTAG (COM4) hinter dem ROM- und dem ESP-IDF-Bootloader, den `probe-rs` mitbringt; das Abbild braucht den App-Deskriptor aus `esp-bootloader-esp-idf`. Stolperstein: die Index-Auflösung nahm `esp-rom-sys` 0.1.1, das `esp-hal` 1.2.1 nicht mehr übersetzt — `cargo update -p esp-rom-sys --precise 0.1.5`. |
| 3 | **fertig 2026-09-17.** `tick`: 1000 Ticks je Sekunde, nominale und gemessene Periode 1 000 000 ns, null verpasste Ticks über die Messdauer; LED blinkt sekündlich. Zwei Befunde: `counts_for` rechnete mit ganzzahligen Nanosekunden je Schritt (62 statt 62,5 bei 16 MHz, acht Promille daneben — FB-192, behoben in `takt-board-support`); der Zyklenzähler des Kerns steht in `wfi`, die gemessene Periode kommt darum aus dem SYSTIMER, der Zähler bleibt für `measure`. Ein `nomem` am `wfi` ließ die Warteschleife den Zähler nicht neu laden — entfernt. |
| 4 | **fertig 2026-09-17.** `takt` mit `29_heartbeat.takt`: Trace `t=100 out led 1`, `t=200 out led 1`, `t=300 out led 1` über USB, bitgleich mit dem Interpreter (`true` an denselben Ticks); LED blinkt im 500-ms-Takt des Programms. Blockierte einmal am veralteten `takt.exe` (FB-193). |
| 5 | **fertig 2026-09-17.** `takt-conformance/tests/board_esp32c6.rs` (nur mit `TAKT_ESP32C6_PORT=COM4`): 37 Programme des Differentialkorpus laufen auf dem Chip, je 60 Ticks, Trace über USB-Serial-JTAG, **0 Abweichungen** gegen den Interpreter — Monitore (47), `map`-Iteration (42), SHA-256 (39), Ströme und `sim`-gekoppelte Modelle (23–27, 49–55) eingeschlossen. Ausgelassen, weil der MCU-Rahmen sie nicht trägt: geplante Ausgaben (28), Systemkanäle (32, 34), Jobs (40). Dafür bekam der Rahmen Natives, Ströme, `sim`-Bindungen, Monitore, Floats/Arrays/vorzeichenlose Werte im Trace und schwache Treiber-Defaults (`takt_out_*`). Befund FB-194: Der Rahmen bemaß seine Zustandspuffer aus einer Schranke statt aus dem Struct des Codegens; `39_sha256` schrieb darüber hinaus, und die Stack-Wache des Boards fing es — auf dem Wirt blieb es unsichtbar. |
| 6 | **fertig 2026-09-17, mit einer Grenze.** Die Schleife ist jetzt `takt_rt_core::Runtime` mit `run_persisting`: das `persist`-Journal liegt in zwei Flash-Sektoren der `nvs`-Partition (`nvm.rs`, `esp-storage`), `idle` schläft als virtuelle Ticks über `TimerClock` (9.9). Belegt in `board_esp32c6.rs`: `35_persist` überlebt einen Reset (der zweite Lauf beginnt mit dem `count` des ersten, `journal: Eintrag`), `56_idle_timer` schläft rund 50 von 60 Ticks und bleibt trace-gleich mit dem Interpreter. Drei Befunde: die Uhr wartete auf das nächste Ereignis statt auf die Frist, nach virtuellen Ticks lief die Schleife der Zeit davon (FB-199, behoben); ein ISR-Zähler verliert Ticks, solange das Flash die Interrupts sperrt — die Tickzahl kommt jetzt aus dem SYSTIMER selbst (FB-198, behoben); und der Preis des Journals: `esp-storage` löscht und schreibt blockierend mit gesperrten Interrupts, ein Schreibvorgang kostet drei bis vier Ticks zu 10 ms (FB-197) — genau der Fall, den 12.3 für `xip_flash` beschreibt, und damit Schritt 7. Die Grenze: 14.7 selbst läuft noch nicht, weil seine Eingänge (AFE, Taster als `Edge`-Strom, Ladegerät als Wake-Quelle) Treiber am Board brauchen, die der MCU-Rahmen heute nicht anbietet — er stellt Ausgänge (`takt_out_*`), aber keine Eingänge. |
| 7 | **fertig 2026-09-17.** Drei Teile. (1) *RAM-Residenz:* Tick-ISR, Zaehler, Wartepfad und Schlaf tragen `#[esp_hal::ram]`; der erzeugte Takt-Code und sein C-Rahmen kommen ueber `rwtext_hook.x` nach `.rwtext` (eingeschaltet mit `ESP_HAL_CONFIG_USE_RWTEXT_LD_HOOK`). Gemessen: 4904 Byte Code und 104 Byte Konstanten wandern aus dem Flash ins RAM. (2) *Flash-Schreibzugriff im Lauf:* `esp-storage` laeuft ohne `critical-section` — es sperrte die Interrupts fuer die ganze Sektorloeschung —, und `FlashNvm` deckt die Nebenlaeufigkeit selbst mit einem Flag. (3) *`takt size` mit `iram`:* `Sections` trennt `.trap`/`.rwtext` von `.text`/`.rodata`, der Bericht hat einen eigenen Posten, und Pruefung 39 vergleicht ihn mit `iram` aus `corpus-try/hw/esp32c6.hw` (FB-201). Die Grenze, die bleibt: Eine Sektorloeschung dauert rund 25 ms und ist unteilbar, bei 10 ms Tick vergehen also Perioden — 400 statt 429 in 16 Schreibvorgaengen. Das ist Hardware, nicht Compiler (FB-197); die logische Zeit bleibt unberuehrt, weil die Tickzahl aus dem SYSTIMER kommt. Belegt mit `57_persist_often.takt` (`min_interval = 0`) in `the_journal_costs_time_but_not_semantics`, und der volle Korpus laeuft mit RAM-Residenz weiter mit **0 Abweichungen** (39 Programme). |
