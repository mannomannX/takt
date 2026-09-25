//! Board 2, ESP32-C6: der Korpus auf dem Chip gegen den Interpreter
//! (13.8, Satz 9.4.4; plan/esp32c6.md Schritt 5).
//!
//! **Laeuft nur mit Board.** `TAKT_ESP32C6_PORT` nennt den seriellen Port
//! des USB-Serial-JTAG (`COM4`, `/dev/ttyACM0`); ohne ihn wird der Test
//! uebersprungen, wie die LLVM-Tests ohne clang. Bauen, Schreiben, Starten
//! und Lesen stehen in `takt_conformance::board`.

mod common;

use std::io::Read;
use std::time::{Duration, Instant};

use common::board::{TICKS, agreement, corpus, last_output, natives_agree, run_interpreted};
use takt_conformance::board::esp32c6::{Esp32c6, REENUMERATE_REG};
use takt_conformance::board::{self, Bin, Board, CORPUS, Options};
use takt_conformance::compare;

/// Ein Board, mehrere Tests: cargo fuehrt Tests nebenlaeufig aus, das Board
/// und sein Port vertragen nur einen Lauf zugleich.
static BOARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Das Board, exklusiv; `None` ohne `TAKT_ESP32C6_PORT`.
fn board() -> Option<(Esp32c6, std::sync::MutexGuard<'static, ()>)> {
    let Some(board) = Esp32c6::from_env() else {
        eprintln!("uebersprungen: TAKT_ESP32C6_PORT nennt kein Board");
        return None;
    };
    Some((board, BOARD.lock().unwrap_or_else(std::sync::PoisonError::into_inner)))
}

/// Ein Programm des Bring-ups, nicht des Korpus.
fn bringup_program(name: &str) -> std::path::PathBuf {
    board::root().join("crates/takt-bringup-esp32c6/programs").join(name)
}

