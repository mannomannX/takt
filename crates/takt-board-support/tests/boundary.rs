//! Die Grenze zwischen rechnender und registerschreibender Haelfte.
//!
//! Das Board-Crate liegt ausserhalb des Workspace, weil Registerzugriff
//! `unsafe` braucht (9.5: Treiber sind TCB). Diese Ausnahme ist nur so
//! lange vertretbar, wie sie **klein** bleibt — und das ist keine
//! Eigenschaft, die sich von selbst haelt: Eine Rechnung, die jemand aus
//! Bequemlichkeit drueben hinschreibt, ist dort ungetestet und faellt
//! niemandem auf.
//!
//! Die Tests hier bewachen die Grenze von beiden Seiten: Dieses Crate
//! darf nichts Hardwarenahes kennen, und das Board-Crate soll die
//! Rechnungen von hier benutzen statt eigene zu haben.

use std::fs;
use std::path::PathBuf;

/// Die Quellverzeichnisse aller Board-Crates (`crates/takt-board-*` ausser
/// diesem), vom Manifest dieses Crates aus.
fn board_sources() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = fs::read_dir("..")
        .expect("crates/")
        .flatten()
        .filter(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.starts_with("takt-board-") && name != "takt-board-support"
        })
        .map(|e| e.path().join("src"))
        .collect();
    out.sort();
    out
}

/// **Dieses Crate bleibt frei von Hardware-Abhaengigkeiten.**
///
/// Sein Wert liegt darin, dass es auf dem Wirt laeuft. Eine Abhaengigkeit
/// auf `cortex-m` oder die PAC naehme ihm genau das — und die Rechnungen
/// waeren wieder ungetestet.
#[test]
fn the_computing_half_has_no_hardware_dependencies() {
    let manifest = fs::read_to_string("Cargo.toml").expect("Cargo.toml");
    // Nur die Abhaengigkeiten, nicht die Kommentare: Der Kommentar, der
    // erklaert, warum `cortex-m` hier *nicht* steht, ist kein Verstoss.
    let deps: String = manifest
        .lines()
        .skip_while(|l| !l.starts_with("[dependencies]"))
        .take_while(|l| l.starts_with("[dependencies]") || !l.starts_with('['))
        .filter(|l| !l.trim_start().starts_with('#'))
        .collect();
    for forbidden in ["cortex-m", "stm32f4", "embedded-hal", "riscv"] {
        assert!(
            !deps.contains(forbidden),
            "`{forbidden}` gehoert nicht hierher: Dieses Crate muss auf dem Wirt laufen, sonst sind seine \
             Rechnungen ungetestet"
        );
    }
}

/// **Kein Board-Crate rechnet selbst.**
///
/// Die vier Groessen, an denen ein stiller Zeitfehler entsteht —
/// Nanosekunden je Zaehlschritt, Prescaler, Zyklenumrechnung,
/// Zaehlerueberlauf — duerfen dort nicht noch einmal stehen. Der Test
/// sucht die Zahlen, an denen man sie erkennt, in jedem Board-Crate; die
/// beiden, die es gibt, muessen da sein — ein fehlendes waere ein
/// stilles Bestehen.
#[test]
fn the_register_half_does_not_compute_time_itself() {
    let sources = board_sources();
    for board in ["takt-board-esp32c6", "takt-board-stm32f401"] {
        assert!(sources.iter().any(|s| s.parent().is_some_and(|p| p.ends_with(board))), "{board} fehlt");
    }
    let mut found = Vec::new();
    for dir in &sources {
        for entry in fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())).flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let src = fs::read_to_string(&path).expect("lesbar");
            for (n, line) in src.lines().enumerate().filter(|(_, l)| !l.trim_start().starts_with("//")) {
                for pattern in ["1_000_000_000 /", "/ 1_000", "* 1_000", "div_ceil(1_000"] {
                    if line.contains(pattern) {
                        found.push(format!("{}:{}: `{pattern}` in `{}`", path.display(), n + 1, line.trim()));
                    }
                }
            }
        }
    }
    assert!(
        found.is_empty(),
        "Zeitrechnung im Board-Crate — sie gehoert nach `takt-board-support`, wo sie getestet wird:\n{}",
        found.join("\n")
    );
}

/// Die Rechnungen, die das Board-Crate braucht, sind alle oeffentlich.
///
/// Ein privates Stueck hier zwaenge das Board-Crate dazu, es drueben
/// nachzubauen — und genau das soll nicht passieren.
#[test]
fn everything_the_board_needs_is_exported() {
    // Kompiliert nur, wenn die Namen oeffentlich sind; der Test ist der
    // Aufruf selbst.
    assert_eq!(takt_board_support::counts_for(1_000_000, 1_000_000), Ok(1000));
    assert_eq!(takt_board_support::prescaler_for(84_000_000, 1_000_000), Some(83));
    assert_eq!(takt_board_support::ns_per_count(1_000_000), Some(1_000));
    assert_eq!(takt_board_support::clock::period_ns(1_000_000, 1000), 1_000_000);

    let c = takt_board_support::Counter64::new();
    c.tick();
    assert_eq!(c.get(), 1);

    let m = takt_board_support::Measurement { cycles: 84_000, core_hz: 84_000_000 };
    assert_eq!(m.ns(), 1_000_000);
}
