//! Board 1, STM32F401: der Korpus auf dem Chip gegen den Interpreter
//! (13.8, Satz 9.4.4; plan/f401.md).
//!
//! **Laeuft nur mit Board.** `TAKT_F401_PORT` nennt den Port des Adapters
//! an USART1 (`COM7`); ohne Port wird der Test uebersprungen. Das Board
//! braucht keine Hand: Jedes Programm gibt es auf `TAKT` an den
//! DFU-Bootloader zurueck (FB-275).
//!
//! **Oder baut nur.** Mit `TAKT_BOARD_BUILD_ONLY=1` baut jeder Test die
//! Abbilder, die er braucht, und endet vor dem Schreiben bestanden (M11
//! Schritt 34d, `.github/workflows/boards.yml`).

mod common;

use std::time::Duration;

use common::board::{
    Case, Drift, GUARD_WATCHPOINT, PARTITION_REFUSED, TICKS, ZEPHYR_COUNTER_CLOCK, ZEPHYR_WIRE_TICKS,
    a_foreign_write_becomes_runtime_hardware, a_hostile_fpu_changes_nothing, a_write_into_the_other_arena_is_refused,
    agreement, agreement_with, binds_no_driver, corpus, driver_edge_agrees, fits_the_poll_form, header_names,
    journal_costs_time_but_not_semantics, journal_writes_in_sleep_windows, last_output, long_job_keeps_the_tick,
    natives_agree, overrun_reaches_every_machine, persistence_survives, ran, runs_shared,
    simultaneous_jobs_finish_on_time, the_flawed_port_fails, the_form_keeps_its_deadline,
    the_poll_form_keeps_its_round, the_port_passes, the_port_passes_with_header, two_programs_share_a_core, verdict,
    zephyr_access_in_the_run_faults, zephyr_image, zephyr_persistence_survives, zephyr_verdict, zephyr_wire_follows,
};
use takt_conformance::board::stm32f401::{Stm32f401, Stm32f401C};
use takt_conformance::board::{self, Board, Flaw, Form, Options};
use takt_conformance::zephyr::Variant;

/// Was der F401 nicht fasst (Pruefung 39): `45_journal_cut` und
/// `110_journal_log` halten ein Flash-Modell mit seinen Sektoren im RAM, das
/// im Build `sim` mitlaeuft; `.bss` laeuft um 47 und 80 KiB ueber die 64 KiB
/// des Chips. Auf dem C6 laufen beide.
const TOO_BIG: &[&str] = &["45_journal_cut.takt", "110_journal_log.takt"];

/// Ein Board, mehrere Tests: cargo fuehrt Tests nebenlaeufig aus, das Board
/// und sein Port vertragen nur einen Lauf zugleich.
static BOARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Das Board, exklusiv. Die Tests laufen nur mit `--ignored`; wer sie
/// verlangt, verlangt das Board, und ohne `TAKT_F401_PORT` scheitern sie.
/// Im Lauf, der nur baut, ist es eines ohne Port.
fn board() -> Option<(Stm32f401, std::sync::MutexGuard<'static, ()>)> {
    let board = Stm32f401::from_env().expect("TAKT_F401_PORT nennt kein Board");
    Some((board, BOARD.lock().unwrap_or_else(std::sync::PoisonError::into_inner)))
}

/// **`next_run = AFTER(delay)` setzt nach der Dauer fort, und der neue Lauf
/// weiss es** (12.7, FB-309): Der erste Lauf endet nach 300 ms mit
/// `AFTER(delay = 2 s)`, das Board schlaeft im Standby, der zweite Lauf
/// beginnt mit `previous_run = ENDED` und zeigt es an `woke`. Ein freier Lauf
/// in Echtzeit: Der Schlaf ist physisch und laesst sich nicht logisch zaehlen.
#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
fn a_run_ended_after_a_delay_begins_the_next_after_it() {
    let Some((mut board, _guard)) = board() else { return };
    let options = Options::timed(0);
    let program = board::root().join("crates/takt-conformance/tests/programs/deep_sleep.takt");
    let elf = board.build(&program, &options).unwrap_or_else(|e| panic!("{e}"));
    let Some(first) = ran(board.run(&elf, &options)) else { return };
    assert!(first.contains("end after"), "der erste Lauf endet mit `next_run`:\n{first}");
    let quiet = board.listen(Duration::from_millis(1200)).unwrap_or_else(|e| panic!("{e}"));
    assert!(!quiet.contains("takt auf stm32f401"), "zu frueh geweckt:\n{quiet}");
    let second = board.listen(Duration::from_secs(6)).unwrap_or_else(|e| panic!("{e}"));
    assert!(second.contains("takt auf stm32f401"), "kein neuer Lauf nach der Dauer:\n{second}");
    assert!(second.contains("out woke true"), "der zweite Lauf beginnt mit `previous_run = ENDED`:\n{second}");
}

