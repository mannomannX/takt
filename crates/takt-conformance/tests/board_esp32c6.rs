//! Board 2, ESP32-C6: der Korpus auf dem Chip gegen den Interpreter
//! (13.8, Satz 9.4.4; plan/esp32c6.md Schritt 5).
//!
//! **Laeuft nur mit Board.** `TAKT_ESP32C6_PORT` nennt den seriellen Port
//! des USB-Serial-JTAG (`COM4`, `/dev/ttyACM0`); ohne ihn wird der Test
//! uebersprungen, wie die LLVM-Tests ohne clang. Je Programm: das
//! Bring-up-Binary mit `TAKT_PROGRAM` und `TAKT_TICKS` bauen, mit
//! `probe-rs` flashen, zuruecksetzen, den Trace bis `takt end` lesen und
//! wie im Linux-Vergleich pruefen.

use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use takt_conformance::compare;
use takt_mir::program::Program;

const TICKS: u64 = 60;

/// Ein Board, drei Tests: cargo fuehrt Tests nebenlaeufig aus, das Board
/// und sein Port vertragen nur einen Lauf zugleich.
static BOARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn board() -> std::sync::MutexGuard<'static, ()> {
    BOARD.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Der Differentialkorpus ohne das, was der MCU-Rahmen nicht traegt:
/// Systemkanaele (32, 34), geplante Ausgaben (28) und Jobs (40).
const KORPUS: &[&str] = &[
    "01_minimal.takt",
    "02_units_and_data.takt",
    "03_sequences_and_faults.takt",
    "12_bitfields.takt",
    "13_framing.takt",
    "13_protocol_analysis.takt",
    "14_latency.takt",
    "15_quality.takt",
    "16_timing.takt",
    "17_nested.takt",
    "18_blocks.takt",
    "19_faults.takt",
    "20_native.takt",
    "21_fault_targets.takt",
    "22_faulted_outputs.takt",
    "23_patterns.takt",
    "24_send_has.takt",
    "25_format.takt",
    "26_samples.takt",
    "27_every.takt",
    "33_enum_param.takt",
    "35_persist.takt",
    "36_int_units.takt",
    "37_follows.takt",
    "39_sha256.takt",
    "41_tunables.takt",
    "42_map.takt",
    "43_sent.takt",
    "46_matrices.takt",
    "47_monitors.takt",
    "49_record_streams.takt",
    "50_clause_words.takt",
    "51_text_into_bytes.takt",
    "52_padding_fields.takt",
    "53_stream_kinds.takt",
    "54_inout.takt",
    "55_frames_with_bytes.takt",
    "56_idle_timer.takt",
    "57_persist_often.takt",
    "58_persist_alert.takt",
    "59_persist_idle.takt",
    "60_resume.takt",
    "62_type_generics.takt",
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn corpus(name: &str) -> Program {
    let path = root().join("corpus-try").join(name);
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let options =
        takt_sema::Options { policy: takt_diag::Policy::default(), build: takt_sema::Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{name}:\n{}", errors.join("\n"));
    out.program.unwrap_or_else(|| panic!("{name}: kein Programm"))
}

fn run_interpreted(p: &Program) -> String {
    let options = takt_interp::RunOptions { ticks: TICKS, ..Default::default() };
    match takt_interp::run(p, &takt_interp::Trace::default(), &options) {
        Ok(r) => r.trace.render(),
        Err(e) => panic!("Interpreter: {e:?}"),
    }
}

/// Baut das Bring-up-Binary fuer `name` und liefert den Pfad des ELF;
/// `fresh` loescht das Journal vor dem Lauf (wie der Interpreter ohne
/// Speicher), sonst laedt der Lauf, was der vorige schrieb.
fn build(name: &str, fresh: bool) -> Result<PathBuf, String> {
    build_program(&root().join("corpus-try").join(name), fresh, TICKS)
}

/// Wie [`build`], aber mit vollem Pfad und eigener Tickzahl: Nicht jedes
/// Programm des Bring-ups liegt im Korpus, und nicht jedes ist nach den
/// 60 Ticks des Konformitaetslaufs fertig.
fn build_program(program: &Path, fresh: bool, ticks: u64) -> Result<PathBuf, String> {
    let manifest = root().join("crates/takt-bringup-esp32c6/Cargo.toml");
    let mut cargo = Command::new("cargo");
    cargo
        .args(["build", "--release", "--target", "riscv32imac-unknown-none-elf", "--bin", "takt"])
        .arg("--message-format=json-render-diagnostics")
        .arg("--manifest-path")
        .arg(&manifest)
        .env("TAKT_PROGRAM", program)
        .env("TAKT_TICKS", ticks.to_string())
        // 12.3: RAM-Residenz des Takt-Programms. `.cargo/config.toml` des
        // Bring-ups greift hier nicht, weil `--manifest-path` von aussen baut.
        .env("ESP_HAL_CONFIG_USE_RWTEXT_LD_HOOK", "true");
    if fresh {
        cargo.env("TAKT_FRESH_JOURNAL", "1");
    } else {
        cargo.env_remove("TAKT_FRESH_JOURNAL");
    }
    let out = cargo.output().map_err(|e| format!("cargo: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    // Die JSON-Meldungen nennen das Binary; ein eigener Parser fuer eine
    // Zeichenkette in einer Zeile ist kuerzer als eine Abhaengigkeit.
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let rest = &l[l.find("\"executable\":\"")? + "\"executable\":\"".len()..];
            Some(rest[..rest.find('"')?].replace("\\\\", "\\"))
        })
        .next_back()
        .map(PathBuf::from)
        .ok_or_else(|| "cargo meldete kein Binary".to_string())
}

fn probe_rs(args: &[&str]) -> Result<(), String> {
    let out = Command::new("probe-rs").args(args).output().map_err(|e| format!("probe-rs: {e}"))?;
    if out.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&out.stderr).into_owned()) }
}

