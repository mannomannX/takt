//! Board 1, STM32F401: der Korpus auf dem Chip gegen den Interpreter
//! (13.8, Satz 9.4.4; plan/f401.md).
//!
//! **Laeuft nur mit Board.** `TAKT_F401_PORT` nennt den Port des Adapters
//! an USART1 (`COM7`); ohne Port wird der Test uebersprungen. Das Board
//! braucht keine Hand: Jedes Programm gibt es auf `TAKT` an den
//! DFU-Bootloader zurueck (FB-275).

mod common;

use std::time::Duration;

use common::board::{
    Drift, TICKS, a_hostile_fpu_changes_nothing, agreement, agreement_with, driver_edge_agrees, last_output,
    long_job_keeps_the_tick, natives_agree, overrun_reaches_every_machine, runs_shared,
    simultaneous_jobs_finish_on_time, the_interrupt_form_keeps_its_deadline,
};
use takt_conformance::board::stm32f401::Stm32f401;
use takt_conformance::board::{self, Board, Form, Options};

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
    let first = board.run(&elf, &options).unwrap_or_else(|e| panic!("{e}"));
    assert!(first.contains("end after"), "der erste Lauf endet mit `next_run`:\n{first}");
    let quiet = board.listen(Duration::from_millis(1200)).unwrap_or_else(|e| panic!("{e}"));
    assert!(!quiet.contains("takt auf stm32f401"), "zu frueh geweckt:\n{quiet}");
    let second = board.listen(Duration::from_secs(6)).unwrap_or_else(|e| panic!("{e}"));
    assert!(second.contains("takt auf stm32f401"), "kein neuer Lauf nach der Dauer:\n{second}");
    assert!(second.contains("out woke 1"), "der zweite Lauf beginnt mit `previous_run = ENDED`:\n{second}");
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
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
fn a_long_job_runs_between_the_ticks() {
    let Some((mut board, _guard)) = board() else { return };
    let failed = long_job_keeps_the_tick(&mut board);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

/// **Ein Ueberlauf faultet im naechsten Tick jede Maschine** (7.3, 5.4,
/// FB-332): `overrun_reaches_every_machine`.
#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
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
#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
fn a_missed_kick_begins_the_next_run_after_the_watchdog() {
    let Some((mut board, _guard)) = board() else { return };
    let options = Options::timed(0);
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
    let first = board.run(&elf, &options).unwrap_or_else(|e| panic!("{name}: {e}"));
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
    assert!(failed.is_empty(), "{}", failed.join("\n\n"));
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
    assert!(failed.is_empty(), "{}", failed.join("\n\n"));
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
    assert!(failed.is_empty(), "{}", failed.join("\n\n"));
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
    let text =
        board.build(&program, &options).and_then(|elf| board.run(&elf, &options)).unwrap_or_else(|e| panic!("{e}"));
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
    let failed = the_interrupt_form_keeps_its_deadline(&mut board);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

/// **Zwei Jobs desselben Ticks sind im naechsten fertig**, im eigenen Kern
/// und in der Interruptform (4.5, 12.11). Unter RTIC gibt die Takt-Aufgabe
/// einen Job je Grenze aus (FB-512, Schritt 14c).
#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
fn simultaneous_jobs_finish_on_time_on_the_board() {
    let Some((mut board, _guard)) = board() else { return };
    let failed: Vec<String> = [Form::Own, Form::Interrupt]
        .into_iter()
        .flat_map(|f| simultaneous_jobs_finish_on_time(&mut board, f))
        .collect();
    assert!(failed.is_empty(), "{}", failed.join("\n\n"));
}

#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
fn the_natives_agree_with_the_host() {
    let Some((mut board, _guard)) = board() else { return };
    let failed = natives_agree(&mut board);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

/// **Eine verstellte FPU aendert nichts** (4.2, 12.11, M11 Schritt 6):
/// `a_hostile_fpu_changes_nothing`.
#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
fn a_hostile_fpu_changes_nothing_on_the_board() {
    let Some((mut board, _guard)) = board() else { return };
    let failed = a_hostile_fpu_changes_nothing(&mut board);
    assert!(
        failed.is_empty(),
        "{}",
        failed.join(
            "
"
        )
    );
}

/// **Der Treiberrand urteilt auf dem Board wie im Interpreter** (12.6, M10
/// Schritt 29c): das Pruefgeraet des Bring-ups mit jedem Verstoss einmal.
#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
fn the_driver_edge_judges_like_the_interpreter() {
    let Some((mut board, _guard)) = board() else { return };
    let failed = driver_edge_agrees(&mut board);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

/// **Ein Zeitgeber, der seine Periode verfehlt, ist `Runtime(Hardware)`**
/// (12.6 Zeile 7, M10 Schritt 29c): das Pruefgeraet streckt die Periode.
#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
fn a_stretched_tick_is_runtime_hardware() {
    let Some((mut board, _guard)) = board() else { return };
    let failed = common::board::a_stretched_tick_is_runtime_hardware(&mut board);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

/// **`guard` aus der Konfiguration wirkt auf dem Board** (7.5, FB-331):
/// `a_schedule_inside_the_guard_is_a_timing_fault`, mit den Werten, die
/// `takt driver-test --board stm32f401` an der Bruecke PA0-PA1 gemessen hat.
#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
fn a_schedule_inside_the_guard_is_a_timing_fault() {
    let Some((mut board, _guard)) = board() else { return };
    let failed = common::board::a_schedule_inside_the_guard_is_a_timing_fault(&mut board);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

/// **Ein Tune von der Konsole gilt ab seiner Grenze und weckt das Board**
/// (8.4, 9.9, FB-389): in Echtzeit, der Host schickt waehrend des Schlafs.
#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
fn a_tune_from_the_console_wakes_the_board() {
    let Some((mut board, _guard)) = board() else { return };
    let failed = common::board::a_tune_from_the_console_wakes_the_board(&mut board);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

/// **Eine Wake-Quelle weckt an der Grenze nach ihrem Ereignis** (9.9,
/// FB-388): Das Pruefgeraet hebt einen Pegel und laeutet eine Klingel, die
/// das Programm nicht vorher kennt.
#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
fn a_wake_source_ends_the_sleep() {
    let Some((mut board, _guard)) = board() else { return };
    let failed = common::board::a_wake_source_ends_the_sleep(&mut board);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

/// **Was das Programm nicht liest, zeichnet der Rahmen auf** (8.2, 12.5,
/// M10 Schritt 29d): das Pruefgeraet liefert Kanaele ohne Bindung.
#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
fn the_unread_channels_are_recorded() {
    let Some((mut board, _guard)) = board() else { return };
    let failed = common::board::unread_channels_are_recorded(&mut board);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

/// **Die MPU weist einen Zugriff der TCB ab und meldet ihn** (12.3, M10
/// Schritt 18). Nach Tick 2 schreibt ein Pruefgeraet zwischen zwei Ticks in
/// den Programmzustand, in den Waechter unter dem Hauptstack oder in den
/// unter dem Job-Stack; oder eine ISR schreibt im naechsten Schritt, bei
/// offenem Programmzustand, hinein. Die MPU weist jeden Zugriff ab, der
/// Handler uebergeht ihn, und im Tick danach bekommen alle Maschinen
/// `Runtime(Hardware)`; die Bilanz nennt die Region. Ohne Schutz haette
/// ein Zugriff auf den Programmzustand das Abbild ueberschrieben.
#[test]
#[ignore = "Board: TAKT_F401_PORT; mit --ignored"]
fn the_mpu_turns_a_write_of_the_tcb_into_runtime_hardware() {
    let Some((mut board, _guard)) = board() else { return };
    for (which, region, tick) in [
        ("tcb", "Programmzustand", 3),
        ("guard", "Waechter", 3),
        ("job_guard", "Waechter", 3),
        ("isr", "Programmzustand", 4),
    ] {
        let program = board::root().join(format!("crates/takt-conformance/tests/programs/protect_{which}.takt"));
        let options = Options::fresh(20);
        let text =
            board.build(&program, &options).and_then(|elf| board.run(&elf, &options)).unwrap_or_else(|e| panic!("{e}"));
        assert!(
            text.contains(&format!("t={tick} runtime Hardware")),
            "{which}: kein Runtime(Hardware) im Tick nach dem Zugriff:\n{text}"
        );
        assert!(
            text.lines().any(|l| l.starts_with(&format!("t={tick} fault m"))),
            "{which}: die Maschine faultet nicht:\n{text}"
        );
        let line = text
            .lines()
            .find(|l| l.starts_with("takt schutz "))
            .unwrap_or_else(|| panic!("{which}: keine Bilanz:\n{text}"));
        assert!(line.contains(&format!("verletzungen 1 region {region}")), "{which}: {line}");
    }
}
