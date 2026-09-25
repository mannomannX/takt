//! Der Korpus auf einem Board gegen den Interpreter (13.8, Satz 9.4.4).

use std::collections::BTreeSet;

use takt_conformance::board::{self, Board, Options};
use takt_conformance::compare;
use takt_mir::program::Program;

/// Ticks eines Konformitaetslaufs auf dem Board.
pub const TICKS: u64 = 60;

/// Ein Programm des Korpus, fuer die Simulation uebersetzt.
pub fn corpus(name: &str) -> Program {
    let path = board::corpus_path(name);
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let options =
        takt_sema::Options { policy: takt_diag::Policy::default(), build: takt_sema::Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{name}:\n{}", errors.join("\n"));
    out.program.unwrap_or_else(|| panic!("{name}: kein Programm"))
}

/// Der Soll-Trace ueber [`TICKS`] Ticks.
pub fn run_interpreted(p: &Program) -> String {
    let options = takt_interp::RunOptions { ticks: TICKS, ..Default::default() };
    match takt_interp::run(p, &takt_interp::Trace::default(), &options) {
        Ok(r) => r.trace.render(),
        Err(e) => panic!("Interpreter: {e:?}"),
    }
}

/// Die Namen der Ausgaenge, die ein Trace nennt.
pub fn output_names(text: &str) -> BTreeSet<String> {
    text.lines()
        .filter_map(|l| {
            let mut w = l.split_whitespace();
            w.next()?.strip_prefix("t=")?;
            (w.next()? == "out").then(|| w.next().map(String::from))?
        })
        .collect()
}

/// Der letzte `out`-Wert eines Ausgangs im Trace.
pub fn last_output(text: &str, name: &str) -> Option<String> {
    text.lines()
        .filter_map(|l| {
            let mut w = l.split_whitespace();
            w.next()?.strip_prefix("t=")?;
            (w.next()? == "out" && w.next()? == name).then(|| w.collect::<Vec<_>>().join(" "))
        })
        .next_back()
}

/// Laesst `names` auf dem Board laufen und haelt jeden Trace gegen den
/// Interpreter; liefert je abweichendem Programm eine Meldung.
///
/// `only` beschraenkt den Lauf auf ein Programm (`TAKT_…_ONLY`).
pub fn agreement(board: &mut dyn Board, names: &[&str], only: Option<&str>) -> Vec<String> {
    let mut failed = Vec::new();
    for name in names.iter().filter(|n| only.is_none_or(|o| o == **n)) {
        let p = corpus(name);
        let options = Options::fresh(TICKS);
        let text = match board.build(&board::corpus_path(name), &options).and_then(|elf| board.run(&elf, &options)) {
            Ok(t) => t,
            Err(e) => {
                failed.push(format!("{name}: kein Lauf auf dem Board:\n{e}"));
                continue;
            }
        };
        let interpreted = run_interpreted(&p);
        let missing: Vec<String> = output_names(&interpreted).difference(&output_names(&text)).cloned().collect();
        let diffs = compare(&interpreted, &text);
        if !missing.is_empty() || !diffs.is_empty() {
            let list: Vec<String> = diffs.iter().take(8).map(|d| format!("  {d}")).collect();
            failed.push(format!(
                "{name}: {} Abweichungen, fehlende Ausgaenge {missing:?}\n{}\n--- Interpreter ---\n{}\n--- Board ---\n{}",
                diffs.len(),
                list.join("\n"),
                interpreted.lines().take(12).collect::<Vec<_>>().join("\n"),
                text.lines().filter(|l| l.starts_with("t=")).take(12).collect::<Vec<_>>().join("\n")
            ));
        }
        eprintln!("{} {name}: {} Abweichungen", board.name(), diffs.len());
    }
    failed
}

/// **Die kuratierten Natives rechnen auf dem Board wie auf dem Wirt, und
/// ihr Stack bleibt in der Zusage** (13.8, 4.5): je Funktion eine Meldung,
/// wenn nicht.
pub fn natives_agree(board: &mut dyn Board) -> Vec<String> {
    let program = board::corpus_path("01_minimal.takt");
    match takt_conformance::bench::natives_on(board, &program) {
        Ok(rows) => rows
            .iter()
            .filter(|r| !r.same_result() || !r.within_contract())
            .map(|r| {
                format!(
                    "{}: abweichende Zeilen {:?}, Stack {} von {} Byte",
                    r.native.name(),
                    r.deviations,
                    r.stack,
                    r.contract
                )
            })
            .collect(),
        Err(e) => vec![format!("kein Lauf der Natives: {e}")],
    }
}