/// Setzt das Board zurueck und liest den Trace bis `takt end`.
///
/// Kommt gar nichts, nicht einmal der ROM-Bootloader, hat der
/// USB-Serial-JTAG nach dem Flashen noch nicht wieder angelegt; ein
/// zweiter Reset genuegt dann. Ein Lauf, der etwas sagt und nicht endet,
/// wird nicht wiederholt — das waere ein Befund.
fn capture(port: &str) -> Result<String, String> {
    for attempt in 0..2 {
        // Nach dem Flashen legt der USB-Serial-JTAG neu an; ein Handle von
        // davor liefert nichts. Darum kurz warten und je Versuch neu oeffnen.
        std::thread::sleep(Duration::from_millis(500));
        let mut serial = serialport::new(port, 115_200)
            .timeout(Duration::from_millis(200))
            .open()
            .map_err(|e| format!("{port}: {e}"))?;
        probe_rs(&["reset", "--chip", "esp32c6"])?;
        let start = Instant::now();
        let mut text = String::new();
        let mut buf = [0u8; 4096];
        while start.elapsed() < Duration::from_secs(30) {
            match serial.read(&mut buf) {
                Ok(n) => text.push_str(&String::from_utf8_lossy(&buf[..n])),
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
                Err(e) => return Err(format!("{port}: {e}")),
            }
            if text.contains("takt end") {
                return Ok(text);
            }
        }
        if !text.is_empty() || attempt == 1 {
            return Err(format!("kein `takt end` binnen 30 s; gelesen:\n{text}"));
        }
    }
    unreachable!("zwei Versuche")
}

/// Die Namen der Ausgaenge, die ein Trace nennt.
fn output_names(text: &str) -> BTreeSet<String> {
    text.lines()
        .filter_map(|l| {
            let mut w = l.split_whitespace();
            w.next()?.strip_prefix("t=")?;
            (w.next()? == "out").then(|| w.next().map(String::from))?
        })
        .collect()
}

/// Der letzte `out`-Wert eines Ausgangs im Trace.
fn last_output(text: &str, name: &str) -> Option<String> {
    text.lines()
        .filter_map(|l| {
            let mut w = l.split_whitespace();
            w.next()?.strip_prefix("t=")?;
            (w.next()? == "out" && w.next()? == name).then(|| w.collect::<Vec<_>>().join(" "))
        })
        .next_back()
}

/// Die Zahl hinter einem Wort der Abschlusszeile (`takt schlief 0
/// ueberlaeufe 0 journal geschrieben 3 …`).
fn counter(text: &str, label: &str) -> Option<u64> {
    let line = text.lines().find(|l| l.starts_with("takt schlief "))?;
    let rest = line.split_once(label)?.1;
    rest.split_whitespace().next()?.parse().ok()
}