/// **`next_run = NOW` beginnt sofort den naechsten Lauf, und der weiss es**
/// (12.7): Der erste Lauf endet nach 300 ms; das Board setzt sich zurueck,
/// und der zweite beginnt mit `previous_run = ENDED` und zeigt es eine
/// Sekunde spaeter an `again`, wenn der Wirt die Leitung wieder offen hat.
/// Der erste kennt nach dem Flashen keinen vorigen Lauf. Ein freier Lauf in
/// Echtzeit, wie beim Schlaf.
#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
fn a_run_ended_now_begins_the_next_at_once() {
    let Some((mut board, _guard)) = board() else { return };
    let options = Options::timed(0);
    let program = board::root().join("crates/takt-conformance/tests/programs/restart.takt");
    let elf = board.build(&program, &options).unwrap_or_else(|e| panic!("{e}"));
    let Some(first) = ran(board.run(&elf, &options)) else { return };
    assert!(first.contains("end now"), "der erste Lauf endet mit `next_run`:\n{first}");
    assert!(first.contains("out previous NONE"), "nach dem Flashen kein voriger Lauf:\n{first}");
    let second = board.listen(Duration::from_secs(4)).unwrap_or_else(|e| panic!("{e}"));
    assert!(second.contains("out again true"), "der zweite Lauf beginnt mit `ENDED`:\n{second}");
    assert!(second.contains("out previous ENDED"), "{second}");
}

/// **`persist` ueberlebt einen Reset und ein neues Abbild** (5.9, 12.3,
/// FB-385): Der zweite Lauf geht ueber den DFU-Bootloader, also ueber einen
/// Reset, und schreibt dasselbe Abbild noch einmal nach Sektor 4; das
/// Journal in Sektor 2 und 3 bleibt stehen.
#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
fn persistence_survives_a_reset_and_a_new_image() {
    let Some((mut board, _guard)) = board() else { return };
    let failed = persistence_survives(&mut board, |board, elf, options| board.run(elf, options));
    verdict(&failed);
}

/// **Unter `overrun = alert` kostet das Journal Zeit, aber keine
/// Semantik** (7.3, 12.3, FB-385): Eine Sektorloeschung haelt den Kern
/// Hunderte Millisekunden an; die Tickzahl kommt aus TIM2.
#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
fn the_journal_costs_time_but_not_semantics() {
    let Some((mut board, _guard)) = board() else { return };
    let failed = journal_costs_time_but_not_semantics(&mut board);
    verdict(&failed);
}

/// **Ein Stillstand ist Rueckstand und wird nachgeholt** (7.3, FB-742):
/// Eine Sektorloeschung haelt den Kern rund 200 ms an, und die Uhr des
/// eigenen Kerns verliert die Perioden nicht, denn sie liest die Zeitachse
/// von TIM2.
#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
fn a_stall_is_lag_and_caught_up() {
    let Some((mut board, _guard)) = board() else { return };
    let failed = common::board::a_stall_is_lag_and_caught_up(&mut board);
    verdict(&failed);
}

/// **Unter `overrun = fault` schreibt das Journal im Schlaf** (9.9, 12.3,
/// FB-385), mit den Zeiten aus `corpus-try/hw/stm32f401.hw`.
#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
fn the_journal_writes_in_sleep_windows() {
    let Some((mut board, _guard)) = board() else { return };
    let failed = journal_writes_in_sleep_windows(&mut board);
    verdict(&failed);
}

/// **Ein Job, der laenger rechnet als ein Tick, verspaetet keinen** (4.5,
/// 12.3, M10 Schritt 7): SHA-256 ueber 4096 Byte im Job-Faden, den jeder
/// Tick unterbricht (`long_job_keeps_the_tick`).
#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
fn a_long_job_runs_between_the_ticks() {
    let Some((mut board, _guard)) = board() else { return };
    let failed = long_job_keeps_the_tick(&mut board);
    verdict(&failed);
}

/// **Ein Ueberlauf faultet im naechsten Tick jede Maschine** (7.3, 5.4,
/// FB-332): `overrun_reaches_every_machine`.
#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
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
#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
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
    assert!(runs.iter().all(|r| r.contains("out rested true")), "jeder Lauf schlief vor dem Reset:\n{text}");
}

/// **Das Board kommt ohne Hand zurueck** (FB-275): Zwei Laeufe
/// hintereinander, und der zweite braucht den Bootloader, den der erste
/// auf `TAKT` freigibt.
#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
fn the_board_hands_itself_back_for_the_next_program() {
    let Some((mut board, _guard)) = board() else { return };
    let name = "01_minimal.takt";
    let options = Options::fresh(TICKS);
    let elf = board.build(&board::corpus_path(name), &options).unwrap_or_else(|e| panic!("{name}: {e}"));
    let Some(first) = ran(board.run(&elf, &options).map_err(|e| format!("{name}: {e}"))) else { return };
    let second = board.run(&elf, &options).unwrap_or_else(|e| panic!("{name}, zweiter Lauf: {e}"));
    let outs = |t: &str| {
        t.lines().filter(|l| l.starts_with("t=") && l.contains(" out ")).map(str::to_string).collect::<Vec<_>>()
    };
    assert!(!outs(&first).is_empty(), "keine Ausgaben:\n{first}");
    assert_eq!(outs(&first), outs(&second));
    assert!(last_output(&second, "vent").is_some(), "{second}");
}

/// **Die Region des Baus ist die Region des Boards** (12.3): Was
/// `takt build --emit embed` aus `protect = armv7m_mpu` an Groesse und
/// geschuetztem Bereich rechnet (`takt_mir::hardware::Protect`), richtet das
/// Board-Crate in der MPU ein (`takt_board_support::mpu::Region`); beide
/// Rechnungen stimmen fuer jede Groesse des Programmbereichs ueberein.
#[test]
fn the_mpu_window_of_the_build_is_the_region_of_the_board() {
    for bytes in (0..=70_000u32).step_by(7).chain([255, 256, 257, 511, 512, 513, 65_536]) {
        let window = takt_mir::hardware::Protect::Armv7mMpu.window(u64::from(bytes)).expect("Region");
        let region = takt_board_support::mpu::Region::covering(0, bytes).expect("an 0 ausgerichtet");
        assert_eq!((window.size, window.protected), (u64::from(region.size), u64::from(region.protected())), "{bytes}");
    }
}

