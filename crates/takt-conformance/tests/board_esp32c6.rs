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

use common::board::{
    TICKS, agreement, corpus, driver_edge_agrees, last_output, long_job_keeps_the_tick, natives_agree,
    overrun_reaches_every_machine, run_interpreted,
};
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

/// Baut ein Korpusprogramm, schreibt es und liest seinen Trace — in
/// Echtzeit: Die Tests damit pruefen Schlaf und Journal an der Uhr.
fn run_corpus(board: &mut Esp32c6, name: &str, fresh: bool) -> String {
    let options = Options { ticks: TICKS, fresh, bin: Bin::Takt, timed: true, hardware: None, rtos: false };
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
fn a_run_ended_after_a_delay_begins_the_next_after_it() {
    let Some((mut board, _guard)) = board() else { return };
    let options = Options { ticks: 0, fresh: true, bin: Bin::Takt, timed: true, hardware: None, rtos: false };
    let program = board::root().join("crates/takt-conformance/tests/programs/deep_sleep.takt");
    let elf = board.build(&program, &options).unwrap_or_else(|e| panic!("{e}"));
    let first = board.run(&elf, &options).unwrap_or_else(|e| panic!("{e}"));
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
fn a_run_ended_now_begins_the_next_at_once() {
    let Some((mut board, _guard)) = board() else { return };
    let options = Options { ticks: 0, fresh: true, bin: Bin::Takt, timed: true, hardware: None, rtos: false };
    let program = board::root().join("crates/takt-conformance/tests/programs/restart.takt");
    let elf = board.build(&program, &options).unwrap_or_else(|e| panic!("{e}"));
    let first = board.run(&elf, &options).unwrap_or_else(|e| panic!("{e}"));
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
fn a_long_job_runs_between_the_ticks() {
    let Some((mut board, _guard)) = board() else { return };
    let failed = long_job_keeps_the_tick(&mut board);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

/// **Ein Ueberlauf faultet im naechsten Tick jede Maschine** (7.3, 5.4,
/// FB-332): `overrun_reaches_every_machine`.
#[test]
fn an_overrun_faults_every_machine_in_the_next_tick() {
    let Some((mut board, _guard)) = board() else { return };
    let failed = overrun_reaches_every_machine(&mut board);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
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
fn a_missed_kick_begins_the_next_run_after_the_watchdog() {
    let Some((mut board, _guard)) = board() else { return };
    let options = Options { ticks: 0, fresh: true, bin: Bin::Takt, timed: true, hardware: None, rtos: false };
    let program = board::root().join("crates/takt-conformance/tests/programs/watchdog.takt");
    let elf = board.build(&program, &options).unwrap_or_else(|e| panic!("{e}"));
    let text = board.run_for(&elf, Duration::from_secs(10)).unwrap_or_else(|e| panic!("{e}"));
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
/// `at now + 1 us` liegt unter der gemessenen Treiberlatenz der Bruecke
/// und ist ein `TimingFault`, `at now + 1 ms` geht durch. Derselbe Bau
/// ohne Konfiguration rechnet wie die Simulation und laesst beides durch.
#[test]
fn a_schedule_inside_the_guard_is_a_timing_fault() {
    let Some((mut board, _guard)) = board() else { return };
    let program = board::root().join("crates/takt-conformance/tests/programs/guard.takt");
    let mut run = |options: Options| {
        board.build(&program, &options).and_then(|elf| board.run(&elf, &options)).unwrap_or_else(|e| panic!("{e}"))
    };
    let with = run(Options::fresh(20).with_hardware(board::root().join("corpus-try/hw/esp32c6.hw")));
    assert!(with.lines().any(|l| l.contains("out probe 1")), "1 ms voraus geht durch:\n{with}");
    assert!(with.lines().any(|l| l.contains("fault m") && l.contains("Timing")), "{with}");
    let without = run(Options::fresh(20));
    assert!(!without.contains("fault m"), "ohne Konfiguration ist guard null:\n{without}");
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
    // hinaus reichen, sonst haelt das Programm mittendrin. In Echtzeit,
    // weil die UART die Bytes mit ihrer Rate abnimmt.
    let options = Options {
        ticks: (WANT as u64 + 4) * 10,
        fresh: true,
        bin: Bin::Takt,
        timed: true,
        hardware: None,
        rtos: false,
    };
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
    let options = Options { ticks: TICKS, fresh: false, bin: Bin::Takt, timed: false, hardware: None, rtos: false };
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

/// **14.7 laeuft auf Board 2** (M10 Schritt 23): Selbsttest, Betrieb,
/// `IDLE_WAIT` und nach zwei Minuten `STANDBY`, mit dem `persist`-Journal
/// im Flash. AFE und Ladegeraet sind `sim`-Bindungen: Das Modell rechnet
/// auf dem Board mit, und weil es nicht `idle` ist, schlaeft das System in
/// `STANDBY` nicht (9.9) — den Schlaf belegt `56_idle_timer`. 12 500 Ticks
/// in logischer Zeit, Tick fuer Tick gegen den Interpreter.
#[test]
fn the_battery_manager_of_14_7_runs_on_board_2() {
    const TICKS_14_7: u64 = 12_500;
    let Some((mut board, _guard)) = board() else { return };
    let name = "sim/14_7/program.takt";
    let options = Options::fresh(TICKS_14_7);
    let text = board
        .build(&board::corpus_path(name), &options)
        .and_then(|elf| board.run(&elf, &options))
        .unwrap_or_else(|e| panic!("{name}: {e}"));
    assert!(counter(&text, "journal geschrieben").is_some_and(|n| n >= 1), "kein Journal:\n{}", tail(&text));
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
fn the_stack_guard_of_board_2_stops_a_write_of_the_tcb() {
    let Some((mut board, _guard)) = board() else { return };
    for (which, message) in [("guard", "stack's guard"), ("job_guard", "Breakpoint exception")] {
        let program = board::root().join(format!("crates/takt-conformance/tests/programs/protect_{which}.takt"));
        let options = Options::fresh(20);
        let elf = board.build(&program, &options).unwrap_or_else(|e| panic!("{e}"));
        let text = board.run_for(&elf, Duration::from_secs(6)).unwrap_or_else(|e| panic!("{e}"));
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

/// **Der Treiberrand urteilt auf dem Board wie im Interpreter** (12.6, M10
/// Schritt 29c): das Pruefgeraet des Bring-ups mit jedem Verstoss einmal.
#[test]
fn the_driver_edge_judges_like_the_interpreter() {
    let Some((mut board, _guard)) = board() else { return };
    let failed = driver_edge_agrees(&mut board);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

/// **Ein Zeitgeber, der seine Periode verfehlt, ist `Runtime(Hardware)`**
/// (12.6 Zeile 7, M10 Schritt 29c): das Pruefgeraet streckt die Periode.
#[test]
fn a_stretched_tick_is_runtime_hardware() {
    let Some((mut board, _guard)) = board() else { return };
    let failed = common::board::a_stretched_tick_is_runtime_hardware(&mut board);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

/// **Was das Programm nicht liest, zeichnet der Rahmen auf** (8.2, 12.5,
/// M10 Schritt 29d): das Pruefgeraet liefert Kanaele ohne Bindung.
#[test]
fn the_unread_channels_are_recorded() {
    let Some((mut board, _guard)) = board() else { return };
    let failed = common::board::unread_channels_are_recorded(&mut board);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}