/// Die Zahl hinter `takt schlief`.
fn slept(text: &str) -> Option<u64> {
    text.lines().find_map(|l| l.strip_prefix("takt schlief ")?.split_whitespace().next()?.parse().ok())
}

/// **Eine `driver machine` schreibt UART0** (12.10, M8 Schritt 20).
///
/// Der Nachweis der Treiberstufe auf echten Registern: Zwei Ports, ein
/// Statuslesen an `0x6000_001C`, zehn Bytes an `0x6000_0000`. Bis hierher
/// war `port` gegen ein Geraetemodell belegt; hier schreibt er in
/// Silizium, und die Bytes erscheinen am UART-Anschluss des Boards.
///
/// **Zwei Ports, zwei Kabel.** `TAKT_ESP32C6_PORT` ist der USB-Serial-JTAG
/// zum Flashen, `TAKT_ESP32C6_UART_PORT` die CP210x-Bruecke an UART0. Ohne
/// den zweiten ist der Test uebersprungen.
///
/// Was der Test prueft, ist nicht „es kam etwas": Jede Zeile traegt eine
/// dreistellige Nummer, und die Folge muss luecken- und dublettenfrei
/// aufsteigen. Ein uebersprungenes Byte zerrisse eine Zeile, ein
/// FIFO-Ueberlauf verschluckte eine Nummer, und eine umsortierte
/// Schreibfolge vertauschte die Ziffern — jeder dieser Faelle faellt hier
/// auf.
#[test]
fn a_driver_machine_writes_uart0_registers() {
    let Ok(uart) = std::env::var("TAKT_ESP32C6_UART_PORT") else {
        eprintln!("uebersprungen: TAKT_ESP32C6_UART_PORT nennt keinen UART-Anschluss");
        return;
    };
    let _guard = board();
    let program = root().join("crates/takt-bringup-esp32c6/programs/uart0_port.takt");
    // Zehn Ticks je Zeile, plus Rand: Der Lauf muss ueber `WANT` Zeilen
    // hinaus reichen, sonst haelt das Programm mittendrin.
    let elf = build_program(&program, true, (WANT as u64 + 4) * 10).unwrap_or_else(|e| panic!("{e}"));
    probe_rs(&["download", "--chip", "esp32c6", &elf.to_string_lossy()]).unwrap_or_else(|e| panic!("{e}"));

    let mut serial = serialport::new(&uart, 115_200)
        .timeout(Duration::from_millis(200))
        .open()
        .unwrap_or_else(|e| panic!("{uart}: {e}"));
    probe_rs(&["reset", "--chip", "esp32c6"]).unwrap_or_else(|e| panic!("{e}"));

    // Der Treiber schreibt je 100 ms eine Zeile, also je zehn Ticks eine.
    // 40 Zeilen sind gut vier Sekunden; die Frist laesst Raum fuer den
    // Start des Boards.
    const WANT: usize = 40;
    let start = Instant::now();
    let mut text = String::new();
    let mut buf = [0u8; 4096];
    while start.elapsed() < Duration::from_secs(30) {
        match serial.read(&mut buf) {
            Ok(n) => text.push_str(&String::from_utf8_lossy(&buf[..n])),
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => panic!("{uart}: {e}"),
        }
        if text.lines().filter(|l| l.starts_with("takt ")).count() > WANT + 1 {
            break;
        }
    }

    // UART0 ist die Bootkonsole des Chips: ROM-Bootloader und zweite
    // Stufe schreiben darauf, bevor das Takt-Programm laeuft. Gezaehlt
    // wird darum ab der ersten `takt `-Zeile; die letzte bleibt aussen
    // vor, weil der Mitschnitt mitten in ihr endet.
    let lines: Vec<&str> = text.lines().collect();
    let first = lines.iter().position(|l| l.starts_with("takt ")).unwrap_or_else(|| {
        panic!(
            "keine `takt `-Zeile:
{text}"
        )
    });
    let whole = &lines[first..lines.len().saturating_sub(1)];
    assert!(
        whole.len() >= WANT,
        "nur {} Zeilen in 30 s:
{text}",
        whole.len()
    );

    // Ab hier ist jede Zeile eine des Treibers: Etwas anderes dazwischen
    // hiesse, dass jemand sonst auf UART0 schreibt, und das waere ein
    // Befund.
    let mut numbers = Vec::new();
    for line in whole {
        let rest = line.strip_prefix("takt ").unwrap_or_else(|| {
            panic!(
                "Zeile ohne `takt `: {line:?}
{text}"
            )
        });
        assert_eq!(rest.len(), 3, "Nummer nicht dreistellig: {line:?}");
        numbers.push(rest.parse::<u32>().unwrap_or_else(|e| panic!("{line:?}: {e}")));
    }

    // Luecken- und dublettenfrei, mit Ueberlauf bei 1000.
    for pair in numbers.windows(2) {
        let want = (pair[0] + 1) % 1000;
        assert_eq!(
            pair[1], want,
            "Sprung {} -> {} in:
{text}",
            pair[0], pair[1]
        );
    }
}

