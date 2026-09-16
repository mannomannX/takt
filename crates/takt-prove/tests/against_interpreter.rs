//! Die Kodierung gegen den Interpreter (plan/m6.md 2.8): Das Modell laeuft
//! konkret mit denselben Eingaben, und Outputs wie Zustaende stimmen in
//! jedem Tick ueberein — ohne Solver.

use std::collections::BTreeMap;

use takt_interp::trace::LineKind;
use takt_interp::{RunOptions, Trace, run};
use takt_mir::Program;
use takt_mir::program::{Binding, Direction};
use takt_prove::{Model, Val, encode, eval};

fn corpus(name: &str) -> Program {
    let path = format!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try/{}"), name);
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let options =
        takt_sema::Options { policy: takt_diag::Policy::default(), build: takt_sema::Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{name}:\n{}", errors.join("\n"));
    out.program.expect("Programm")
}

/// Der Wert einer Trace-Zeile als `Val`.
fn parse_val(text: &str) -> Option<Val> {
    let first = text.split_whitespace().next()?;
    match first {
        "true" => Some(Val::Bool(true)),
        "false" => Some(Val::Bool(false)),
        _ => first.parse::<i64>().map(Val::Int).ok().or_else(|| first.parse::<f64>().map(Val::F64).ok()),
    }
}

fn same(a: Val, b: Val) -> bool {
    match (a, b) {
        (Val::F64(x), Val::F64(y)) => x.to_bits() == y.to_bits(),
        (Val::F32(x), Val::F64(y)) | (Val::F64(y), Val::F32(x)) => f64::from(x).to_bits() == y.to_bits(),
        (Val::Int(x), Val::F64(y)) | (Val::F64(y), Val::Int(x)) => (x as f64).to_bits() == y.to_bits(),
        (x, y) => x == y,
    }
}

/// `sim`-Bindungen (8.3): Input-Name und der Output, der ihn speist.
fn sim_bindings(p: &Program) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for c in p.channels.iter().filter(|c| c.dir == Direction::Input) {
        let Binding::Hw(addr) = &c.binding else { continue };
        for o in p.channels.iter().filter(|o| o.dir == Direction::Output) {
            if matches!(&o.binding, Binding::Sim(a) if a == addr) {
                out.push((c.name.clone(), o.name.clone()));
            }
        }
    }
    out
}

