//! Der MCU-Rahmen auf dem Wirt (13.8, M10 Schritt 11): derselbe Rahmen,
//! dieselbe Schleife und derselbe Treiberrand wie auf den Boards, mit dem
//! Pruefgeraet `takt-driver-probe` als Treiber-Crate. Es sind dieselben
//! Pruefungen wie in `board_esp32c6.rs` und `board_stm32f401.rs`, ohne Board.
//!
//! Gebaut wird mit clang und dem Werkzeug `takt` des eigenen Profils, wie
//! `cargo test --workspace` es baut; ohne clang uebersprungen.

mod common;

use common::board::{agreement, driver_edge_agrees, unread_channels_are_recorded};
use takt_conformance::board::{self, Board, host::Host};

/// Der Wirt mit dem Pruefgeraet; ohne clang keiner.
fn probe() -> Option<Host> {
    common::clang_path()?;
    Some(Host::with_driver(board::root().join("crates/takt-driver-probe")))
}

/// **Ein Treiber-Crate auf dem Wirt wird geurteilt wie auf dem Board**
/// (12.6): `driver_edge_agrees`.
#[test]
fn the_driver_edge_judges_on_the_host_like_the_interpreter() {
    let Some(mut host) = probe() else { return };
    let failed = driver_edge_agrees(&mut host);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

/// **Was das Programm nicht liest, zeichnet der Rahmen auch auf dem Wirt
/// auf** (8.2, 12.5): `unread_channels_are_recorded`.
#[test]
fn the_unread_channels_are_recorded_on_the_host() {
    let Some(mut host) = probe() else { return };
    let failed = unread_channels_are_recorded(&mut host);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

/// **Der MCU-Rahmen rechnet den Korpus der Boards wie der Interpreter, auch
/// ohne Board** (9.4.4): derselbe Vergleich wie auf F401 und C6, ohne
/// Treiber-Crate. Der Bau kostet einige Sekunden je Programm, darum nur mit
/// `--ignored`; `TAKT_HOST_ONLY=42_map.takt` fuer einen einzelnen Fall.
#[test]
#[ignore = "langsam: der Korpus der Boards auf dem Wirt; mit --ignored"]
fn the_host_board_agrees_with_the_interpreter() {
    if probe().is_none() {
        return;
    }
    let only = std::env::var("TAKT_HOST_ONLY").ok();
    let failed = agreement(&mut Host::new(), &board::corpus(), only.as_deref());
    assert!(failed.is_empty(), "{}", failed.join("\n\n"));
}

/// **Ein unbekannter Name in `TAKT_…_ONLY` laesst den Lauf scheitern**
/// (KON1-027): Ein Tippfehler oder ein vergessenes `.takt` filterte sonst
/// jedes Programm weg, und der Vergleich bestuende ohne Lauf. Gebaut wird
/// dafuer nichts.
#[test]
fn an_unknown_only_name_fails_instead_of_passing() {
    let failed = agreement(&mut Host::new(), &board::corpus(), Some("42_map"));
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert!(failed[0].contains("`42_map`") && failed[0].contains("42_map.takt"), "{failed:?}");
}

/// **Ein Treiber-Crate stummelt nichts** (12.6, 12.11, GEN-021): Eine
/// Adresse, die weder das Crate noch das Bring-up verdrahtet — ein
/// Tippfehler `edge_a/q` statt `edge_a/p`, ein vergessener Heartbeat —,
/// scheitert vor dem Bau mit ihrem Namen; eine Adresse, die Crate und
/// Bring-up beide verdrahten, ebenso. Gebaut wird dafuer nichts.
#[test]
fn a_driver_crate_must_wire_every_address_once() {
    let dir = takt_conformance::target_dir().join(format!("takt-host-wiring-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("Verzeichnis");
    std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"wiring-probe\"\n").expect("schreibbar");
    let program = dir.join("program.takt");
    let src = "system:\n    language = 1\n    tick     = 10 ms\n\n\
               input  p : int in 0..100 @ hw(\"edge_a/p\")\n\
               output o : bool          @ hw(\"edge_o/o\") with safe = false\n\n\
               machine m:\n    initial RUN\n    state RUN:\n        loop:\n            o = p.valid\n";
    std::fs::write(&program, src).expect("schreibbar");
    let build = |wiring: &str| {
        std::fs::write(dir.join("takt-drivers.toml"), wiring).expect("schreibbar");
        Host::with_driver(dir.clone()).build(&program, &board::Options::fresh(4)).expect_err("kein Bau")
    };
    let complete = "\"edge_a/p\" = \"P\"\n\"edge_o/o\" = \"O\"\n\"edge_o\" = \"D\"\n";
    let typo = build(&complete.replace("edge_a/p", "edge_a/q"));
    assert!(typo.contains("`edge_a/p` ist nicht verdrahtet") && !typo.contains("`edge_o"), "{typo}");
    let beat = build(&complete.replace("\"edge_o\" = \"D\"\n", ""));
    assert!(beat.contains("`edge_o` ist nicht verdrahtet") && !beat.contains("`edge_a/p`"), "{beat}");
    let twice = build(&format!("{complete}\"sys/previous_run\" = \"S\"\n"));
    assert!(twice.contains("`sys/previous_run` verdrahtet schon") && twice.contains("takt-bringup-host"), "{twice}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// **Der Wirt stellt die Wanduhr** (7.4, 12.7; FB-415): `sys/clock` bedient
/// `takt_bringup_host::WallClock` aus der Verdrahtung des Bring-ups. Das
/// Programm liest den Kanal gueltig und mit einem Wert nach 2020; der
/// Interpreter ohne Stimulus hat keine Uhr, darum kein Vergleich mit ihm.
#[test]
fn the_host_serves_the_wall_clock() {
    if common::clang_path().is_none() {
        return;
    }
    let dir = takt_conformance::target_dir().join(format!("takt-host-clock-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("Verzeichnis");
    let program = dir.join("clock.takt");
    let src = "system:\n    language = 1\n    tick     = 10 ms\n\n\
               input  wall  : Duration @ hw(\"sys/clock\")\n\
               output known : bool @ hw(\"o/known\") with safe = false\n\
               output late  : bool @ hw(\"o/late\")  with safe = false\n\n\
               machine m:\n    initial RUN\n    state RUN:\n        loop:\n\
               \x20           known = wall.valid\n            late = wall.or(0 s) > 18_500 d\n";
    std::fs::write(&program, src).expect("schreibbar");
    let mut host = Host::new();
    let options = board::Options::fresh(4);
    let text =
        host.build(&program, &options).and_then(|exe| host.run(&exe, &options)).unwrap_or_else(|e| panic!("{e}"));
    // Der MCU-Rahmen schreibt `bool` als 0 und 1.
    assert!(text.contains("t=0 out known 1") && text.contains("t=0 out late 1"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}