/// **Ein Eingang vom Board erreicht das Prozessabbild** (12.1 Schritt 2).
///
/// Bis hierher stellte der MCU-Rahmen nur Ausgaenge; ein `hw`-Eingang
/// blieb `Bad`, weil es keinen Weg gab, ihn zu setzen. Jetzt gibt es
/// `takt_in_*` als Gegenstueck zu `takt_out_*`, und dieser Test zeigt,
/// dass der Weg traegt: Der Taster an IO9 geht ueber den Treiber in das
/// Abbild, und das Programm liest ihn.
///
/// **Ohne Finger am Board** prueft der Test, was ohne Druck gilt: Der
/// Eingang ist `Good` und `false` — nicht `Bad`. Ein `Bad` hiesse, dass
/// der Treiber nicht gerufen wurde, und genau das war der Zustand vorher.
#[test]
fn a_board_input_reaches_the_process_image() {
    let Ok(port) = std::env::var("TAKT_ESP32C6_PORT") else {
        eprintln!("uebersprungen: TAKT_ESP32C6_PORT nennt kein Board");
        return;
    };
    let _guard = board();
    let program = root().join("crates/takt-bringup-esp32c6/programs/button_input.takt");
    let elf = build_program(&program, true, 40).unwrap_or_else(|e| panic!("{e}"));
    probe_rs(&["download", "--chip", "esp32c6", &elf.to_string_lossy()]).unwrap_or_else(|e| panic!("{e}"));
    let text = capture(&port).unwrap_or_else(|e| panic!("{e}"));

    // Der Taster ist ungedrueckt: `led` bleibt aus, `pressed` bei null.
    assert_eq!(last_output(&text, "led").as_deref(), Some("0"), "{text}");
    assert_eq!(last_output(&text, "pressed").as_deref(), Some("0"), "{text}");

    // Und das ist die Aussage: Waere der Treiber nicht gerufen worden,
    // bliebe der Eintrag `Bad` — dann faultete `btn.or(false)` nicht,
    // aber `shaky` zaehlte jeden Tick, weil `Bad` auch `suspect` ist.
    assert_eq!(last_output(&text, "shaky").as_deref(), Some("0"), "{text}");
}

/// **`persist` ueberlebt einen Reset** (5.9, 12.3; plan/esp32c6.md 6).
///
/// `35_persist` zaehlt `cycles` hoch; der Lauf schreibt das Journal am
/// Ende (`flush`). Ein zweiter Lauf desselben Abbilds nach einem Reset —
/// ohne frisches Journal — muss mit dem letzten Stand des ersten beginnen.
#[test]
fn persistence_survives_a_reset() {
    let Ok(port) = std::env::var("TAKT_ESP32C6_PORT") else {
        eprintln!("uebersprungen: TAKT_ESP32C6_PORT nennt kein Board");
        return;
    };
    let _board = board();
    let name = "35_persist.takt";
    let first = build(name, false)
        .and_then(|elf| {
            probe_rs(&["download", "--chip", "esp32c6", &elf.to_string_lossy()])?;
            capture(&port)
        })
        .unwrap_or_else(|e| panic!("{name}: {e}"));
    let end = last_output(&first, "count").unwrap_or_else(|| panic!("kein `count` im ersten Lauf:\n{first}"));
    assert!(first.contains("flush 1"), "das Journal wurde am Ende nicht geschrieben:\n{first}");
    // Der zweite Lauf: derselbe Chip, ein Reset, das Journal bleibt.
    let second = capture(&port).unwrap_or_else(|e| panic!("{name}, zweiter Lauf: {e}"));
    let start = second
        .lines()
        .find_map(|l| l.strip_prefix("t=0 out count "))
        .map(str::trim)
        .unwrap_or_else(|| panic!("kein `count` bei t=0 im zweiten Lauf:\n{second}"));
    assert_eq!(start, end.trim(), "der zweite Lauf beginnt nicht mit dem Stand des ersten:\n{second}");
    assert!(second.contains("journal: Eintrag"), "das Journal wurde nicht geladen:\n{second}");
}

