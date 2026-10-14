//! Board 2, ESP32-C6: der Korpus auf dem Chip gegen den Interpreter
//! (13.8, Satz 9.4.4; plan/esp32c6.md Schritt 5).
//!
//! **Laeuft nur mit Board.** `TAKT_ESP32C6_PORT` nennt den seriellen Port
//! des USB-Serial-JTAG (`COM4`, `/dev/ttyACM0`); ohne ihn wird der Test
//! uebersprungen, wie die LLVM-Tests ohne clang. Bauen, Schreiben, Starten
//! und Lesen stehen in `takt_conformance::board`.
//!
//! **Oder baut nur.** Mit `TAKT_BOARD_BUILD_ONLY=1` baut jeder Test die
//! Abbilder, die er braucht, und endet vor dem Schreiben bestanden (M11
//! Schritt 34d, `.github/workflows/boards.yml`).

mod common;

use std::io::Read;
use std::time::{Duration, Instant};

use common::board::{
    TICKS, agreement, agreement_with, corpus, driver_edge_agrees, journal_costs_time_but_not_semantics,
    journal_writes_in_sleep_windows, last_output, long_job_keeps_the_tick, natives_agree,
    overrun_reaches_every_machine, overrun_reaches_every_machine_with, persistence_survives, ran, replayed,
    runs_shared, simultaneous_jobs_finish_on_time, the_form_keeps_its_deadline, the_port_passes, verdict, zephyr_image,
    zephyr_persistence_survives, zephyr_verdict,
};
use takt_conformance::board::esp32c6::{Esp32c6, REENUMERATE_REG};
use takt_conformance::board::{self, Board, Form, Options};
use takt_conformance::compare;
use takt_conformance::esp_idf::Esp32c6Idf;

/// Ein Board, mehrere Tests: cargo fuehrt Tests nebenlaeufig aus, das Board
/// und sein Port vertragen nur einen Lauf zugleich.
static BOARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Das Board, exklusiv. Die Tests laufen nur mit `--ignored`; wer sie
/// verlangt, verlangt das Board, und ohne `TAKT_ESP32C6_PORT` scheitern sie.
/// Im Lauf, der nur baut, ist es eines ohne Port.
fn board() -> Option<(Esp32c6, std::sync::MutexGuard<'static, ()>)> {
    let board = Esp32c6::from_env().expect("TAKT_ESP32C6_PORT nennt kein Board");
    Some((board, BOARD.lock().unwrap_or_else(std::sync::PoisonError::into_inner)))
}

/// Ein Programm des Bring-ups, nicht des Korpus.
fn bringup_program(name: &str) -> std::path::PathBuf {
    board::root().join("crates/takt-bringup-esp32c6/programs").join(name)
}

/// Baut ein Korpusprogramm, schreibt es und liest seinen Trace in Echtzeit;
/// `None` im Lauf, der nur baut.
fn run_corpus(board: &mut Esp32c6, name: &str, fresh: bool) -> Option<String> {
    ran(common::board::run_corpus(board, name, fresh).map_err(|e| format!("{name}: {e}")))
}

/// Die Zahl hinter `takt schlief`.
fn slept(text: &str) -> Option<u64> {
    text.lines().find_map(|l| l.strip_prefix("takt schlief ")?.split_whitespace().next()?.parse().ok())
}