/// **`TOO_BIG` ist, was Pruefung 39 fuer den F401 ablehnt** (11.5,
/// FB-442): je Programm der Board-Suite im Build `sim`, wie das Board es
/// baut, gegen `ram` und `flash` aus `corpus-try/hw/stm32f401.hw`. Ein
/// neues Programm, das nicht passt, faellt so am Wirt auf statt im
/// Boardlauf, und ein Eintrag, der wieder passt, ebenso.
#[test]
fn the_programs_too_big_for_the_board_are_those_check_39_rejects() {
    let text = std::fs::read_to_string(board::root().join("corpus-try/hw/stm32f401.hw")).expect("hw lesbar");
    let hw = takt_mir::hardware::parse(&text).unwrap_or_else(|e| panic!("{}: {}", e.line, e.message));
    let target = hw.target("thumbv7em").expect("Ziel thumbv7em");
    let rejected: Vec<&str> = board::corpus()
        .into_iter()
        .filter(|name| {
            let p = common::board::program(&board::corpus_path(name));
            takt_sema::calibrated::check(&p, target, takt_diag::Span::default()).iter().any(|d| d.code == "SC-39")
        })
        .collect();
    assert_eq!(rejected, TOO_BIG);
}

#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
fn the_board_agrees_with_the_interpreter() {
    let Some((mut board, _guard)) = board() else { return };
    // `TAKT_F401_ONLY=42_map.takt` fuer einen einzelnen Fall.
    let only = std::env::var("TAKT_F401_ONLY").ok();
    let names: Vec<&str> = board::corpus().into_iter().filter(|n| !TOO_BIG.contains(n)).collect();
    let failed = agreement(&mut board, &names, only.as_deref());
    verdict(&failed);
}

/// **Unter RTIC rechnet der Korpus wie der Interpreter** (12.8, M10
/// Schritt 16): Takt als hoechstpriore Aufgabe, darueber eine Funk-ISR,
/// darunter eine Treiber-Aufgabe mit kritischen Abschnitten und die Jobs.
/// Die Semantik bleibt die des Programms; nur die Zeit wird gemessen statt
/// bewiesen.
#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
fn the_board_agrees_with_the_interpreter_under_rtos() {
    let Some((mut board, _guard)) = board() else { return };
    let only = std::env::var("TAKT_F401_ONLY").ok();
    let names: Vec<&str> = board::corpus().into_iter().filter(|n| !TOO_BIG.contains(n) && runs_shared(n)).collect();
    let failed = agreement_with(&mut board, &names, only.as_deref(), &Options::fresh(TICKS).in_form(Form::Rtos));
    verdict(&failed);
}

/// **In der Interruptform rechnet der Korpus wie der Interpreter** (12.11,
/// M11 Schritt 14): Den Schritt rechnet die ISR von TIM2 zu der Frist, die
/// `service` nennt, Jobs der Job-Interrupt, und darunter laeuft eine fremde
/// Hauptschleife. In logischer Zeit gibt sie den Alarm erst frei, wenn das
/// System ruht.
#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
fn the_board_agrees_with_the_interpreter_in_the_interrupt_form() {
    let Some((mut board, _guard)) = board() else { return };
    let only = std::env::var("TAKT_F401_ONLY").ok();
    let names: Vec<&str> = board::corpus().into_iter().filter(|n| !TOO_BIG.contains(n) && runs_shared(n)).collect();
    let failed = agreement_with(&mut board, &names, only.as_deref(), &Options::fresh(TICKS).in_form(Form::Interrupt));
    verdict(&failed);
}

/// **Unter Embassy rechnet der Korpus wie der Interpreter** (12.11, M11
/// Schritt 14d): Takt als Aufgabe eines Executors hoher Prioritaet, die mit
/// `embassy-time` auf die Frist wartet; Jobs im Job-Interrupt, darunter eine
/// fremde Hauptschleife.
#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
fn the_board_agrees_with_the_interpreter_under_embassy() {
    let Some((mut board, _guard)) = board() else { return };
    let only = std::env::var("TAKT_F401_ONLY").ok();
    let names: Vec<&str> = board::corpus().into_iter().filter(|n| !TOO_BIG.contains(n) && runs_shared(n)).collect();
    let failed = agreement_with(&mut board, &names, only.as_deref(), &Options::fresh(TICKS).in_form(Form::Embassy));
    verdict(&failed);
}

/// **Unter Embassy beginnt der Schritt auf seiner Frist, neben einer fremden
/// Hauptschleife** (12.11, M11 Schritt 14d).
#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
fn the_embassy_task_steps_on_its_deadline_beside_a_foreign_main_loop() {
    let Some((mut board, _guard)) = board() else { return };
    let failed = the_form_keeps_its_deadline(&mut board, Form::Embassy);
    verdict(&failed);
}