/// **`idle` schlaeft** (9.9, Satz 9.9.1; plan/esp32c6.md 6):
/// `56_idle_timer` verbringt 500 von 700 ms im Schlaf, mit dem Timer als
/// einziger Wake-Quelle; die Schleife zaehlt die Ticks dazwischen als
/// virtuelle, und der Trace bleibt der des Interpreters.
#[test]
fn an_idle_state_sleeps_in_virtual_ticks() {
    let Ok(port) = std::env::var("TAKT_ESP32C6_PORT") else {
        eprintln!("uebersprungen: TAKT_ESP32C6_PORT nennt kein Board");
        return;
    };
    let _board = board();
    let name = "56_idle_timer.takt";
    let text = build(name, true)
        .and_then(|elf| {
            probe_rs(&["download", "--chip", "esp32c6", &elf.to_string_lossy()])?;
            capture(&port)
        })
        .unwrap_or_else(|e| panic!("{name}: {e}"));
    let slept = slept(&text).unwrap_or_else(|| panic!("keine Schlafzeile:\n{text}"));
    assert!(slept >= 40, "60 Ticks mit 500 ms `idle` bei 10 ms Tick: mehr als 40 virtuelle erwartet, {slept}:\n{text}");
    let p = corpus(name);
    let diffs = compare(&run_interpreted(&p), &text);
    assert!(diffs.is_empty(), "{diffs:?}");
}

/// **Unter `overrun = alert` kostet das Journal Zeit, aber keine
/// Semantik** (7.3, 12.3; plan/nvm.md 2.2, FB-197).
///
/// `58_persist_alert` schreibt in jedem Tick (`min_interval = 0`) und
/// nimmt die Ueberlaeufe an. Ein Schreibvorgang haelt den Kern
/// zweistellige Perioden lang an; die logische Zeit beruehrt das nicht —
/// der Trace bleibt der des Interpreters (Satz 9.4.4), und die Tickzahl
/// kommt aus dem SYSTIMER (FB-198). Der Test zaehlt die Perioden und
/// druckt die gemessenen Lösch- und Programmierzeiten: die Zahlen fuer
/// `nvm_erase_ns` und `nvm_program_ns` in `corpus-try/hw/esp32c6.hw`.
#[test]
fn the_journal_costs_time_but_not_semantics() {
    let Ok(port) = std::env::var("TAKT_ESP32C6_PORT") else {
        eprintln!("uebersprungen: TAKT_ESP32C6_PORT nennt kein Board");
        return;
    };
    let _board = board();
    let name = "58_persist_alert.takt";
    let text = build(name, true)
        .and_then(|elf| {
            probe_rs(&["download", "--chip", "esp32c6", &elf.to_string_lossy()])?;
            capture(&port)
        })
        .unwrap_or_else(|e| panic!("{name}: {e}"));
    let writes = counter(&text, "journal geschrieben").unwrap_or_else(|| panic!("keine Journalzeile:\n{text}"));
    assert!(writes > 1, "das Journal schrieb nur {writes}-mal; der Fall aus 12.3 trat nicht ein:\n{text}");
    assert_eq!(counter(&text, "fehlgeschlagen"), Some(0), "ein Schreibvorgang scheiterte:\n{text}");
    eprintln!(
        "{name}: {writes} Journal-Schreibvorgaenge, {} verlorene Perioden, Rueckstand {} ns; \
         loeschen {} ns, programmieren {} ns",
        counter(&text, "verloren").unwrap_or(0),
        counter(&text, "rueckstand").unwrap_or(0),
        counter(&text, "nvm loeschen").unwrap_or(0),
        counter(&text, "programmieren").unwrap_or(0)
    );
    let times = text.lines().filter(|l| l.contains(" time took=")).count();
    assert!(times as u64 >= TICKS, "die Zeitzeilen fehlen ({times} von {TICKS}):\n{text}");
    let p = corpus(name);
    let diffs = compare(&run_interpreted(&p), &text);
    assert!(diffs.is_empty(), "{diffs:?}");
}