/// **`next_run = AFTER(delay)` setzt nach der Dauer fort, und der neue Lauf
/// weiss es** (12.7, FB-309): Der erste Lauf endet nach 300 ms mit
/// `AFTER(delay = 2 s)`, das Board schlaeft ohne RAM, der zweite Lauf
/// beginnt mit `previous_run = ENDED` und zeigt es an `woke`.
///
/// Im Tiefschlaf ist der USB-Serial-JTAG aus: Dass der Port verschwindet
/// und erst nach der Dauer zurueckkommt, belegt den Schlaf selbst.
/// `previous_run` liest der Test ueber JTAG, denn die ersten Zeilen des
/// zweiten Laufs entstehen, bevor der Host den Port wieder oeffnet; der
/// Wert belegt dabei, dass der RTC-RAM den Schlaf ueberlebt. Danach traegt
/// die Konsole den zweiten Lauf von seiner ersten Zeile an; nach dem
/// Bus-Reset der Neuanmeldung blieb sie stumm (FB-311).
#[test]
#[ignore = "Board: TAKT_ESP32C6_PORT; mit --ignored"]
fn a_run_ended_after_a_delay_begins_the_next_after_it() {
    let Some((mut board, _guard)) = board() else { return };
    let options = Options::timed(0);
    let program = board::root().join("crates/takt-conformance/tests/programs/deep_sleep.takt");
    let elf = board.build(&program, &options).unwrap_or_else(|e| panic!("{e}"));
    let Some(first) = ran(board.run(&elf, &options)) else { return };
    assert!(first.contains("end after"), "der erste Lauf endet mit `next_run`:\n{first}");
    assert!(board.port_gone(Duration::from_secs(3)), "der Port blieb: kein Schlaf");
    let slept = Instant::now();
    assert!(board.listen_port(Duration::from_secs(8)), "der Port kam nicht zurueck: kein Wecken");
    assert!(slept.elapsed() >= Duration::from_millis(1500), "zu frueh geweckt: {:?}", slept.elapsed());
    std::thread::sleep(Duration::from_millis(500));
    let previous = board.word_over_jtag(&elf, |n| n.contains("PREVIOUS_RUN")).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(previous, 1, "der zweite Lauf beginnt mit `previous_run = ENDED` (12.7)");
    let heard = board.listen(Duration::from_secs(3)).unwrap_or_else(|e| panic!("{e}"));
    assert!(heard.contains("out woke 1"), "die Konsole schweigt nach dem Wecken (FB-311):\n{heard}");
}

/// **`next_run = NOW` beginnt sofort den naechsten Lauf, und der weiss es**
/// (12.7): Der erste Lauf endet nach 300 ms; das Board setzt sich zurueck,
/// und der zweite beginnt mit `previous_run = ENDED` und zeigt es eine
/// Sekunde spaeter an `again`, wenn der Wirt die Leitung wieder offen hat.
/// Der erste kennt nach dem Flashen keinen vorigen Lauf. Ein freier Lauf in
/// Echtzeit, wie beim Schlaf.
///
/// Der Neustart ist ein Software-Reset des HP-Systems: Der USB-Serial-JTAG
/// bleibt angemeldet, und die Konsole traegt den zweiten Lauf.
#[test]
#[ignore = "Board: TAKT_ESP32C6_PORT; mit --ignored"]
fn a_run_ended_now_begins_the_next_at_once() {
    let Some((mut board, _guard)) = board() else { return };
    let options = Options::timed(0);
    let program = board::root().join("crates/takt-conformance/tests/programs/restart.takt");
    let elf = board.build(&program, &options).unwrap_or_else(|e| panic!("{e}"));
    let Some(first) = ran(board.run(&elf, &options)) else { return };
    assert!(first.contains("end now"), "der erste Lauf endet mit `next_run`:\n{first}");
    assert!(first.contains("out previous NONE"), "nach dem Flashen kein voriger Lauf:\n{first}");
    let second = board.listen(Duration::from_secs(4)).unwrap_or_else(|e| panic!("{e}"));
    assert!(second.contains("out again 1"), "der zweite Lauf beginnt mit `ENDED`:\n{second}");
    assert!(second.contains("out previous ENDED"), "{second}");
}

/// **Ein Job, der laenger rechnet als ein Tick, verspaetet keinen** (4.5,
/// 12.3, M10 Schritt 7): SHA-256 ueber 4096 Byte im Job-Faden, den jeder
/// Tick unterbricht (`long_job_keeps_the_tick`).
#[test]
#[ignore = "Board: TAKT_ESP32C6_PORT; mit --ignored"]
fn a_long_job_runs_between_the_ticks() {
    let Some((mut board, _guard)) = board() else { return };
    let failed = long_job_keeps_the_tick(&mut board);
    verdict(&failed);
}

/// **Ein Ueberlauf faultet im naechsten Tick jede Maschine** (7.3, 5.4,
/// FB-332): `overrun_reaches_every_machine`.
#[test]
#[ignore = "Board: TAKT_ESP32C6_PORT; mit --ignored"]
fn an_overrun_faults_every_machine_in_the_next_tick() {
    let Some((mut board, _guard)) = board() else { return };
    let failed = overrun_reaches_every_machine(&mut board);
    verdict(&failed);
}