/// **Die Takt-Aufgabe beginnt zweistellige Mikrosekunden nach der Grenze**
/// (12.8): `drift` ueber 3000 Ticks bei 1 ms, unter der Funk-ISR (alle
/// 577 us, je 20 us) und der Treiber-Aufgabe (jeder dritte Aufruf, je 30 us
/// kritischer Abschnitt). Die Last ist nicht an den Tick gebunden; ohne
/// Spanne haette der Test sie verfehlt.
/// Die Spanne vom Median zum spaetesten Tick ist der Jitter, den 12.8 zu
/// messen verlangt.
#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
fn the_rtos_task_starts_within_tens_of_microseconds() {
    let Some((mut board, _guard)) = board() else { return };
    let program = board::root().join("crates/takt-conformance/tests/programs/rtos_jitter.takt");
    let options = Options::timed(3000).in_form(Form::Rtos);
    let Some(text) = ran(board.build(&program, &options).and_then(|elf| board.run(&elf, &options))) else { return };
    let drift = Drift::of(&text);
    eprintln!("rtos: {} Ticks, Median {} ns, spaetester {} ns darueber", drift.ticks, drift.median, drift.late);
    assert!(drift.ticks >= 2000, "{} Zeitzeilen", drift.ticks);
    assert!(drift.late > 0, "die Last traf keine Tickgrenze");
    assert!(drift.late < 100_000, "ein Tick {} ns spaeter als im Mittel", drift.late);
    assert!(text.contains(" out count "), "keine Ausgaben");
}

/// **In der Interruptform beginnt der Schritt auf seiner Frist, neben einer
/// fremden Hauptschleife** (12.11).
#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
fn the_interrupt_form_steps_on_its_deadline_beside_a_foreign_main_loop() {
    let Some((mut board, _guard)) = board() else { return };
    let failed = the_form_keeps_its_deadline(&mut board, Form::Interrupt);
    verdict(&failed);
}

/// **In der Pollform rechnet der Korpus wie der Interpreter** (12.11, M11
/// Schritt 14): Die Hauptschleife des Bring-ups ruft `service`, sobald die
/// Frist erreicht ist, Jobs rechnen im Job-Interrupt. In logischer Zeit
/// oeffnet sie das Tor der Frist in jeder Runde, und der Job-Interrupt
/// rechnet, bevor sie weiterlaeuft. Programme, deren Jobs mit der Runde
/// ueber den Tick reichen, weist die Pollform beim Bau ab (M11 Schritt 15).
#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
fn the_board_agrees_with_the_interpreter_in_the_poll_form() {
    let Some((mut board, _guard)) = board() else { return };
    let only = std::env::var("TAKT_F401_ONLY").ok();
    let round = i64::try_from(main_loop_ns()).expect("eine Dauer");
    let names: Vec<&str> = board::corpus()
        .into_iter()
        .filter(|n| !TOO_BIG.contains(n) && runs_shared(n) && fits_the_poll_form(n, round))
        .collect();
    let failed = agreement_with(&mut board, &names, only.as_deref(), &Options::fresh(TICKS).in_form(Form::Poll));
    verdict(&failed);
}

/// Die laengste Runde der Hauptschleife, die die Hardware-Konfiguration des
/// Boards fuer die Pollform nennt (8.10).
fn main_loop_ns() -> u64 {
    let text = std::fs::read_to_string(board::root().join("corpus-try/hw/stm32f401.hw")).expect("hw lesbar");
    let hw = takt_mir::hardware::parse(&text).unwrap_or_else(|e| panic!("{}: {}", e.line, e.message));
    let ns = hw.target("thumbv7em").and_then(|t| t.host.main_loop_ns).expect("`main_loop_ns` im Ziel thumbv7em");
    u64::try_from(ns).expect("eine Dauer")
}

/// **In der Pollform kommt der Schritt binnen einer Runde der
/// Hauptschleife** (12.11): gegen `main_loop_ns` aus `stm32f401.hw`; der
/// Aufruf, der den Tick rechnet, darf 20 us dazugeben.
#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
fn the_poll_form_steps_within_a_round_of_its_main_loop() {
    let Some((mut board, _guard)) = board() else { return };
    let failed = the_poll_form_keeps_its_round(&mut board, main_loop_ns(), 20_000);
    verdict(&failed);
}

/// **Jeder Port des F401 mit unterbrechbarem Job-Kontext besteht die
/// Portpruefung** (12.11, 13.8; M11 Schritt 15): Interruptform, RTIC und
/// Embassy. Die Pollform weist das Pruefprogramm beim Bau ab: Ihr Schritt
/// unterbricht keinen Job (`port_test_cmd.rs`). Die MPU gehoert in jeder
/// Form Takt: `Schutz: Fenster` (M11 Schritt 23).
#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
fn every_port_passes_the_port_test() {
    let Some((mut board, _guard)) = board() else { return };
    let failed: Vec<String> = [Form::Interrupt, Form::Rtos, Form::Embassy]
        .into_iter()
        .flat_map(|f| the_port_passes(&mut board, f, Case::Window))
        .collect();
    verdict(&failed);
}

/// **Der Zephyr-Port besteht die Portpruefung auf dem F401** (12.11, 13.8;
/// M11 Schritt 19): `examples/zephyr` fuer `blackpill_f401cc` faehrt das
/// Pruefprogramm ueber den Port mit Faeden und seinen Adapter fuer Zephyr,
/// ab Sektor 4 wie jedes Abbild hinter dem Sprungbrett. Die Leitung ist
/// USART1 mit XON/XOFF, und auf `TAKT` springt das Beispiel in den
/// DFU-Bootloader: Das naechste Programm schreibt der Lauf ohne Hand. Braucht
/// ausser dem Board Zephyr (`TAKT_WEST` und die Umgebung aus
/// `D:\takt-tools\activate.ps1`) und `<target>/release/takt`.
#[test]
#[ignore = "Board: TAKT_F401_PORT; Zephyr: TAKT_WEST; mit --ignored"]
fn the_zephyr_port_passes_the_port_test() {
    let Some((mut board, _guard)) = board() else { return };
    let options = Options::timed(takt_conformance::port::TICKS).judged();
    let example = takt_conformance::zephyr::ZEPHYR;
    let run = zephyr_image(board.name(), example, takt_conformance::zephyr::STM32F401, Variant::FRESH)
        .and_then(|image| board.run(&image.elf, &options));
    verdict(&zephyr_verdict(board.name(), example, Case::Partition, Some(ZEPHYR_COUNTER_CLOCK), run));
}

