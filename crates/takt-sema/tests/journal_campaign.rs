//! Die Stromausfall-Kampagne gegen das Journal (5.9, 13.7, 8.11): jeder
//! Lauf endet mit dem alten oder dem neuen Stand, nie dazwischen — die
//! Aussage von `takt-rt-core/tests/journal.rs`, hier ueber das Takt-Modell.

use takt_diag::Policy;
use takt_interp::trace::LineKind;
use takt_interp::{RunOptions, Trace, Verdict, run};
use takt_sema::{Build, Options};

const PROGRAM: &str = include_str!("../../../corpus-try/45_journal_cut.takt");

fn compile(src: &str) -> Result<takt_mir::Program, Vec<String>> {
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
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