/// Baut ein Korpusprogramm, schreibt es und liest seinen Trace.
fn run_corpus(board: &mut Esp32c6, name: &str, fresh: bool) -> String {
    let options = Options { ticks: TICKS, fresh, bin: Bin::Takt };
    board
        .build(&board::corpus_path(name), &options)
        .and_then(|elf| board.run(&elf, &options))
        .unwrap_or_else(|e| panic!("{name}: {e}"))
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
    let Some((board, _guard)) = board() else { return };
    // Zehn Ticks je Zeile, plus Rand: Der Lauf muss ueber `WANT` Zeilen
    // hinaus reichen, sonst haelt das Programm mittendrin.
    let options = Options { ticks: (WANT as u64 + 4) * 10, fresh: true, bin: Bin::Takt };
    let elf = board.build(&bringup_program("uart0_port.takt"), &options).unwrap_or_else(|e| panic!("{e}"));
    board.download(&elf).unwrap_or_else(|e| panic!("{e}"));

    let mut serial = serialport::new(&uart, 115_200)
        .timeout(Duration::from_millis(200))
        .open()
        .unwrap_or_else(|e| panic!("{uart}: {e}"));
    board.probe_rs(&["reset", "--chip", "esp32c6"]).unwrap_or_else(|e| panic!("{e}"));

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
    let first =
        lines.iter().position(|l| l.starts_with("takt ")).unwrap_or_else(|| panic!("keine `takt `-Zeile:\n{text}"));
    let whole = &lines[first..lines.len().saturating_sub(1)];
    assert!(whole.len() >= WANT, "nur {} Zeilen in 30 s:\n{text}", whole.len());

    // Ab hier ist jede Zeile eine des Treibers: Etwas anderes dazwischen
    // hiesse, dass jemand sonst auf UART0 schreibt, und das waere ein
    // Befund.
    let mut numbers = Vec::new();
    for line in whole {
        let rest = line.strip_prefix("takt ").unwrap_or_else(|| panic!("Zeile ohne `takt `: {line:?}\n{text}"));
        assert_eq!(rest.len(), 3, "Nummer nicht dreistellig: {line:?}");
        numbers.push(rest.parse::<u32>().unwrap_or_else(|e| panic!("{line:?}: {e}")));
    }

    // Luecken- und dublettenfrei, mit Ueberlauf bei 1000.
    for pair in numbers.windows(2) {
        assert_eq!(pair[1], (pair[0] + 1) % 1000, "Sprung {} -> {} in:\n{text}", pair[0], pair[1]);
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
    let Some((mut board, _guard)) = board() else { return };
    let options = Options::fresh(40);
    let text = board
        .build(&bringup_program("button_input.takt"), &options)
        .and_then(|elf| board.run(&elf, &options))
        .unwrap_or_else(|e| panic!("{e}"));

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
    let Some((mut board, _guard)) = board() else { return };
    let name = "35_persist.takt";
    let options = Options { ticks: TICKS, fresh: false, bin: Bin::Takt };
    let elf = board.build(&board::corpus_path(name), &options).unwrap_or_else(|e| panic!("{name}: {e}"));
    let first = board.run(&elf, &options).unwrap_or_else(|e| panic!("{name}: {e}"));
    let end = last_output(&first, "count").unwrap_or_else(|| panic!("kein `count` im ersten Lauf:\n{first}"));
    assert!(first.contains("flush 1"), "das Journal wurde am Ende nicht geschrieben:\n{first}");
    // Der zweite Lauf: derselbe Chip, ein Reset, das Journal bleibt.
    let second = board.capture(&elf, TICKS).unwrap_or_else(|e| panic!("{name}, zweiter Lauf: {e}"));
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
    let Some((mut board, _guard)) = board() else { return };
    let name = "56_idle_timer.takt";
    let text = run_corpus(&mut board, name, true);
    let slept = slept(&text).unwrap_or_else(|| panic!("keine Schlafzeile:\n{text}"));
    assert!(slept >= 40, "60 Ticks mit 500 ms `idle` bei 10 ms Tick: mehr als 40 virtuelle erwartet, {slept}:\n{text}");
    let diffs = compare(&run_interpreted(&corpus(name)), &text);
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
    let Some((mut board, _guard)) = board() else { return };
    let name = "58_persist_alert.takt";
    let text = run_corpus(&mut board, name, true);
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
    let diffs = compare(&run_interpreted(&corpus(name)), &text);
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
    let Some((mut board, _guard)) = board() else { return };
    let name = "59_persist_idle.takt";
    let text = run_corpus(&mut board, name, true);
    let writes = counter(&text, "journal geschrieben").unwrap_or_else(|| panic!("keine Journalzeile:\n{text}"));
    assert!(writes >= 1, "das Journal schrieb nie im Schlaf:\n{text}");
    assert_eq!(counter(&text, "verloren"), Some(0), "ein Schreibvorgang im Schlaf hat Perioden gekostet:\n{text}");
    assert_eq!(counter(&text, "ueberlaeufe"), Some(0), "{text}");
    let diffs = compare(&run_interpreted(&corpus(name)), &text);
    assert!(diffs.is_empty(), "{diffs:?}");
}

/// **Der Wunsch ueber die Konsole setzt den Chip zurueck** (FB-264):
/// `TAKT` in den Empfangspuffer, Reset ueber RTS — der Port geht und
/// kommt, und JTAG antwortet danach, auch wenn es vorher stand.
#[test]
fn the_board_resets_on_console_request() {
    let Some((board, _guard)) = board() else { return };
    board.reenumerate_via_console().unwrap_or_else(|e| panic!("{e}"));
    board.probe_rs_plain(&["read", "--chip", "esp32c6", "b32", REENUMERATE_REG, "1"]).unwrap_or_else(|e| panic!("{e}"));
}

/// **Das Board meldet sein USB-Geraet auf Wunsch neu an** (FB-266).
///
/// Der Wunsch ueber JTAG in STORE0, ein Reset: Der Port verschwindet und
/// kommt zurueck, und ein Lauf danach spricht wie zuvor. Dazu der
/// Tickzaehler ueber JTAG, der eine stumme Konsole von einem stehenden
/// Programm unterscheidet.
#[test]
fn the_board_reenumerates_its_usb_on_request() {
    let Some((mut board, _guard)) = board() else { return };
    let name = "01_minimal.takt";
    let options = Options::fresh(TICKS);
    let elf = board.build(&board::corpus_path(name), &options).unwrap_or_else(|e| panic!("{name}: {e}"));
    let first = board.run(&elf, &options).unwrap_or_else(|e| panic!("{name}: {e}"));
    assert_eq!(board.tick_over_jtag(&elf).unwrap_or_else(|e| panic!("{e}")), TICKS as u32, "{first}");
    board.reenumerate().unwrap_or_else(|e| panic!("{e}"));
    let second = board.capture_once().unwrap_or_else(|e| panic!("{name}, nach der Neuanmeldung: {e}"));
    assert!(second.contains(board::END), "nach der Neuanmeldung:\n{second}");
    let outs = |t: &str| {
        t.lines().filter(|l| l.starts_with("t=") && l.contains(" out ")).map(str::to_string).collect::<Vec<_>>()
    };
    assert_eq!(outs(&first), outs(&second));
}

#[test]
fn the_board_agrees_with_the_interpreter() {
    let Some((mut board, _guard)) = board() else { return };
    // `TAKT_ESP32C6_ONLY=42_map.takt` fuer einen einzelnen Fall.
    let only = std::env::var("TAKT_ESP32C6_ONLY").ok();
    let failed = agreement(&mut board, CORPUS, only.as_deref());
    assert!(failed.is_empty(), "{}", failed.join("\n\n"));
}

#[test]
fn the_natives_agree_with_the_host() {
    let Some((mut board, _guard)) = board() else { return };
    let failed = natives_agree(&mut board);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}