/// **`persist` ueberlebt unter Zephyr einen Neustart des F401 und ein neues
/// Abbild** (5.9; M11 Schritt 19, FB-732): `examples/zephyr` ohne frisches
/// Journal, zweimal geschrieben und gestartet; das Journal liegt in Sektor 2
/// und 3, die das Schreiben eines Abbilds nicht beruehrt.
#[test]
#[ignore = "Board: TAKT_F401_PORT; Zephyr: TAKT_WEST; mit --ignored"]
fn the_zephyr_journal_survives_a_reset_and_a_new_image() {
    let Some((mut board, _guard)) = board() else { return };
    let options = Options::timed(takt_conformance::port::TICKS).judged();
    let image = match zephyr_image(
        board.name(),
        takt_conformance::zephyr::ZEPHYR,
        takt_conformance::zephyr::STM32F401,
        Variant::KEPT,
    ) {
        Ok(image) => image,
        Err(e) => return verdict(&[e]),
    };
    let failed = zephyr_persistence_survives(
        &mut board,
        &image,
        |board, image| board.run(&image.elf, &options),
        |board, image| board.run(&image.elf, &options),
    );
    verdict(&failed);
}

/// **Der CMSIS-RTOS2-Port besteht die Portpruefung auf dem F401** (12.11,
/// 13.8; M11 Schritt 19c): `examples/cmsis-rtos2` faehrt das Pruefprogramm
/// ueber den Port mit Faeden und seinen generischen Adapter auf CMSIS-RTOS2,
/// unter Zephyrs CMSIS-Schicht; Leitung, Lage und Rueckgabe des Boards wie
/// beim Zephyr-Port. Braucht ausser dem Board Zephyr (`TAKT_WEST` und die
/// Umgebung aus `D:\takt-tools\activate.ps1`) und `<target>/release/takt`.
/// CMSIS-RTOS2 kennt keine Partition: `Schutz: keiner` (M11 Schritt 23).
#[test]
#[ignore = "Board: TAKT_F401_PORT; Zephyr: TAKT_WEST; mit --ignored"]
fn the_cmsis_rtos2_port_passes_the_port_test() {
    let Some((mut board, _guard)) = board() else { return };
    let options = Options::timed(takt_conformance::port::TICKS).judged();
    let example = takt_conformance::zephyr::CMSIS_RTOS2;
    let run = zephyr_image(board.name(), example, takt_conformance::zephyr::STM32F401, Variant::FRESH)
        .and_then(|image| board.run(&image.elf, &options));
    verdict(&zephyr_verdict(board.name(), example, Case::Unprotected, None, run));
}

/// **Ein Port, der die FPU-Register nicht sichert, faellt durch** (12.11,
/// M11 Schritt 15): Die Interruptform ohne verzoegertes Sichern; die fremde
/// Last rechnet falsch, sobald der Schritt sie unterbricht.
#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
fn a_port_without_fpu_saving_fails_the_port_test() {
    let Some((mut board, _guard)) = board() else { return };
    let failed = the_flawed_port_fails(&mut board, Flaw::UnsavedFpu, &["FPU-Kontext nicht gesichert"], &[]);
    verdict(&failed);
}

/// **Ein Port, der Jobs nicht unterbrechen laesst, faellt durch** (4.5,
/// 12.11, M11 Schritt 15): Die Jobs mit der Prioritaet des Schritts; er
/// wartet auf jeden, und die Ticks kommen zu spaet. Danach holt er die
/// Ticks am Stueck nach, und ihr Trace kann den Ring vor der Leitung
/// ueberlaufen (FB-292): dieselbe Ursache, kein eigener Befund.
#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
fn a_port_with_uninterruptible_jobs_fails_the_port_test() {
    let Some((mut board, _guard)) = board() else { return };
    let failed = the_flawed_port_fails(&mut board, Flaw::UninterruptibleJobs, &["7.3", "Tickgrenze"], &["verwarf"]);
    verdict(&failed);
}

/// **Zwei Jobs desselben Ticks sind im naechsten fertig**, in jeder Form des
/// Boards (4.5, 12.11, FB-512): Nach dem ersten Auftrag verteilt der
/// Schrittkontext den zweiten, bevor die Grenze kommt.
#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
fn simultaneous_jobs_finish_on_time_on_the_board() {
    let Some((mut board, _guard)) = board() else { return };
    let failed: Vec<String> = [Form::Own, Form::Interrupt, Form::Poll, Form::Rtos, Form::Embassy]
        .into_iter()
        .flat_map(|f| simultaneous_jobs_finish_on_time(&mut board, f))
        .collect();
    verdict(&failed);
}

