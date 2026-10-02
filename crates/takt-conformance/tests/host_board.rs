//! Der MCU-Rahmen auf dem Wirt (13.8, M10 Schritt 11): derselbe Rahmen,
//! dieselbe Schleife und derselbe Treiberrand wie auf den Boards, mit dem
//! Pruefgeraet `takt-driver-probe` als Treiber-Crate. Es sind dieselben
//! Pruefungen wie in `board_esp32c6.rs` und `board_stm32f401.rs`, ohne Board.
//!
//! Gebaut wird mit clang und dem Werkzeug `takt` des eigenen Profils, wie
//! `cargo test --workspace` es baut; ohne clang uebersprungen.

mod common;

use common::board::{agreement, driver_edge_agrees, unread_channels_are_recorded};
use takt_conformance::board::{self, CORPUS, host::Host};

/// Der Wirt mit dem Pruefgeraet; ohne clang keiner.
fn probe() -> Option<Host> {
    if matches!(takt_llvm::toolchain::find(), takt_llvm::toolchain::Clang::Missing) {
        eprintln!("clang fehlt; uebersprungen");
        return None;
    }
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
/// `TAKT_HOST_CORPUS`; `TAKT_HOST_ONLY=42_map.takt` fuer einen einzelnen Fall.
#[test]
fn the_host_board_agrees_with_the_interpreter() {
    if std::env::var_os("TAKT_HOST_CORPUS").is_none() || probe().is_none() {
        return;
    }
    let only = std::env::var("TAKT_HOST_ONLY").ok();
    let failed = agreement(&mut Host::new(), CORPUS, only.as_deref());
    assert!(failed.is_empty(), "{}", failed.join("\n\n"));
}
