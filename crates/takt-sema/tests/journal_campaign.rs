//! Die Stromausfall-Kampagne gegen das Journal (5.9, 13.7, 8.11): jeder
//! Lauf endet mit dem alten oder dem neuen Stand, nie dazwischen — die
//! Aussage von `takt-rt-core/tests/journal.rs`, hier ueber das Takt-Modell.
//! `45_journal_cut` schreibt je Slot einen Eintrag, `110_journal_log` das
//! Log der Version 2 ueber drei Generationen.
//!
//! TODO: Ein Schnitt waehrend ERASE braucht ein Flash-Modell, das halb
//! loescht; `flash_model` im Prelude loescht atomar.

use std::collections::BTreeMap;

use takt_diag::Policy;
use takt_interp::trace::LineKind;
use takt_interp::{RunOptions, Trace, Verdict, run};
use takt_sema::{Build, Options};

const PROGRAM: &str = include_str!("../../../corpus-try/45_journal_cut.takt");
const LOG: &str = include_str!("../../../corpus-try/110_journal_log.takt");

fn compile(src: &str) -> Result<takt_mir::Program, Vec<String>> {
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
    let out = takt_sema::compile(src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    if errors.is_empty() { Ok(out.program.expect("Programm")) } else { Err(errors) }
}

#[test]
fn every_cut_leaves_the_old_or_the_new_entry() {
    let p = compile(PROGRAM).expect("uebersetzt");
    let runs = takt_interp::campaign::runs(&p, &p.campaigns[0]).expect("Laufraum");
    assert_eq!(runs.len(), 91);
    for run_ in &runs {
        let cut: u32 = run_.params[0].1.parse().expect("CUT");
        let options = RunOptions { ticks: 200, overrides: run_.params.clone(), ..Default::default() };
        let r = run(&p, &Trace::default(), &options).expect("Lauf");
        let text = r.trace.render();
        assert_eq!(r.verdict, Verdict::Pass, "CUT={cut}:\n{text}");
        assert!(!text.contains("fault"), "CUT={cut}:\n{text}");
        let stand = r
            .trace
            .lines
            .iter()
            .find_map(|l| match &l.kind {
                LineKind::Measure { name, value, .. } if name == "stand" => Some(value.clone()),
                _ => None,
            })
            .expect("Messwert");
        // 43 Byte je Schreibvorgang: bis dahin ist der erste Eintrag halb
        // (leer), danach steht der alte, ab dem letzten Byte der neue.
        let want = match cut {
            1..=42 => "0",
            43..=85 => "1",
            _ => "2",
        };
        assert_eq!(stand, want, "CUT={cut}");
    }
}

#[test]
fn an_instance_argument_may_be_a_param_but_not_a_tunable() {
    let body = "system:\n    language = 1\n
param A : int in 0..10 = 3
tunable param T : int in 0..10 = 3
output o : int in 0..10 @ hw(\"o/o\") with safe = 0
machine tmpl(gain: int in 0..10):
    initial RUN
    state RUN:
        loop:
            o = gain
";
    let p = compile(&format!("{body}instance a = tmpl(gain = A)\n")).expect("param als Argument");
    let r = run(&p, &Trace::default(), &RunOptions { ticks: 1, ..Default::default() }).expect("Lauf");
    assert!(r.trace.render().contains("out o 3"), "{}", r.trace.render());
    let e = compile(&format!("{body}instance a = tmpl(gain = T)\n")).expect_err("tunable");
    assert!(e.join("\n").contains("tunable"), "{e:?}");
}

/// Die Messwerte eines Laufs nach Namen.
fn measures(trace: &Trace) -> BTreeMap<String, String> {
    trace
        .lines
        .iter()
        .filter_map(|l| match &l.kind {
            LineKind::Measure { name, value, .. } => Some((name.clone(), value.clone())),
            _ => None,
        })
        .collect()
}

/// Parameter eines Laufs und seine Messwerte.
type Measured = (Vec<(String, String)>, BTreeMap<String, String>);

/// Die Laeufe einer Kampagne von `110_journal_log`: jeder mit Verdikt
/// `pass` und ohne Fault.
fn log_runs(campaign: &str) -> Vec<Measured> {
    let p = compile(LOG).expect("uebersetzt");
    let c = p.campaigns.iter().find(|c| c.name == campaign).expect("Kampagne");
    let runs = takt_interp::campaign::runs(&p, c).expect("Laufraum");
    runs.iter()
        .map(|r| {
            let options = RunOptions { ticks: 320, overrides: r.params.clone(), ..Default::default() };
            let out = run(&p, &Trace::default(), &options).expect("Lauf");
            let text = out.trace.render();
            assert_eq!(out.verdict, Verdict::Pass, "{:?}:\n{text}", r.params);
            assert!(!text.contains(" fault "), "{:?}:\n{text}", r.params);
            (r.params.clone(), measures(&out.trace))
        })
        .collect()
}

/// **Das Log ueber drei Generationen, mit Schnitt an jedem Byte**
/// (SEM2-066). Sieben Vorgaenge zu 37 Byte (5 Nutzlast, dann 32 Kopf)
/// fuellen Slot 0, Slot 1 und nach dem Loeschen wieder Slot 0. Bis zum
/// letzten Byte eines Kopfes findet der Neustart den vorigen Stand, mit ihm
/// den neuen; ein halber Kopf faellt an Magic, Logik-Hash oder CRC durch.
/// Danach schreibt das Programm weiter, und der Start am Ende findet den
/// siebten Stand mit Sequenznummer 7.
///
/// Wo der letzte Eintrag liegt, zeigt die Regel fuer den Rest: Ohne Schnitt
/// steht er in Slot 0 vorn (dritte Generation). Bricht das Anhaengen des
/// zweiten Eintrags in seiner Nutzlast ab, ist der Rest von Slot 0 nicht
/// mehr geloescht; der zweite geht nach Slot 1, und der siebte steht in
/// Slot 0 als dritter. Bricht er erst nach dem letzten Kopfbyte ab, gilt
/// der Eintrag, und alles liegt wie ohne Schnitt.
#[test]
fn every_cut_in_the_log_leaves_a_whole_entry_and_writing_goes_on() {
    let runs = log_runs("power_cut");
    assert_eq!(runs.len(), 261);
    let layout = BTreeMap::from([(0, ("0", "40")), (40, ("0", "120")), (74, ("0", "40"))]);
    for (params, m) in &runs {
        let cut: u32 = params[0].1.parse().expect("CUT");
        let after_cut = (1..=259).contains(&cut).then(|| (cut / 37).to_string());
        assert_eq!(m.get("after_cut"), after_cut.as_ref(), "CUT={cut}");
        assert_eq!(m["stand"], "7", "CUT={cut}");
        if let Some((slot, end)) = layout.get(&cut) {
            assert_eq!((m["last_slot"].as_str(), m["last_end"].as_str()), (*slot, *end), "CUT={cut}");
        }
    }
}

/// **Ein Bitfehler im juengsten Eintrag** (SEM2-066): Der CRC verwirft
/// ihn, der Rest von Slot 0 ist nicht mehr geloescht, und es gilt der
/// sechste Stand am Ende von Slot 1 — der Gewinner ist der gueltige
/// Eintrag mit der hoechsten Sequenznummer, nicht der zuletzt geschriebene.
#[test]
fn a_bit_error_in_the_newest_entry_falls_back_to_the_one_before() {
    let runs = log_runs("bit_error");
    assert_eq!(runs.len(), 1);
    let m = &runs[0].1;
    assert_eq!((m["stand"].as_str(), m["last_slot"].as_str(), m["last_end"].as_str()), ("6", "1", "120"));
}