/// **Ein ausgelassener Kick setzt zurueck, und der naechste Lauf weiss es**
/// (12.3, 12.7). Der erste Lauf zeigt `previous_run`, schlaeft eine Sekunde
/// in `idle` und rechnet dann einen Tick lang weit ueber die Frist des
/// Watchdogs. Der zweite beginnt mit `previous_run = WATCHDOG` und bleibt
/// stehen. Dass jeder Lauf vor dem Reset `rested` zeigt, belegt den Schlaf:
/// Die Schleife bestaetigt den Watchdog je geschlafenem Tick.
///
/// Der MWDT setzt das HP-System zurueck wie ein Software-Reset; der
/// USB-Serial-JTAG bleibt angemeldet, und die Konsole traegt beide Laeufe.
#[test]
#[ignore = "Board: TAKT_ESP32C6_PORT; mit --ignored"]
fn a_missed_kick_begins_the_next_run_after_the_watchdog() {
    let Some((mut board, _guard)) = board() else { return };
    let options = Options::timed(0);
    let program = board::root().join("crates/takt-conformance/tests/programs/watchdog.takt");
    let elf = board.build(&program, &options).unwrap_or_else(|e| panic!("{e}"));
    let Some(text) = ran(board.run_for(&elf, Duration::from_secs(10))) else { return };
    // Je Lauf ein Abschnitt ab der Marke; davor steht nur der Start.
    let runs: Vec<&str> = text.split(board::MARK).skip(1).collect();
    let first = |run: &str, name: &str| {
        let key = format!(" out {name} ");
        run.lines().find_map(|l| l.split_once(key.as_str()).map(|(_, v)| v.trim().to_string()))
    };
    let previous: Vec<String> = runs.iter().filter_map(|r| first(r, "previous")).collect();
    assert_eq!(previous, ["NONE", "WATCHDOG"], "der zweite Lauf weiss vom Watchdog (12.7):\n{text}");
    assert!(runs.iter().all(|r| r.contains("out rested 1")), "jeder Lauf schlief vor dem Reset:\n{text}");
}