/// Fuehrt Interpreter und Modell mit demselben Stimulus und vergleicht je
/// Tick jeden Output und jedes Blatt. Ein `sim`-gebundener Input liest
/// den committeten Output des vorigen Ticks (Unit-Delay, 8.3).
fn agree(name: &str, stimulus: &str, ticks: u64) {
    let p = corpus(name);
    let model: Model = encode(&p).unwrap_or_else(|e| panic!("{name}: {}", e.what));
    let stim = Trace::parse(stimulus).expect("Stimulus");
    let r = run(&p, &stim, &RunOptions { ticks, ..Default::default() }).expect("Lauf");
    let bound = sim_bindings(&p);
    // Eingaben des Modells: Commands im Tick ihrer Zeile, Inputs halten.
    let mut inputs: BTreeMap<(u64, String), Val> = BTreeMap::new();
    let mut held: BTreeMap<String, Val> = BTreeMap::new();
    for k in 0..=ticks {
        for l in stim.lines.iter().filter(|l| l.tick == k) {
            match &l.kind {
                LineKind::Command { name } => {
                    inputs.insert((k, format!("i.cmd.{name}")), Val::Bool(true));
                }
                LineKind::Input { channel, sample } => {
                    if let Some(v) = sample.value.as_deref().and_then(parse_val) {
                        held.insert(format!("i.{channel}"), v);
                    }
                }
                _ => {}
            }
        }
        for (n, v) in &held {
            inputs.insert((k, n.clone()), *v);
        }
    }
    let mut states: Vec<eval::Env> = Vec::new();
    for k in 0..=ticks {
        let mut env = states.last().cloned().unwrap_or_default();
        for (n, sort) in &model.inputs {
            let from_sim = bound
                .iter()
                .find(|(i, _)| format!("i.{i}") == *n)
                .and_then(|(_, o)| states.last().and_then(|s| s.get(&format!("s.out.{o}")).copied()));
            let v = inputs.get(&(k, n.clone())).copied().or(from_sim).unwrap_or(Val::zero(*sort));
            env.insert(n.clone(), v);
        }
        let step: eval::Env = model
            .state
            .iter()
            .map(|v| (v.name.clone(), eval::eval(if k == 0 { &v.init } else { &v.next }, &env)))
            .collect();
        states.push(step);
    }
    // Der Interpreter schreibt Outputs und Zustaende nur bei Aenderung.
    let mut outputs: BTreeMap<String, Val> = BTreeMap::new();
    let mut leaves: BTreeMap<String, String> = BTreeMap::new();
    for k in 0..=ticks {
        for l in r.trace.lines.iter().filter(|l| l.tick == k) {
            match &l.kind {
                LineKind::Output { channel, value } => {
                    if let Some(v) = parse_val(value) {
                        outputs.insert(channel.clone(), v);
                    }
                }
                LineKind::State { machine, path } => {
                    leaves.insert(machine.clone(), path.rsplit('.').next().unwrap_or(path).to_string());
                }
                _ => {}
            }
        }
        let s = &states[k as usize];
        for (c, want) in &outputs {
            let got = s[&format!("s.out.{c}")];
            assert!(same(got, *want), "{name} t={k}: Output `{c}`: Modell {got:?}, Interpreter {want:?}");
        }
        for (m, want) in &leaves {
            let Val::Int(code) = s[&format!("s.{m}.leaf")] else { panic!("Blattcode") };
            // Ein Sequenzsegment heisst in der MIR `IGNITION.S0`, im Trace steht der Pfad.
            let got = model.leaf_name(m, code).unwrap_or("?");
            assert_eq!(got.rsplit('.').next(), Some(want.as_str()), "{name} t={k}: Zustand von `{m}`");
        }
    }
}

#[test]
fn a_blinker_with_after_and_a_command_agrees() {
    agree("16_timing.takt", "t=3 cmd go\nt=40 cmd go\n", 60);
}

#[test]
fn a_range_fault_takes_the_fault_path_in_both() {
    agree("19_faults.takt", "", 10);
}

#[test]
fn a_guarded_check_with_inputs_agrees() {
    let mut stim = String::new();
    for k in 0..=30 {
        let p = if (10..20).contains(&k) { 70 } else { 40 };
        stim.push_str(&format!("t={k} in tank_p {p} bar\n"));
    }
    stim.push_str("t=2 cmd start\nt=25 cmd reset\n");
    agree("01_minimal.takt", &stim, 30);
}

/// 14.1 (der Hotfire-Test der Referenz) mit den Szenarien des Korpus:
/// Sequenzen als Zustaende, `after`, verschachtelte Zustaende, `abort`,
/// ein Modell mit Periode 50 und `sim`-Bindungen.
#[test]
fn the_hotfire_example_agrees_over_its_scenarios() {
    for scenario in ["nominal", "abort", "overpressure", "reset", "timeout"] {
        let path = format!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try/sim/14_1/{}.stim.trace"), scenario);
        let stim = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        agree("sim/14_1/program.takt", &stim, 4200);
    }
}

/// Ein Block mit Zustand (5.7): `step` eingebettet, der Zustand in Feldern.
#[test]
fn a_block_step_is_inlined_faithfully() {
    let mut stim = String::new();
    for k in 0..=12 {
        stim.push_str(&format!("t={k} in level {}\n", (k as f64) - 4.0));
    }
    agree("48_contracts.takt", &stim, 12);
}

#[test]
fn the_export_is_smtlib_with_both_queries() {
    let p = corpus("16_timing.takt");
    let model = encode(&p).expect("kodierbar");
    let text = takt_prove::export(&model, 3);
    assert!(text.contains("(set-logic ALL)"));
    assert!(text.contains("(declare-const |s.blink.leaf@0| (_ BitVec 64))"), "{text}");
    assert!(text.contains("(declare-const |i.cmd.go#3| Bool)"), "{text}");
    assert_eq!(text.matches("(check-sat)").count(), 0, "ohne Eigenschaft keine Anfrage");
}