/// **Der Port der Interruptform in C besteht die Portpruefung** (12.11,
/// 13.8; M11 Schritt 18b): der C-Wirt der Black Pill mit dem SDK-Archiv,
/// `ports/irq` und seinem Adapter fuer STM32F4. Den Schritt rechnet die ISR
/// von TIM2, die Jobs PendSV, darunter rechnet die Hauptschleife die fremde
/// Last mit der FPU. Die MPU bewacht nur den Stack; ein Fenster um den
/// Schritt oeffnet der Port nicht (FB-755): `Schutz: keiner`.
#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
fn the_c_interrupt_port_passes_the_port_test() {
    let Some((board, _guard)) = board() else { return };
    let failed = the_port_passes(&mut Stm32f401C::on(board), Form::Interrupt, Case::Unprotected);
    verdict(&failed);
}

/// **Im Port der Interruptform in C rechnet die Korpusauswahl wie der
/// Interpreter** (12.11, Satz 9.4.4; M11 Schritt 18b): jedes Programm des
/// Board-Korpus, das unter `shared` laeuft und keinen Treiber bindet — der
/// C-Wirt stellt keine —, in logischer Zeit hinter dem Tor der
/// Hauptschleife und mit verlustfreier Leitung.
#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
fn the_c_interrupt_port_agrees_with_the_interpreter() {
    let Some((board, _guard)) = board() else { return };
    let only = std::env::var("TAKT_F401_ONLY").ok();
    let names: Vec<&str> =
        board::corpus().into_iter().filter(|n| !TOO_BIG.contains(n) && runs_shared(n) && binds_no_driver(n)).collect();
    let options = Options::fresh(TICKS).in_form(Form::Interrupt);
    let failed = agreement_with(&mut Stm32f401C::on(board), &names, only.as_deref(), &options);
    verdict(&failed);
}

/// **Der Port mit Faeden unter FreeRTOS besteht die Portpruefung** (12.11,
/// 13.8; M11 Schritt 20): der C-Wirt der Black Pill mit dem SDK-Archiv,
/// `ports/rtos`, seinem Adapter `ports/freertos` und dem Kern von FreeRTOS
/// V11.3.1 (`ARM_CM4F`). Den Schritt rechnet eine Aufgabe hoechster
/// Prioritaet, die Jobs eine darunter, darunter rechnet eine fremde Aufgabe
/// des Wirts ihre Last mit der FPU. Ohne MPU-Port schuetzt die Arena
/// niemand: `Schutz: keiner`.
#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
fn the_freertos_port_passes_the_port_test() {
    let Some((board, _guard)) = board() else { return };
    let failed = the_port_passes(&mut Stm32f401C::on(board), Form::Rtos, Case::Unprotected);
    verdict(&failed);
}

/// **Unter FreeRTOS-MPU besteht der Port die Portpruefung hinter einer
/// Partition** (12.11, 13.8; M11 Schritt 20b): derselbe Wirt mit dem Port
/// `ARM_CM4_MPU`. Schritt und Jobs sind privilegierte Aufgaben, die fremde
/// Last rechnet ohne Privileg, und vor dem Lauf wies die MPU ihren
/// Schreibzugriff auf die Arena ab (`takt schutz partition verweigert`). Den
/// Stack der Interrupts bewacht ein Watchpoint, und kein Debugger haelt bei
/// einem Treffer an (`takt waechter watchpoint`); haelt einer an (die Probe
/// am SWD hat DHCSR.C_DEBUGEN gesetzt), faellt der Test, bis das Board ohne
/// Strom war.
#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
fn the_freertos_mpu_port_passes_the_port_test() {
    let Some((board, _guard)) = board() else { return };
    let header = [PARTITION_REFUSED, GUARD_WATCHPOINT];
    let failed = the_port_passes_with_header(&mut Stm32f401C::mpu(board), Form::Rtos, Case::Partition, &header);
    verdict(&failed);
}

/// **Unter FreeRTOS-MPU wird ein Schreibzugriff der fremden Last auf die
/// Arena `Runtime(Hardware)`** (12.3; M11 Schritt 20b): Die Aufgabe ohne
/// Privileg schreibt 20 ms nach dem Start ihrer Last hinein, die MPU weist
/// den Zugriff ab, der Handler von MemManage meldet ihn dem Kern
/// (`takt_core_hardware_fault`), und im naechsten Tick faulten die Maschinen.
#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
fn the_freertos_mpu_turns_a_foreign_write_into_runtime_hardware() {
    let Some((board, _guard)) = board() else { return };
    let failed = a_foreign_write_becomes_runtime_hardware(&mut Stm32f401C::mpu_probe(board));
    verdict(&failed);
}

#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
fn the_natives_agree_with_the_host() {
    let Some((mut board, _guard)) = board() else { return };
    let failed = natives_agree(&mut board);
    verdict(&failed);
}

/// **Eine verstellte FPU aendert nichts** (4.2, 12.11, M11 Schritt 6):
/// `a_hostile_fpu_changes_nothing`.
#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
fn a_hostile_fpu_changes_nothing_on_the_board() {
    let Some((mut board, _guard)) = board() else { return };
    let failed = a_hostile_fpu_changes_nothing(&mut board);
    verdict(&failed);
}

