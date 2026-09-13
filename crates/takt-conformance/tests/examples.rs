//! Die Beispiele der Referenz auf beiden Wegen (M4-Exit, 13.8).
//!
//! `plan.md` verlangt fuer M4 „14.1–14.6 in Echtzeit auf der Box". Die
//! sechs Programme liegen in `corpus-try/sim/14_x/`, laufen im
//! Interpreter gegen ihre Golden-Traces — und liefen bis hierher *nur*
//! dort: Der differentielle Test kannte sie nicht, und der Exit war an
//! dieser Stelle behauptet statt belegt (FB-102).
//!
//! **Warum sie einen eigenen Test brauchen.** Fuenf der sechs haben ein
//! Plant-Modell (8.3) — eine zweite Maschine, die `sim`-Outputs auf
//! dieselben Adressen schreibt, von denen das Programm liest. Der
//! Testrahmen tickte eine Maschine; ohne das Modell bleiben die
//! Eingaenge `Bad`, und verglichen wuerde ein Lauf, den es nicht gibt.
//! `harness::build_all` fuehrt darum alle Maschinen, jede nach ihrer
//! Periode, mit der Bindung `sim` -> `hw` am Ende des Ticks.

use takt_conformance::run::compare;
use takt_diag::Policy;
use takt_interp::{RunOptions, Trace, run};
use takt_llvm::toolchain::{Clang, find};
use takt_mir::Program;
use takt_sema::{Build, Options};

mod common;

/// Die Beispiele, die der M4-Exit nennt.
///
/// 14.7 fehlt: Es laeuft auf Hardware (M5-Exit), und es gibt kein
/// Programm dafuer im Korpus. 14.8 gehoert zu M6 (Flash-Modell-
/// Kampagne) und hat seine Golden-Traces bereits.
const EXAMPLES: [&str; 5] = ["14_1", "14_2", "14_3", "14_4", "14_5"];

/// Die uebrigen, mit dem Konstrukt, an dem der Codegen abbricht.
///
/// Sie stehen hier und nicht im Kommentar, weil ein Test sie mitzaehlen
/// soll: Was fehlt, ist Teil des Ergebnisses. Der Eintrag verschwindet,
/// sobald das Konstrukt gesenkt wird — und dann faellt der Test auf, der
/// ihn noch fuehrt.
const OPEN: [(&str, &str); 1] = [("14_6", "Mustervergleich und `send` im Codegen (8.7, 8.8; FB-112)")];

/// Wie viele Ticks verglichen werden.
///
/// Lang genug, dass jedes Beispiel seinen Ablauf beginnt, kurz genug,
/// dass der Vergleich im Testlauf bleibt.
const TICKS: u64 = 40;

fn example(name: &str) -> Program {
    let path = format!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try/sim/{}/program.takt"), name);
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{name}:\n{}", errors.join("\n"));
    out.program.expect("Programm")
}

fn interpreted(p: &Program) -> String {
    let options = RunOptions { ticks: TICKS, ..Default::default() };
    match run(p, &Trace::default(), &options) {
        Ok(out) => out.trace.render(),
        Err(e) => format!("Trap: {e:?}"),
    }
}

/// **Die Abnahme.** Jedes Beispiel liefert im Interpreter und im
/// erzeugten Code denselben Trace (Satz 9.4.4).
#[test]
fn the_reference_examples_agree_on_both_paths() {
    let Clang::At(path) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return;
    };
    let clang = Clang::At(path);
    let mut failed = Vec::new();
    let mut checked = 0;
    for name in EXAMPLES {
        let p = example(name);
        let native = match common::run_native_all(&clang, &p, name, TICKS) {
            Ok(t) => t,
            Err(e) => {
                failed.push(format!("{name}: laesst sich nicht bauen:\n{e}"));
                continue;
            }
        };
        let interpreted = interpreted(&p);
        let diffs = compare(&interpreted, &native);
        checked += 1;
        if !diffs.is_empty() {
            let list: Vec<String> = diffs.iter().take(8).map(|d| format!("  {d}")).collect();
            failed.push(format!(
                "{name}: {} Abweichungen\n{}\n--- Interpreter ---\n{}\n--- nativ ---\n{}",
                diffs.len(),
                list.join("\n"),
                interpreted.lines().take(12).collect::<Vec<_>>().join("\n"),
                native.lines().take(12).collect::<Vec<_>>().join("\n")
            ));
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n\n"));
    assert_eq!(checked, EXAMPLES.len(), "es wurden nicht alle Beispiele geprueft");
}

/// Was der M4-Exit noch schuldet.
///
/// `plan.md` verlangt „14.1–14.6 in Echtzeit auf der Box"; zwei davon
/// laufen. Der Test haelt die Luecke fest, statt sie in einem Kommentar
/// verschwinden zu lassen — und er schlaegt an, sobald ein Beispiel
/// laeuft, das hier noch als offen steht.
#[test]
fn the_remaining_examples_name_what_is_missing() {
    let Clang::At(path) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return;
    };
    let clang = Clang::At(path);
    let mut unerwartet = Vec::new();
    for (name, reason) in OPEN {
        let p = example(name);
        if let Ok(native) = common::run_native_all(&clang, &p, name, TICKS) {
            if compare(&interpreted(&p), &native).is_empty() {
                unerwartet.push(format!("{name} laeuft jetzt ({reason} ist gesenkt) — in BEISPIELE aufnehmen"));
            }
        }
    }
    assert!(unerwartet.is_empty(), "{}", unerwartet.join("\n"));
    assert_eq!(EXAMPLES.len() + OPEN.len(), 6, "die sechs Beispiele des M4-Exits");
}
