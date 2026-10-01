//! Der Korpus auf einem Board gegen den Interpreter (13.8, Satz 9.4.4).

use std::collections::BTreeSet;

use takt_conformance::board::{self, Board, Options};
use takt_conformance::compare;
use takt_mir::program::Program;

/// Ticks eines Konformitaetslaufs auf dem Board.
pub const TICKS: u64 = 60;

/// Ein Programm des Korpus, fuer die Simulation uebersetzt.
pub fn corpus(name: &str) -> Program {
    program(&board::corpus_path(name))
}

/// Ein Programm, fuer die Simulation uebersetzt.
pub fn program(path: &std::path::Path) -> Program {
    let src = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let options = takt_sema::Options {
        policy: takt_diag::Policy::default(),
        build: takt_sema::Build::Sim,
        profile: None,
        ..Default::default()
    };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{}:\n{}", path.display(), errors.join("\n"));
    out.program.unwrap_or_else(|| panic!("{}: kein Programm", path.display()))
}

/// **Ein Job, der laenger rechnet als ein Tick, verspaetet keinen** (4.5,
/// 12.3). `long_job.takt` rechnet SHA-256 ueber 4096 Byte bei 1 ms Tick in
/// Echtzeit: Das Ergebnis stimmt mit dem Interpreter ueberein und erscheint
/// nach seiner Dauer, und kein Tick beginnt um mehr als eine Zehntelperiode
/// spaeter als im Mittel. Liefe der Job im Schritt oder ohne Unterbrechung,
/// begaenne der Tick danach um die Dauer seiner Rechnung zu spaet.
///
/// Gemessen wird die Verspaetung gegen den Median, nicht der Sprung zum
/// vorigen Tick; die ersten beiden Ticks laufen noch an. Ein Tick, der eine
/// Periode zu spaet kommt, faellt hier auf (FB-352); dass er nach dem Job
/// frueher begann als nach `wfi` (FB-317), war dieselbe Ursache.
pub fn long_job_keeps_the_tick(board: &mut dyn Board) -> Vec<String> {
    let path = board::root().join("crates/takt-conformance/tests/programs/long_job.takt");
    let options = Options::timed(TICKS);
    let text = match board.build(&path, &options).and_then(|elf| board.run(&elf, &options)) {
        Ok(t) => t,
        Err(e) => return vec![format!("kein Lauf: {e}")],
    };
    let mut failed = Vec::new();
    let diffs = compare(&run_interpreted(&program(&path)), &text);
    if !diffs.is_empty() {
        failed.push(format!("{} Abweichungen: {diffs:?}\n{text}", diffs.len()));
    }
    let drift = Drift::of(&text);
    if drift.ticks < 50 || drift.late > 100_000 {
        failed.push(format!("{} Zeitzeilen, ein Tick {} ns spaeter als im Mittel:\n{text}", drift.ticks, drift.late));
    }
    failed
}

/// Wie spaet die Ticks eines Laufs nach ihrer Grenze begannen (`drift`
/// der Zeitzeilen, 7.3); die ersten beiden laufen noch an.
#[derive(Clone, Copy, Debug)]
pub struct Drift {
    /// Zeitzeilen.
    pub ticks: usize,
    /// Median in ns.
    pub median: i64,
    /// Der spaeteste Tick, in ns ueber dem Median.
    pub late: i64,
}

impl Drift {
    /// Aus dem Trace eines Laufs in Echtzeit.
    pub fn of(text: &str) -> Drift {
        let mut drifts: Vec<i64> = text
            .lines()
            .filter_map(|l| l.split_whitespace().find_map(|w| w.strip_prefix("drift="))?.parse().ok())
            .skip(2)
            .collect();
        drifts.sort_unstable();
        let median = drifts.get(drifts.len() / 2).copied().unwrap_or(0);
        Drift { ticks: drifts.len(), median, late: drifts.last().map_or(0, |d| d - median) }
    }
}