/// **Ein Abbild beginnt wie nach einem Reset, auch wenn sein Vorgaenger eine
/// Region der MPU stehen liess** (12.3, FB-752): Nach `leftover` belegt das
/// Sprungbrett bei der Rueckgabe die letzte Region nur lesbar ueber dem Stack
/// und laesst die MPU aus, wie Zephyrs Rueckgabe es tat. Danach laufen ein
/// Rust-Abbild der Form own und, nach einem zweiten `leftover`, der C-Wirt
/// ohne Schutzverletzung und mit einem Stack in der Schranke. Vorher schaltete
/// jedes die MPU mit der fremden Region wieder ein, und das Bemalen des Stacks
/// blieb an ihr haengen.
#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
fn a_region_left_by_the_previous_image_changes_nothing() {
    let Some((mut rust, _guard)) = board() else { return };
    // Ein Programm, das beide Wirte rechnen: ohne Treiber und ohne Profil.
    let name = board::corpus()
        .into_iter()
        .find(|n| !TOO_BIG.contains(n) && binds_no_driver(n) && corpus(n).config.runtime_profile().is_none())
        .expect("ein Programm fuer beide Wirte");
    let program = board::corpus_path(name);
    let (leftover, own, irq) =
        (Options::leftover(), Options::fresh(TICKS), Options::fresh(TICKS).in_form(Form::Interrupt));
    let mut c = Stm32f401C::on(Stm32f401::from_env().expect("TAKT_F401_PORT nennt kein Board"));
    let left = rust.build(&program, &leftover).unwrap_or_else(|e| panic!("{e}"));
    let rust_elf = rust.build(&program, &own).unwrap_or_else(|e| panic!("{e}"));
    let c_elf = c.build(&program, &irq).unwrap_or_else(|e| panic!("{e}"));
    let mut failed = Vec::new();
    failed.extend(rust.run(&left, &leftover).err().map(|e| format!("leftover: {e}")));
    failed.extend(after_leftover(&format!("{name}, Rust in der Form own"), rust.run(&rust_elf, &own)));
    failed.extend(rust.run(&left, &leftover).err().map(|e| format!("leftover: {e}")));
    failed.extend(after_leftover(&format!("{name}, C-Wirt"), c.run(&c_elf, &irq)));
    verdict(&failed);
}

/// Was ein Lauf nach `leftover` falsch machte: kein Lauf, der die Pruefungen
/// des Harness besteht, oder eine Verletzung des Speicherschutzes.
fn after_leftover(who: &str, run: Result<String, String>) -> Option<String> {
    match run {
        Ok(text) => text.lines().find(|l| l.starts_with("takt schutz verletzungen")).map(|l| format!("{who}: {l}")),
        Err(e) => Some(format!("{who}: {e}")),
    }
}

/// **Der Treiberrand urteilt auf dem Board wie im Interpreter** (12.6, M10
/// Schritt 29c): das Pruefgeraet des Bring-ups mit jedem Verstoss einmal.
#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
fn the_driver_edge_judges_like_the_interpreter() {
    let Some((mut board, _guard)) = board() else { return };
    let failed = driver_edge_agrees(&mut board);
    verdict(&failed);
}

/// **Ein Zeitgeber, der seine Periode verfehlt, ist `Runtime(Hardware)`**
/// (12.6 Zeile 7, M10 Schritt 29c): das Pruefgeraet streckt die Periode.
#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
fn a_stretched_tick_is_runtime_hardware() {
    let Some((mut board, _guard)) = board() else { return };
    let failed = common::board::a_stretched_tick_is_runtime_hardware(&mut board);
    verdict(&failed);
}

/// **`guard` aus der Konfiguration wirkt auf dem Board** (7.5, FB-331):
/// `a_schedule_inside_the_guard_is_a_timing_fault`, mit den Werten, die
/// `takt driver-test --board stm32f401` an der Bruecke PA0-PA1 gemessen hat.
#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
fn a_schedule_inside_the_guard_is_a_timing_fault() {
    let Some((mut board, _guard)) = board() else { return };
    let failed = common::board::a_schedule_inside_the_guard_is_a_timing_fault(&mut board);
    verdict(&failed);
}

/// **Ein Tune von der Konsole gilt ab seiner Grenze und weckt das Board**
/// (8.4, 9.9, FB-389): in Echtzeit, der Host schickt waehrend des Schlafs.
#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
fn a_tune_from_the_console_wakes_the_board() {
    let Some((mut board, _guard)) = board() else { return };
    let failed = common::board::a_tune_from_the_console_wakes_the_board(&mut board);
    verdict(&failed);
}

/// **Eine Wake-Quelle weckt an der Grenze nach ihrem Ereignis** (9.9,
/// FB-388): Das Pruefgeraet hebt einen Pegel und laeutet eine Klingel, die
/// das Programm nicht vorher kennt.
#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
fn a_wake_source_ends_the_sleep() {
    let Some((mut board, _guard)) = board() else { return };
    let failed = common::board::a_wake_source_ends_the_sleep(&mut board);
    verdict(&failed);
}

/// **Was das Programm nicht liest, zeichnet der Rahmen auf** (8.2, 12.5,
/// M10 Schritt 29d): das Pruefgeraet liefert Kanaele ohne Bindung.
#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
fn the_unread_channels_are_recorded() {
    let Some((mut board, _guard)) = board() else { return };
    let failed = common::board::unread_channels_are_recorded(&mut board);
    verdict(&failed);
}

