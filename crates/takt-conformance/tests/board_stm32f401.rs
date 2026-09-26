//! Board 1, STM32F401: der Korpus auf dem Chip gegen den Interpreter
//! (13.8, Satz 9.4.4; plan/f401.md).
//!
//! **Laeuft nur mit Board.** `TAKT_F401_PORT` nennt den Port des Adapters
//! an USART1 (`COM7`), `TAKT_DFU_UTIL` den Pfad zu `dfu-util`, wenn er
//! nicht im `PATH` steht; ohne Port wird der Test uebersprungen. Das Board
//! braucht keine Hand: Jedes Programm gibt es auf `TAKT` an den
//! DFU-Bootloader zurueck (FB-275).

mod common;

use std::time::Duration;

use common::board::{TICKS, agreement, last_output, natives_agree};
use takt_conformance::board::stm32f401::Stm32f401;
use takt_conformance::board::{self, Bin, Board, CORPUS, Options};

/// Was der F401 nicht fasst: `45_journal_cut` haelt ein Flash-Modell mit
/// zwei Sektoren im RAM, und `.bss` laeuft um gut 47 KiB ueber die 64 KiB
/// des Chips (Pruefung 39); auf dem C6 laeuft es.
const TOO_BIG: &[&str] = &["45_journal_cut.takt"];

/// Ein Board, mehrere Tests: cargo fuehrt Tests nebenlaeufig aus, das Board
/// und sein Port vertragen nur einen Lauf zugleich.
static BOARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Das Board, exklusiv; `None` ohne `TAKT_F401_PORT`.
fn board() -> Option<(Stm32f401, std::sync::MutexGuard<'static, ()>)> {
    let Some(board) = Stm32f401::from_env() else {
        eprintln!("uebersprungen: TAKT_F401_PORT nennt kein Board");
        return None;
    };
    Some((board, BOARD.lock().unwrap_or_else(std::sync::PoisonError::into_inner)))
}

/// **`DEEP_SLEEP_FOR` schlaeft und weckt nach seiner Weckzeit** (12.7,
/// FB-309). Der erste Lauf geht nach 300 ms fuer zwei Sekunden in den
/// Tiefschlaf; der zweite beginnt mit `boot_reason = DEEP_SLEEP_WAKE` und
/// zeigt es an `woke`. Ein freier Lauf in Echtzeit: Nur dort fuehrt das
/// Board das Kommando aus, ein Konformitaetslauf endet mit dem Trace.
///
/// Die Leitung bleibt offen: In der ersten Sekunde darf kein neuer Start
/// kommen, danach der zweite Lauf mit `woke`.
#[test]
fn a_deep_sleep_ends_after_its_duration() {
    let Some((mut board, _guard)) = board() else { return };
    let options = Options { ticks: 0, fresh: true, bin: Bin::Takt, timed: true };
    let program = board::root().join("crates/takt-conformance/tests/programs/deep_sleep.takt");
    let elf = board.build(&program, &options).unwrap_or_else(|e| panic!("{e}"));
    let first = board.run(&elf, &options).unwrap_or_else(|e| panic!("{e}"));
    assert!(first.contains("end deep_sleep"), "der erste Lauf endet mit dem Kommando:\n{first}");
    let quiet = board.listen(Duration::from_millis(1200)).unwrap_or_else(|e| panic!("{e}"));
    assert!(!quiet.contains("takt auf stm32f401"), "zu frueh geweckt:\n{quiet}");
    let second = board.listen(Duration::from_secs(6)).unwrap_or_else(|e| panic!("{e}"));
    assert!(second.contains("takt auf stm32f401"), "kein neuer Start nach dem Tiefschlaf:\n{second}");
    assert!(second.contains("out woke 1"), "der zweite Lauf beginnt mit `DEEP_SLEEP_WAKE`:\n{second}");
}

/// **`reboot = RESTART` startet den Chip neu, und der neue Lauf weiss es**
/// (12.7): Der erste Lauf startet nach 300 ms neu; der zweite beginnt mit
/// `boot_reason = SOFTWARE` und zeigt es eine Sekunde spaeter an `again`,
/// wenn der Wirt die Leitung wieder offen hat. Ein freier Lauf in Echtzeit,
/// wie beim Tiefschlaf.
#[test]
fn a_restart_begins_again_with_software_as_the_reason() {
    let Some((mut board, _guard)) = board() else { return };
    let options = Options { ticks: 0, fresh: true, bin: Bin::Takt, timed: true };
    let program = board::root().join("crates/takt-conformance/tests/programs/restart.takt");
    let elf = board.build(&program, &options).unwrap_or_else(|e| panic!("{e}"));
    let first = board.run(&elf, &options).unwrap_or_else(|e| panic!("{e}"));
    assert!(first.contains("end restart"), "der erste Lauf endet mit dem Kommando:\n{first}");
    let second = board.listen(Duration::from_secs(4)).unwrap_or_else(|e| panic!("{e}"));
    assert!(second.contains("out again 1"), "der zweite Lauf beginnt mit `SOFTWARE`:\n{second}");
}

/// **Das Board kommt ohne Hand zurueck** (FB-275): Zwei Laeufe
/// hintereinander, und der zweite braucht den Bootloader, den der erste
/// auf `TAKT` freigibt.
#[test]
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

#[test]
fn the_board_agrees_with_the_interpreter() {
    let Some((mut board, _guard)) = board() else { return };
    // `TAKT_F401_ONLY=42_map.takt` fuer einen einzelnen Fall.
    let only = std::env::var("TAKT_F401_ONLY").ok();
    let names: Vec<&str> = CORPUS.iter().copied().filter(|n| !TOO_BIG.contains(n)).collect();
    let failed = agreement(&mut board, &names, only.as_deref());
    assert!(failed.is_empty(), "{}", failed.join("\n\n"));
}

#[test]
fn the_natives_agree_with_the_host() {
    let Some((mut board, _guard)) = board() else { return };
    let failed = natives_agree(&mut board);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}
