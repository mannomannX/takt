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
/// kommen, danach der zweite Lauf mit `woke` und `reset_count = 0`, denn
/// der Tiefschlaf war ein geordnetes Ende.
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
    assert!(second.contains("out count 0"), "ein Tiefschlaf ist ein geordnetes Ende (12.7):\n{second}");
}

/// **`reboot = RESTART` startet den Chip neu, und der neue Lauf weiss es**
/// (12.7): Der erste Lauf startet nach 300 ms neu; der zweite beginnt mit
/// `boot_reason = SOFTWARE` und zeigt es eine Sekunde spaeter an `again`,
/// wenn der Wirt die Leitung wieder offen hat. Beide Laeufe zeigen
/// `reset_count = 0` — das Einschalten und ein befohlener Neustart zaehlen
/// nicht — und `image_state = CONFIRMED`, denn ohne Startstufe gibt es ein
/// Image. Ein freier Lauf in Echtzeit, wie beim Tiefschlaf.
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
    for (run, text) in [("erste", &first), ("zweite", &second)] {
        assert!(text.contains("out count 0"), "der {run} Lauf zaehlt keinen Start (12.7):\n{text}");
        assert!(text.contains("out image CONFIRMED"), "ohne Startstufe ist das Image bestaetigt:\n{text}");
    }
}

/// **Ein ausgelassener Kick setzt zurueck, und der naechste Lauf weiss es**
/// (12.3, 12.7). Jeder Lauf zeigt Reset-Ursache und `reset_count`, schlaeft
/// eine Sekunde in `idle` und rechnet dann einen Tick lang weit ueber die
/// Frist des Watchdogs. Der erste Lauf beginnt beim Einschalten mit 0, die
/// beiden folgenden mit `WATCHDOG` und 1 und 2; der dritte bleibt stehen.
/// Dass jeder Lauf vor dem Reset `rested` zeigt, belegt den Schlaf: Die
/// Schleife bestaetigt den Watchdog je geschlafenem Tick.
#[test]
fn a_missed_kick_resets_and_counts() {
    let Some((mut board, _guard)) = board() else { return };
    let options = Options { ticks: 0, fresh: true, bin: Bin::Takt, timed: true };
    let program = board::root().join("crates/takt-conformance/tests/programs/watchdog.takt");
    let elf = board.build(&program, &options).unwrap_or_else(|e| panic!("{e}"));
    let text = board.run_for(&elf, Duration::from_secs(14)).unwrap_or_else(|e| panic!("{e}"));
    // Je Lauf ein Abschnitt ab der Marke; davor steht nur der Start.
    let runs: Vec<&str> = text.split(board::MARK).skip(1).collect();
    let first = |run: &str, name: &str| {
        let key = format!(" out {name} ");
        run.lines().find_map(|l| l.split_once(key.as_str()).map(|(_, v)| v.trim().to_string()))
    };
    let counts: Vec<String> = runs.iter().filter_map(|r| first(r, "count")).collect();
    assert_eq!(counts, ["0", "1", "2"], "ein Start mehr je Watchdog (12.7):\n{text}");
    let reasons: Vec<String> = runs.iter().filter_map(|r| first(r, "reason")).collect();
    assert_eq!(reasons.get(1..), Some(&["WATCHDOG".to_string(), "WATCHDOG".to_string()][..]), "{text}");
    assert!(runs.iter().all(|r| r.contains("out rested 1")), "jeder Lauf schlief vor dem Reset:\n{text}");
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
