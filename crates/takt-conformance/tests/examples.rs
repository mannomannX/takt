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
use takt_mir::Program;
use takt_sema::{Build, Options};

mod common;

/// Die Beispiele der Referenz: die sechs des M4-Exits, 14.7 und 14.8. Die
/// Signaturpruefung von 14.8 ruft der Rahmen ueber `takt-native-abi`
/// (FB-293).
const EXAMPLES: [&str; 8] = ["14_1", "14_2", "14_3", "14_4", "14_5", "14_6", "14_7", "14_8"];

/// Bekannte Luecken des Wirtsrahmens: Beispiel, Ausgang, Grund. Jede muss
/// noch auftreten — schliesst sich eine, scheitert der Test, bis sie hier
/// verschwindet; eine neue Abweichung faellt nie hinein.
const GAPS: &[(&str, &str, &str)] = &[(
    "14_8",
    "keys_sim",
    "FB-402: Der Wirtsrahmen schreibt keinen Record-Ausgang mit `bytes`-Feld. \
     TODO(M11 Schritt 9): mit dem Produktrahmen",
)];

/// Wie viele Ticks verglichen werden.
///
/// Lang genug, dass jedes Beispiel seinen Ablauf beginnt, kurz genug,
/// dass der Vergleich im Testlauf bleibt.
const TICKS: u64 = 40;

fn example(name: &str) -> Program {
    let path = format!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try/sim/{}/program.takt"), name);
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
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
    let Some(clang) = common::clang() else { return };
    let mut failed = Vec::new();
    let mut checked = 0;
    let mut gaps_seen = Vec::new();
    for name in EXAMPLES {
        let p = example(name);
        let native = match common::run_native_all(&clang, &p, name, TICKS) {
            Ok(t) => t,
            Err(e) => {
                failed.push(format!("{name}: kein nativer Lauf:\n{e}"));
                continue;
            }
        };
        let interpreted = interpreted(&p);
        let (gaps, diffs): (Vec<_>, Vec<_>) = compare(&interpreted, &native).into_iter().partition(|d| {
            GAPS.iter().any(|(example, output, _)| *example == name && d.output == *output && d.native == "fehlt")
        });
        gaps_seen.extend(gaps.iter().map(|d| (name, d.output.clone())));
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
    for (example, output, why) in GAPS {
        assert!(
            gaps_seen.iter().any(|(n, o)| n == example && o == output),
            "die Luecke {example} `{output}` ist geschlossen; den Eintrag in `GAPS` entfernen ({why})"
        );
    }
}