/// **Die MPU weist einen Zugriff der TCB ab und meldet ihn** (12.3, M10
/// Schritt 18). Nach Tick 2 schreibt ein Pruefgeraet zwischen zwei Ticks in
/// den Programmzustand, in den Waechter unter dem Hauptstack oder in den
/// unter dem Job-Stack; oder eine ISR schreibt im naechsten Schritt, bei
/// offenem Programmzustand, hinein. Die MPU weist jeden Zugriff ab, der
/// Handler uebergeht ihn, und im Tick danach bekommen alle Maschinen
/// `Runtime(Hardware)`; die Bilanz nennt die Region. Ohne Schutz haette
/// ein Zugriff auf den Programmzustand das Abbild ueberschrieben.
///
/// **Das Fenster in jeder Form** (12.11; M11 Schritt 23): Der Zugriff
/// zwischen zwei Ticks laeuft in jeder Form des Boards, der Kopf nennt
/// `Schutz: Fenster`. Die uebrigen Faelle haengen nicht an der Form und
/// laufen im eigenen Kern.
#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
fn the_mpu_turns_a_write_of_the_tcb_into_runtime_hardware() {
    let Some((mut board, _guard)) = board() else { return };
    let every = board.forms();
    let mut failed = Vec::new();
    for (which, region, tick, forms) in [
        ("tcb", "Programmzustand", 3, every),
        ("guard", "Waechter", 3, &[Form::Own][..]),
        ("job_guard", "Waechter", 3, &[Form::Own][..]),
        ("isr", "Programmzustand", 4, &[Form::Own][..]),
    ] {
        let program = board::root().join(format!("crates/takt-conformance/tests/programs/protect_{which}.takt"));
        for &form in forms {
            let options = Options::fresh(20).in_form(form);
            let Some(text) = ran(board.build(&program, &options).and_then(|elf| board.run(&elf, &options))) else {
                continue;
            };
            let found = window_held(&text, tick, region);
            failed.extend(
                found.into_iter().chain(header_names(&text, Case::Window)).map(|f| format!("{which} {form:?}: {f}")),
            );
        }
    }
    verdict(&failed);
}

/// Was ein abgewiesener Zugriff hinterlaesst: `Runtime(Hardware)` und der
/// Fault der Maschine im Tick `tick`, eine Verletzung der Region `region` in
/// der Bilanz; je Fehlendes ein Befund mit dem Lauf.
fn window_held(text: &str, tick: u32, region: &str) -> Vec<String> {
    let mut failed = Vec::new();
    if !text.contains(&format!("t={tick} runtime Hardware")) {
        failed.push(format!("kein Runtime(Hardware) im Tick {tick} nach dem Zugriff"));
    }
    if !text.lines().any(|l| l.starts_with(&format!("t={tick} fault m"))) {
        failed.push("die Maschine faultet nicht".to_string());
    }
    match text.lines().find(|l| l.starts_with("takt schutz verletzungen ")) {
        Some(line) if line.contains(&format!("verletzungen 1 region {region}")) => {}
        Some(line) => failed.push(format!("die Bilanz nennt `{line}`")),
        None => failed.push("keine Bilanz des Speicherschutzes".to_string()),
    }
    if !failed.is_empty() {
        failed.push(text.to_string());
    }
    failed
}

/// **Die Pins aus dem Devicetree treiben und lesen die Drahtbruecke des
/// F401** (8.10, 12.11; M11 Schritt 19): `examples/zephyr-gpio` mit den
/// Treibern aus `wire_zephyr.c` auf PA0 und PA1; jeder Tick liest den Pegel
/// des Ticks davor.
#[test]
#[ignore = "Board: TAKT_F401_PORT; Zephyr: TAKT_WEST; mit --ignored"]
fn the_zephyr_gpio_wire_follows_its_pin() {
    let Some((mut board, _guard)) = board() else { return };
    let options = Options::timed(ZEPHYR_WIRE_TICKS).judged();
    let example = takt_conformance::zephyr::ZEPHYR_GPIO;
    let run = zephyr_image(board.name(), example, takt_conformance::zephyr::STM32F401, Variant::KEPT)
        .and_then(|image| board.run(&image.elf, &options));
    verdict(&zephyr_wire_follows(board.name(), run));
}

/// **Eine Schutzverletzung im Lauf wird auf dem F401 zum Fault des Kerns**
/// (12.3; M11 Schritt 19): `examples/zephyr` mit `TAKT_PROBE_IN_RUN`; die
/// MPU weist die zweite Probe ab, und der Kern meldet `Runtime(Hardware)`.
#[test]
#[ignore = "Board: TAKT_F401_PORT; Zephyr: TAKT_WEST; mit --ignored"]
fn the_zephyr_partition_turns_an_access_in_the_run_into_a_fault() {
    let Some((mut board, _guard)) = board() else { return };
    let options = Options::timed(takt_conformance::port::TICKS).judged();
    let example = takt_conformance::zephyr::ZEPHYR;
    let run = zephyr_image(board.name(), example, takt_conformance::zephyr::STM32F401, Variant::PROBE_IN_RUN)
        .and_then(|image| board.run(&image.elf, &options));
    verdict(&zephyr_access_in_the_run_faults(board.name(), run));
}

/// **Zwei Programme auf einem Kern** (12.11, M11 Schritt 16): `pair_fast`
/// (1 kHz) und `pair_slow` (100 Hz) im Binary `pair`, gekoppelt ueber die
/// Mailbox in beide Richtungen; beide Traces gleich dem Interpreter, keine
/// Antwort laenger als berichtet ([`two_programs_share_a_core`]).
#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
fn two_programs_share_the_core() {
    let Some((mut board, _guard)) = board() else { return };
    verdict(&two_programs_share_a_core(&mut board, "pair_fast.takt"));
}

/// **Kein Programm schreibt in die Arena des anderen** (12.3, 12.11, M11
/// Schritt 16): Die MPU weist den Zugriff ab, und nur das schreibende
/// Programm bekommt `Runtime(Hardware)` ([`a_write_into_the_other_arena_is_refused`]).
#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
fn a_program_cannot_write_into_the_other_arena() {
    let Some((mut board, _guard)) = board() else { return };
    verdict(&a_write_into_the_other_arena_is_refused(&mut board));
}
