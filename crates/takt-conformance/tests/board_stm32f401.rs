//! Board 1, STM32F401: der Korpus auf dem Chip gegen den Interpreter
//! (13.8, Satz 9.4.4; plan/f401.md).
//!
//! **Laeuft nur mit Board.** `TAKT_F401_PORT` nennt den Port des Adapters
//! an USART1 (`COM7`), `TAKT_DFU_UTIL` den Pfad zu `dfu-util`, wenn er
//! nicht im `PATH` steht; ohne Port wird der Test uebersprungen. Das Board
//! braucht keine Hand: Jedes Programm gibt es auf `TAKT` an den
//! DFU-Bootloader zurueck (FB-275).

mod common;

use common::board::{TICKS, agreement, last_output, natives_agree};
use takt_conformance::board::stm32f401::Stm32f401;
use takt_conformance::board::{self, Board, CORPUS, Options};

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