/// **`guard` aus der Konfiguration wirkt auf dem Board** (7.5, FB-331):
/// `a_schedule_inside_the_guard_is_a_timing_fault`.
#[test]
#[ignore = "Board: TAKT_ESP32C6_PORT; mit --ignored"]
fn a_schedule_inside_the_guard_is_a_timing_fault() {
    let Some((mut board, _guard)) = board() else { return };
    let failed = common::board::a_schedule_inside_the_guard_is_a_timing_fault(&mut board);
    verdict(&failed);
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
#[ignore = "Board: TAKT_ESP32C6_PORT; mit --ignored"]
fn a_driver_machine_writes_uart0_registers() {
    // Die Bruecke an UART0 (CP2102) steckt nicht an jedem Rechner; fehlt
    // sie, ueberspringt nur `TAKT_ALLOW_MISSING=uart-bridge` den Test.
    let port = std::env::var("TAKT_ESP32C6_UART_PORT").ok();
    let hint = "`TAKT_ESP32C6_UART_PORT` auf den Anschluss der Bruecke setzen (COM5)";
    let Some(uart) = takt_testkit::require("uart-bridge", port, hint) else { return };
    let Some((board, _guard)) = board() else { return };
    // Zehn Ticks je Zeile, plus Rand: Der Lauf muss ueber `WANT` Zeilen
    // hinaus reichen, sonst haelt das Programm mittendrin. In Echtzeit,
    // weil die UART die Bytes mit ihrer Rate abnimmt.
    let options = Options::timed((WANT as u64 + 4) * 10);
    let elf = board.build(&bringup_program("uart0_port.takt"), &options).unwrap_or_else(|e| panic!("{e}"));
    let Some(()) = ran(board.download(&elf)) else { return };

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
#[ignore = "Board: TAKT_ESP32C6_PORT; mit --ignored"]
fn a_board_input_reaches_the_process_image() {
    let Some((mut board, _guard)) = board() else { return };
    let options = Options::fresh(40);
    let Some(text) =
        ran(board.build(&bringup_program("button_input.takt"), &options).and_then(|elf| board.run(&elf, &options)))
    else {
        return;
    };

    // Der Taster ist ungedrueckt: `led` bleibt aus, `pressed` bei null.
    assert_eq!(last_output(&text, "led").as_deref(), Some("0"), "{text}");
    assert_eq!(last_output(&text, "pressed").as_deref(), Some("0"), "{text}");

    // Und das ist die Aussage: Waere der Treiber nicht gerufen worden,
    // bliebe der Eintrag `Bad` — dann faultete `btn.or(false)` nicht,
    // aber `shaky` zaehlte jeden Tick, weil `Bad` auch `suspect` ist.
    assert_eq!(last_output(&text, "shaky").as_deref(), Some("0"), "{text}");
}

/// **`persist` ueberlebt einen Reset** (5.9, 12.3; plan/esp32c6.md 6): Der
/// zweite Lauf ist derselbe Chip nach einem Reset, ohne neues Abbild.
#[test]
#[ignore = "Board: TAKT_ESP32C6_PORT; mit --ignored"]
fn persistence_survives_a_reset() {
    let Some((mut board, _guard)) = board() else { return };
    let failed = persistence_survives(&mut board, |board, elf, options| board.capture(elf, options));
    verdict(&failed);
}

/// **`idle` schlaeft** (9.9, Satz 9.9.1; plan/esp32c6.md 6):
/// `56_idle_timer` verbringt 500 von 700 ms im Schlaf, mit dem Timer als
/// einziger Wake-Quelle; die Schleife zaehlt die Ticks dazwischen als
/// virtuelle, und der Trace bleibt der des Interpreters.
#[test]
#[ignore = "Board: TAKT_ESP32C6_PORT; mit --ignored"]
fn an_idle_state_sleeps_in_virtual_ticks() {
    let Some((mut board, _guard)) = board() else { return };
    let name = "56_idle_timer.takt";
    let Some(text) = run_corpus(&mut board, name, true) else { return };
    let slept = slept(&text).unwrap_or_else(|| panic!("keine Schlafzeile:\n{text}"));
    assert!(slept >= 40, "60 Ticks mit 500 ms `idle` bei 10 ms Tick: mehr als 40 virtuelle erwartet, {slept}:\n{text}");
    let diffs = compare(&replayed(&corpus(name), &text), &text);
    assert!(diffs.is_empty(), "{diffs:?}");
}

/// **14.7 laeuft auf Board 2** (M10 Schritt 23): Selbsttest, Betrieb,
/// `IDLE_WAIT` und nach zwei Minuten `STANDBY`, mit dem `persist`-Journal
/// im Flash. AFE und Ladegeraet sind `sim`-Bindungen: Das Modell rechnet
/// auf dem Board mit, und weil es nicht `idle` ist, schlaeft das System in
/// `STANDBY` nicht (9.9) — den Schlaf belegt `56_idle_timer`. 12 500 Ticks
/// in logischer Zeit, Tick fuer Tick gegen den Interpreter.
#[test]
#[ignore = "Board: TAKT_ESP32C6_PORT; mit --ignored"]
fn the_battery_manager_of_14_7_runs_on_board_2() {
    const TICKS_14_7: u64 = 12_500;
    let Some((mut board, _guard)) = board() else { return };
    let name = "sim/14_7/program.takt";
    let options = Options::fresh(TICKS_14_7);
    let built = board.build(&board::corpus_path(name), &options);
    let Some(text) = ran(built.and_then(|elf| board.run(&elf, &options)).map_err(|e| format!("{name}: {e}"))) else {
        return;
    };
    assert!(board::counter(&text, "journal geschrieben").is_some_and(|n| n >= 1), "kein Journal:\n{}", tail(&text));
    let options = takt_interp::RunOptions { ticks: TICKS_14_7, ..Default::default() };
    let interpreted =
        takt_interp::run(&corpus(name), &takt_interp::Trace::default(), &options).expect("Lauf").trace.render();
    // Die Board-Telemetrie traegt keine Zustaende; `STANDBY` zeigt sich dort
    // an seinem `enter:` — LED aus, beide FETs offen.
    let standby = interpreted.lines().find(|l| l.ends_with("state bms STANDBY")).expect("STANDBY im Interpreter");
    let at = standby.split_whitespace().next().expect("Tick");
    assert!(text.lines().any(|l| l.starts_with(&format!("{at} out led 0"))), "kein STANDBY:\n{}", tail(&text));
    let diffs = compare(&interpreted, &text);
    assert!(diffs.is_empty(), "{} Abweichungen, etwa {:?}", diffs.len(), &diffs[..diffs.len().min(6)]);
}

/// **Die Waechter unter beiden Stacks greifen** (12.3, M10 Schritt 18). Auf
/// dem C6 sind es Daten-Watchpoints: der von `esp-hal` auf einem Wort knapp
/// ueber dem unteren Ende des Hauptstacks, der des Boards auf den 32 Byte
/// unter dem Job-Stack. Nach Tick 2 schreibt ein Pruefgeraet zwischen zwei
/// Ticks hinein; der Kern haelt mit der Meldung des Watchpoints an, statt
/// weiterzurechnen. Eine Region, die den Programmzustand je Tick schuetzt,
/// hat der C6 nicht (m10.md 2.12).
#[test]
#[ignore = "Board: TAKT_ESP32C6_PORT; mit --ignored"]
fn the_stack_guard_of_board_2_stops_a_write_of_the_tcb() {
    let Some((mut board, _guard)) = board() else { return };
    for (which, message) in [("guard", "stack's guard"), ("job_guard", "Breakpoint exception")] {
        let program = board::root().join(format!("crates/takt-conformance/tests/programs/protect_{which}.takt"));
        let options = Options::fresh(20);
        let elf = board.build(&program, &options).unwrap_or_else(|e| panic!("{e}"));
        let Some(text) = ran(board.run_for(&elf, Duration::from_secs(6))) else { continue };
        assert!(
            text.contains("takt panic") && text.contains(message),
            "{which}: der Waechter griff nicht:\n{}",
            tail(&text)
        );
        assert!(!text.contains("t=5 out alive"), "{which}: das Board rechnete weiter:\n{}", tail(&text));
    }
}

/// Die letzten Zeilen eines Traces, fuer Meldungen.
fn tail(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(12)..].join("\n")
}

/// **Unter `overrun = alert` kostet das Journal Zeit, aber keine
/// Semantik** (7.3, 12.3): Ein Schreibvorgang haelt den Kern zweistellige
/// Perioden lang an; die Tickzahl kommt aus dem SYSTIMER (FB-198).
#[test]
#[ignore = "Board: TAKT_ESP32C6_PORT; mit --ignored"]
fn the_journal_costs_time_but_not_semantics() {
    let Some((mut board, _guard)) = board() else { return };
    let failed = journal_costs_time_but_not_semantics(&mut board);
    verdict(&failed);
}

/// **Ein Stillstand ist Rueckstand und wird nachgeholt** (7.3, FB-198):
/// Eine Sektorloeschung haelt den Tick zweistellige Millisekunden an, und
/// die Uhr liest den SYSTIMER.
#[test]
#[ignore = "Board: TAKT_ESP32C6_PORT; mit --ignored"]
fn a_stall_is_lag_and_caught_up() {
    let Some((mut board, _guard)) = board() else { return };
    let failed = common::board::a_stall_is_lag_and_caught_up(&mut board);
    verdict(&failed);
}

/// **Unter `overrun = fault` schreibt das Journal im Schlaf** (9.9, 12.3),
/// mit den Zeiten aus `corpus-try/hw/esp32c6.hw`.
#[test]
#[ignore = "Board: TAKT_ESP32C6_PORT; mit --ignored"]
fn the_journal_writes_in_sleep_windows() {
    let Some((mut board, _guard)) = board() else { return };
    let failed = journal_writes_in_sleep_windows(&mut board);
    verdict(&failed);
}

/// **Der Wunsch ueber die Konsole setzt den Chip zurueck** (FB-264):
/// `TAKT` in den Empfangspuffer, Reset ueber RTS — der Port geht und
/// kommt, und JTAG antwortet danach, auch wenn es vorher stand.
#[test]
#[ignore = "Board: TAKT_ESP32C6_PORT; mit --ignored"]
fn the_board_resets_on_console_request() {
    let Some((board, _guard)) = board() else { return };
    let Some(()) = ran(board.reenumerate_via_console()) else { return };
    board.probe_rs_plain(&["read", "--chip", "esp32c6", "b32", REENUMERATE_REG, "1"]).unwrap_or_else(|e| panic!("{e}"));
}

/// **Das Board meldet sein USB-Geraet auf Wunsch neu an** (FB-266).
///
/// Der Wunsch ueber JTAG in STORE0, ein Reset: Der Port verschwindet und
/// kommt zurueck, und ein Lauf danach spricht wie zuvor. Dazu der
/// Tickzaehler ueber JTAG, der eine stumme Konsole von einem stehenden
/// Programm unterscheidet.
#[test]
#[ignore = "Board: TAKT_ESP32C6_PORT; mit --ignored"]
fn the_board_reenumerates_its_usb_on_request() {
    let Some((mut board, _guard)) = board() else { return };
    let name = "01_minimal.takt";
    let options = Options::fresh(TICKS);
    let elf = board.build(&board::corpus_path(name), &options).unwrap_or_else(|e| panic!("{name}: {e}"));
    let Some(first) = ran(board.run(&elf, &options).map_err(|e| format!("{name}: {e}"))) else { return };
    assert_eq!(board.tick_over_jtag(&elf).unwrap_or_else(|e| panic!("{e}")), TICKS as u32, "{first}");
    board.reenumerate().unwrap_or_else(|e| panic!("{e}"));
    let second = board.capture_once(&[]).unwrap_or_else(|e| panic!("{name}, nach der Neuanmeldung: {e}"));
    assert!(second.contains(board::END), "nach der Neuanmeldung:\n{second}");
    let outs = |t: &str| {
        t.lines().filter(|l| l.starts_with("t=") && l.contains(" out ")).map(str::to_string).collect::<Vec<_>>()
    };
    assert_eq!(outs(&first), outs(&second));
}

#[test]
#[ignore = "Board: TAKT_ESP32C6_PORT; mit --ignored"]
fn the_board_agrees_with_the_interpreter() {
    let Some((mut board, _guard)) = board() else { return };
    // `TAKT_ESP32C6_ONLY=42_map.takt` fuer einen einzelnen Fall.
    let only = std::env::var("TAKT_ESP32C6_ONLY").ok();
    let failed = agreement(&mut board, &board::corpus(), only.as_deref());
    verdict(&failed);
}

/// **In der Interruptform rechnet der Korpus wie der Interpreter** (12.11,
/// M11 Schritt 14): Den Schritt rechnet die ISR des SYSTIMER-Vergleichers
/// zu der Frist, die `service` nennt, Jobs ein Software-Interrupt, und
/// darunter laeuft eine fremde Hauptschleife. In logischer Zeit gibt sie
/// den Alarm erst frei, wenn das System ruht.
#[test]
#[ignore = "Board: TAKT_ESP32C6_PORT; mit --ignored"]
fn the_board_agrees_with_the_interpreter_in_the_interrupt_form() {
    let Some((mut board, _guard)) = board() else { return };
    let only = std::env::var("TAKT_ESP32C6_ONLY").ok();
    let names: Vec<&str> = board::corpus().into_iter().filter(|n| runs_shared(n)).collect();
    let failed = agreement_with(&mut board, &names, only.as_deref(), &Options::fresh(TICKS).in_form(Form::Interrupt));
    verdict(&failed);
}

/// **In der Interruptform beginnt der Schritt auf seiner Frist, neben einer
/// fremden Hauptschleife** (12.11).
#[test]
#[ignore = "Board: TAKT_ESP32C6_PORT; mit --ignored"]
fn the_interrupt_form_steps_on_its_deadline_beside_a_foreign_main_loop() {
    let Some((mut board, _guard)) = board() else { return };
    let failed = the_form_keeps_its_deadline(&mut board, Form::Interrupt);
    verdict(&failed);
}

/// **Unter Embassy rechnet der Korpus wie der Interpreter** (12.11, M11
/// Schritt 14d): Takt als Aufgabe eines Executors hoher Prioritaet, die mit
/// `embassy-time` auf die Frist wartet; Jobs im Software-Interrupt, darunter
/// eine fremde Hauptschleife.
#[test]
#[ignore = "Board: TAKT_ESP32C6_PORT; mit --ignored"]
fn the_board_agrees_with_the_interpreter_under_embassy() {
    let Some((mut board, _guard)) = board() else { return };
    let only = std::env::var("TAKT_ESP32C6_ONLY").ok();
    let names: Vec<&str> = board::corpus().into_iter().filter(|n| runs_shared(n)).collect();
    let failed = agreement_with(&mut board, &names, only.as_deref(), &Options::fresh(TICKS).in_form(Form::Embassy));
    verdict(&failed);
}

/// **Unter Embassy beginnt der Schritt auf seiner Frist, neben einer fremden
/// Hauptschleife** (12.11, M11 Schritt 14d).
#[test]
#[ignore = "Board: TAKT_ESP32C6_PORT; mit --ignored"]
fn the_embassy_task_steps_on_its_deadline_beside_a_foreign_main_loop() {
    let Some((mut board, _guard)) = board() else { return };
    let failed = the_form_keeps_its_deadline(&mut board, Form::Embassy);
    verdict(&failed);
}

/// **Jeder Port des C6 besteht die Portpruefung** (12.11, 13.8; M11
/// Schritt 15): Interruptform und Embassy. Ohne FPU rechnet die fremde Last
/// in Software; die FPU-Pflicht belegt hier nur, dass sie lief.
#[test]
#[ignore = "Board: TAKT_ESP32C6_PORT; mit --ignored"]
fn every_port_passes_the_port_test() {
    let Some((mut board, _guard)) = board() else { return };
    let failed: Vec<String> =
        [Form::Interrupt, Form::Embassy].into_iter().flat_map(|f| the_port_passes(&mut board, f)).collect();
    verdict(&failed);
}

/// **Der Zephyr-Port besteht die Portpruefung auf dem C6** (12.11, 13.8;
/// M11 Schritt 19): `examples/zephyr` fuer `esp32c6_devkitc/esp32c6/hpcore`
/// faehrt das Pruefprogramm ueber den Port mit Faeden und seinen Adapter fuer
/// Zephyr. `zephyr.bin` traegt den Kopf fuer den ROM-Lader und geht an
/// `CONFIG_FLASH_LOAD_OFFSET`; die Leitung ist der USB-Serial-JTAG. Braucht
/// ausser dem Board Zephyr (`TAKT_WEST` und die Umgebung aus
/// `D:\takt-tools\activate.ps1`) und `<target>/release/takt`.
#[test]
#[ignore = "Board: TAKT_ESP32C6_PORT; Zephyr: TAKT_WEST; mit --ignored"]
fn the_zephyr_port_passes_the_port_test() {
    let Some((mut board, _guard)) = board() else { return };
    let options = Options::timed(takt_conformance::port::TICKS).judged();
    let example = takt_conformance::zephyr::ZEPHYR;
    let run = zephyr_image(board.name(), example, takt_conformance::zephyr::ESP32C6, true)
        .and_then(|image| zephyr_run(&mut board, &image, &options));
    verdict(&zephyr_verdict(board.name(), example, true, run));
}

/// **Der Port der Interruptform unter ESP-IDF besteht die Portpruefung**
/// (12.11, 13.8; M11 Schritt 21a): `examples/esp-idf` mit der Komponente
/// `takt`, dem Adapter `ports/irq/esp-idf` und dem Journal in seiner
/// Partition. Den Schritt rechnet die IRAM-ISR des `gptimer`, die Jobs eine
/// Aufgabe, darunter rechnet `app_main` die fremde Last; die Leitung ist der
/// USB-Serial-JTAG. Das Abbild am Stueck geht an 0x0. Baut mit ESP-IDF
/// (`TAKT_IDF`), am Board aus dem Zwischenspeicher.
#[test]
#[ignore = "Board: TAKT_ESP32C6_PORT; ESP-IDF: TAKT_IDF; mit --ignored"]
fn the_esp_idf_port_passes_the_port_test() {
    let Some((board, _guard)) = board() else { return };
    let failed = the_port_passes(&mut Esp32c6Idf::on(board), Form::Interrupt);
    verdict(&failed);
}

/// **Unter ESP-IDF wird ein Tick ueber seiner Periode `Runtime(Overrun)`**
/// (7.3, 12.11; M11 Schritt 21a): Nach dem langen Tick liegt jede Frist schon
/// zurueck, wenn der Adapter den Alarm stellt; er laesst die ISR dann selbst
/// anstehen, statt auf einen Alarm zu warten, der nicht kommt
/// (`ports/irq/esp-idf`). Ohne das stuende der Lauf nach dem Ueberlauf.
#[test]
#[ignore = "Board: TAKT_ESP32C6_PORT; ESP-IDF: TAKT_IDF; mit --ignored"]
fn the_esp_idf_port_turns_an_overrun_into_a_fault() {
    let Some((board, _guard)) = board() else { return };
    let failed = overrun_reaches_every_machine_with(&mut Esp32c6Idf::on(board), "overrun_sim.takt", Form::Interrupt);
    verdict(&failed);
}

/// Schreibt ein Zephyr-Abbild (`zephyr.bin` mit dem Kopf fuer den ROM-Lader,
/// an `CONFIG_FLASH_LOAD_OFFSET`), startet es und liest bis `takt end`.
fn zephyr_run(
    board: &mut Esp32c6,
    image: &takt_conformance::zephyr::Image,
    options: &Options,
) -> Result<String, String> {
    let offset = image.config("FLASH_LOAD_OFFSET").ok_or("CONFIG_FLASH_LOAD_OFFSET fehlt")?;
    let offset = u64::from_str_radix(offset.trim_start_matches("0x"), 16).map_err(|e| format!("{offset}: {e}"))?;
    board.run_raw(&image.bin, offset, &image.elf, options)
}

/// **`persist` ueberlebt unter Zephyr einen Neustart des C6** (5.9; M11
/// Schritt 19, FB-732): `examples/zephyr` ohne frisches Journal, geschrieben
/// und gelesen, dann ein Reset und ein zweiter Lauf desselben Abbilds; das
/// Journal liegt in der Partition `takt_journal` (0x3b_0000).
#[test]
#[ignore = "Board: TAKT_ESP32C6_PORT; Zephyr: TAKT_WEST; mit --ignored"]
fn the_zephyr_journal_survives_a_reset() {
    let Some((mut board, _guard)) = board() else { return };
    let options = Options::timed(takt_conformance::port::TICKS).judged();
    let image =
        match zephyr_image(board.name(), takt_conformance::zephyr::ZEPHYR, takt_conformance::zephyr::ESP32C6, false) {
            Ok(image) => image,
            Err(e) => return verdict(&[e]),
        };
    let failed = zephyr_persistence_survives(
        &mut board,
        &image,
        |board, image| zephyr_run(board, image, &options),
        |board, image| board.capture(&image.elf, &options),
    );
    verdict(&failed);
}

/// **Zwei Jobs desselben Ticks sind im naechsten fertig**, im eigenen Kern,
/// in der Interruptform und unter Embassy (4.5, 12.11).
#[test]
#[ignore = "Board: TAKT_ESP32C6_PORT; mit --ignored"]
fn simultaneous_jobs_finish_on_time_on_the_board() {
    let Some((mut board, _guard)) = board() else { return };
    let failed: Vec<String> = [Form::Own, Form::Interrupt, Form::Embassy]
        .into_iter()
        .flat_map(|f| simultaneous_jobs_finish_on_time(&mut board, f))
        .collect();
    verdict(&failed);
}

#[test]
#[ignore = "Board: TAKT_ESP32C6_PORT; mit --ignored"]
fn the_natives_agree_with_the_host() {
    let Some((mut board, _guard)) = board() else { return };
    let failed = natives_agree(&mut board);
    verdict(&failed);
}

/// **Der Treiberrand urteilt auf dem Board wie im Interpreter** (12.6, M10
/// Schritt 29c): das Pruefgeraet des Bring-ups mit jedem Verstoss einmal.
#[test]
#[ignore = "Board: TAKT_ESP32C6_PORT; mit --ignored"]
fn the_driver_edge_judges_like_the_interpreter() {
    let Some((mut board, _guard)) = board() else { return };
    let failed = driver_edge_agrees(&mut board);
    verdict(&failed);
}

/// **Ein Zeitgeber, der seine Periode verfehlt, ist `Runtime(Hardware)`**
/// (12.6 Zeile 7, M10 Schritt 29c): das Pruefgeraet streckt die Periode.
#[test]
#[ignore = "Board: TAKT_ESP32C6_PORT; mit --ignored"]
fn a_stretched_tick_is_runtime_hardware() {
    let Some((mut board, _guard)) = board() else { return };
    let failed = common::board::a_stretched_tick_is_runtime_hardware(&mut board);
    verdict(&failed);
}

/// **Ein Tune von der Konsole gilt ab seiner Grenze und weckt das Board**
/// (8.4, 9.9, FB-389): in Echtzeit, der Host schickt waehrend des Schlafs.
#[test]
#[ignore = "Board: TAKT_ESP32C6_PORT; mit --ignored"]
fn a_tune_from_the_console_wakes_the_board() {
    let Some((mut board, _guard)) = board() else { return };
    let failed = common::board::a_tune_from_the_console_wakes_the_board(&mut board);
    verdict(&failed);
}

/// **Eine Wake-Quelle weckt an der Grenze nach ihrem Ereignis** (9.9,
/// FB-388): Das Pruefgeraet hebt einen Pegel und laeutet eine Klingel, die
/// das Programm nicht vorher kennt.
#[test]
#[ignore = "Board: TAKT_ESP32C6_PORT; mit --ignored"]
fn a_wake_source_ends_the_sleep() {
    let Some((mut board, _guard)) = board() else { return };
    let failed = common::board::a_wake_source_ends_the_sleep(&mut board);
    verdict(&failed);
}

/// **Was das Programm nicht liest, zeichnet der Rahmen auf** (8.2, 12.5,
/// M10 Schritt 29d): das Pruefgeraet liefert Kanaele ohne Bindung.
#[test]
#[ignore = "Board: TAKT_ESP32C6_PORT; mit --ignored"]
fn the_unread_channels_are_recorded() {
    let Some((mut board, _guard)) = board() else { return };
    let failed = common::board::unread_channels_are_recorded(&mut board);
    verdict(&failed);
}