/// **Ein Tick ueber seiner Periode faultet im naechsten jede Maschine**
/// (7.3, 5.4, FB-332). `overrun.takt` laesst die Last wachsen, bis ein Tick
/// laenger rechnet als seine Periode, aber weit unter der Frist des
/// Watchdogs. Die Schleife meldet den Ueberlauf, der Rahmen schreibt
/// `runtime Overrun` und stellt `Runtime(Overrun)` im naechsten Tick zu:
/// `ramp` nimmt seinen Fault-Pfad und `bystander` auch, obwohl sie nichts
/// rechnet. Danach laeuft der Lauf ohne Reset zu Ende, und der Interpreter
/// spielt ihn mit der Zeile `runtime` als Stimulus nach (12.5).
pub fn overrun_reaches_every_machine(board: &mut dyn Board) -> Vec<String> {
    const RUN: u64 = 300;
    let path = board::root().join("crates/takt-conformance/tests/programs/overrun.takt");
    let options = Options::timed(RUN);
    let text = match board.build(&path, &options).and_then(|elf| board.run(&elf, &options)) {
        Ok(t) => t,
        Err(e) => return vec![format!("kein Lauf: {e}")],
    };
    let tick_of = |needle: &str| {
        text.lines()
            .find(|l| l.contains(needle))
            .and_then(|l| l.strip_prefix("t=")?.split_whitespace().next()?.parse::<u64>().ok())
    };
    let raised = tick_of(" runtime Overrun");
    let (ramp, bystander) = (tick_of(" fault ramp Runtime(Overrun)"), tick_of(" fault bystander Runtime(Overrun)"));
    let Some(at) = raised.filter(|_| raised == ramp && raised == bystander) else {
        return vec![format!(
            "nicht im selben Tick: runtime {raised:?}, ramp {ramp:?}, bystander {bystander:?}\n{text}"
        )];
    };
    let mut failed = Vec::new();
    if text.matches(" runtime Overrun").count() != 1 {
        failed.push(format!("mehr als ein Ueberlauf; `SAFE` rechnet nicht mehr:\n{text}"));
    }
    let stimulus = takt_interp::Trace::parse(&format!("t={at} runtime Overrun\n")).expect("Stimulus");
    let options = takt_interp::RunOptions { ticks: RUN, ..Default::default() };
    let replayed = match takt_interp::run(&program(&path), &stimulus, &options) {
        Ok(r) => r.trace.render(),
        Err(e) => return vec![format!("Interpreter: {e:?}")],
    };
    let diffs = compare(&replayed, &text);
    if !diffs.is_empty() {
        failed.push(format!("{} Abweichungen vom nachgespielten Lauf: {diffs:?}\n{text}", diffs.len()));
    }
    failed
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
    agreement_with(board, names, only, &Options::fresh(TICKS))
}

/// Wie [`agreement`], mit anderen Optionen des Baus — etwa im Profil
/// `rtos` (12.8).
pub fn agreement_with(board: &mut dyn Board, names: &[&str], only: Option<&str>, options: &Options) -> Vec<String> {
    let mut failed = Vec::new();
    for name in names.iter().filter(|n| only.is_none_or(|o| o == **n)) {
        let p = corpus(name);
        let text = match board.build(&board::corpus_path(name), options).and_then(|elf| board.run(&elf, options)) {
            Ok(t) => t,
            Err(e) => {
                failed.push(format!("{name}: kein Lauf auf dem Board:\n{e}"));
                continue;
            }
        };
        let interpreted = run_interpreted(&p);
        let missing: Vec<String> = output_names(&interpreted).difference(&output_names(&text)).cloned().collect();
        // Ein `f32` schreibt das Board als seinen Wert in `f64` (4.2, FB-356).
        let widened = takt_conformance::run::widen_f32(&interpreted, &takt_conformance::run::f32_outputs(&p));
        let diffs = compare(&widened, &text);
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
    let name = board.name().to_string();
    match takt_conformance::bench::natives_on(board, &program) {
        Ok((natives, math)) => {
            let natives = natives
                .iter()
                .inspect(|r| eprintln!("{name} {}: Stack {} von {} Byte", r.native.name(), r.stack, r.contract))
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
                .collect::<Vec<_>>();
            let contract = takt_mir::analysis::stack::MATH_STACK;
            let math = math
                .iter()
                .inspect(|r| {
                    eprintln!("{name} {}: Stack {} von {contract} Byte, {} Zyklen", r.name(), r.stack, r.cycles)
                })
                .filter(|r| !r.same_result() || !r.within_contract())
                .map(|r| {
                    format!(
                        "{}: abweichende Zeilen {:?}, Stack {} von {contract} Byte",
                        r.name(),
                        r.deviations,
                        r.stack
                    )
                })
                .collect::<Vec<_>>();
            natives.into_iter().chain(math).collect()
        }
        Err(e) => vec![format!("kein Lauf der Natives: {e}")],
    }
}