/// **Unter `overrun = fault` schreibt das Journal im Schlaf** (9.9, 12.3;
/// plan/nvm.md 2.3): `59_persist_idle` gibt der Runtime je Runde 500 ms
/// `idle`, mehr als ein Schreibvorgang braucht. Sie schreibt dort, und
/// die Schleife verpasst keine Periode; der Trace ist der ohne Schlaf
/// (Satz 9.9.1). Braucht `nvm_blocking` und die Zeiten in
/// `corpus-try/hw/esp32c6.hw`, sonst gilt das Geraet als asynchron.
#[test]
fn the_journal_writes_in_sleep_windows() {
    let Ok(port) = std::env::var("TAKT_ESP32C6_PORT") else {
        eprintln!("uebersprungen: TAKT_ESP32C6_PORT nennt kein Board");
        return;
    };
    let _board = board();
    let name = "59_persist_idle.takt";
    let text = build(name, true)
        .and_then(|elf| {
            probe_rs(&["download", "--chip", "esp32c6", &elf.to_string_lossy()])?;
            capture(&port)
        })
        .unwrap_or_else(|e| panic!("{name}: {e}"));
    let writes = counter(&text, "journal geschrieben").unwrap_or_else(|| panic!("keine Journalzeile:\n{text}"));
    assert!(writes >= 1, "das Journal schrieb nie im Schlaf:\n{text}");
    assert_eq!(counter(&text, "verloren"), Some(0), "ein Schreibvorgang im Schlaf hat Perioden gekostet:\n{text}");
    assert_eq!(counter(&text, "ueberlaeufe"), Some(0), "{text}");
    let p = corpus(name);
    let diffs = compare(&run_interpreted(&p), &text);
    assert!(diffs.is_empty(), "{diffs:?}");
}

#[test]
fn the_board_agrees_with_the_interpreter() {
    let Ok(port) = std::env::var("TAKT_ESP32C6_PORT") else {
        eprintln!("uebersprungen: TAKT_ESP32C6_PORT nennt kein Board");
        return;
    };
    let _board = board();
    // `TAKT_ESP32C6_ONLY=42_map.takt` fuer einen einzelnen Fall.
    let only = std::env::var("TAKT_ESP32C6_ONLY").ok();
    let mut failed = Vec::new();
    for name in KORPUS.iter().filter(|n| only.as_deref().is_none_or(|o| o == **n)) {
        let p = corpus(name);
        let board = match build(name, true).and_then(|elf| {
            probe_rs(&["download", "--chip", "esp32c6", &elf.to_string_lossy()])?;
            capture(&port)
        }) {
            Ok(t) => t,
            Err(e) => {
                failed.push(format!("{name}: kein Lauf auf dem Board:\n{e}"));
                continue;
            }
        };
        let interpreted = run_interpreted(&p);
        let missing: Vec<String> = output_names(&interpreted).difference(&output_names(&board)).cloned().collect();
        let diffs = compare(&interpreted, &board);
        if !missing.is_empty() || !diffs.is_empty() {
            let list: Vec<String> = diffs.iter().take(8).map(|d| format!("  {d}")).collect();
            failed.push(format!(
                "{name}: {} Abweichungen, fehlende Ausgaenge {missing:?}\n{}\n--- Interpreter ---\n{}\n--- Board ---\n{}",
                diffs.len(),
                list.join("\n"),
                interpreted.lines().take(12).collect::<Vec<_>>().join("\n"),
                board.lines().filter(|l| l.starts_with("t=")).take(12).collect::<Vec<_>>().join("\n")
            ));
        }
        eprintln!("{name}: {} Abweichungen", diffs.len());
    }
    assert!(failed.is_empty(), "{}", failed.join("\n\n"));
}
