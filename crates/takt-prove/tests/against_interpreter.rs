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
    let options = takt_sema::Options {
        policy: takt_diag::Policy::default(),
        build: takt_sema::Build::Sim,
        profile: None,
        ..Default::default()
    };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{name}:\n{}", errors.join("\n"));
    out.program.expect("Programm")
}

/// Der Wert eines Outputs als `Val`: Zahlen und Wahrheitswerte wie im
/// Trace, ein Enum als seine Diskriminante, wie das Modell es haelt.
fn output_val(p: &Program, channel: &str, text: &str) -> Option<Val> {
    if let Some(v) = parse_val(text) {
        return Some(v);
    }
    let c = p.channels.iter().find(|c| c.name == channel)?;
    let takt_mir::types::Type::Enum(e) = p.types.get(c.ty) else { return None };
    let v = p.enums[e.index()].variants.iter().find(|v| v.name == text.trim())?;
    Some(Val::Int(v.discriminant))
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
    let mut compared = 0usize;
    for k in 0..=ticks {
        for l in r.trace.lines.iter().filter(|l| l.tick == k) {
            match &l.kind {
                LineKind::Output { channel, value } => {
                    let v = output_val(&p, channel, value);
                    let v = v.unwrap_or_else(|| panic!("{name} t={k}: `{value}` von `{channel}` hat keinen Wert"));
                    outputs.insert(channel.clone(), v);
                }
                LineKind::State { machine, path } => {
                    leaves.insert(machine.clone(), path.rsplit('.').next().unwrap_or(path).to_string());
                }
                // Ein Fault fuehrt im selben Tick in sein Ziel (5.3), auch
                // wenn das Ziel der Zustand ist, in dem er auftrat.
                LineKind::Fault { machine, target, .. } => {
                    let leaf = target.rsplit('.').next().unwrap_or(target).to_string();
                    leaves.insert(machine.clone(), leaf);
                }
                _ => {}
            }
        }
        let s = &states[k as usize];
        for (c, want) in &outputs {
            let got = s[&format!("s.out.{c}")];
            assert!(same(got, *want), "{name} t={k}: Output `{c}`: Modell {got:?}, Interpreter {want:?}");
            compared += 1;
        }
        for (m, want) in &leaves {
            let Val::Int(code) = s[&format!("s.{m}.leaf")] else { panic!("Blattcode") };
            // Ein Sequenzsegment heisst in der MIR `IGNITION.S0`, im Trace steht der Pfad.
            let got = model.leaf_name(m, code).unwrap_or("?");
            assert_eq!(got.rsplit('.').next(), Some(want.as_str()), "{name} t={k}: Zustand von `{m}`");
            compared += 1;
        }
    }
    // Ein Vergleich, der nichts vergleicht, bestaende immer: je Tick
    // mindestens ein Wert.
    assert!(compared > usize::try_from(ticks).unwrap_or(usize::MAX), "{name}: nur {compared} Werte verglichen");
}

/// Stimulus und Ticks je Programm der Suite `beweiser`.
fn case(name: &str) -> Option<(String, u64)> {
    Some(match name {
        // Ein Blinker mit `after` und einem Command.
        "16_timing.takt" => ("t=3 cmd go\nt=40 cmd go\n".to_string(), 60),
        // Ein Range-Fault nimmt in beiden den Fault-Pfad.
        "19_faults.takt" => (String::new(), 10),
        // Ein bewachter `check` mit Inputs.
        "01_minimal.takt" => {
            let pressure = |k: u32| if (10..20).contains(&k) { 70 } else { 40 };
            let mut stim: String = (0..=30).map(|k| format!("t={k} in tank_p {} bar\n", pressure(k))).collect();
            stim.push_str("t=2 cmd start\nt=25 cmd reset\n");
            (stim, 30)
        }
        // Ein Block mit Zustand (5.7): `step` eingebettet, der Zustand in Feldern.
        "48_contracts.takt" => ((0..=12).map(|k| format!("t={k} in level {}\n", f64::from(k) - 4.0)).collect(), 12),
        _ => return None,
    })
}

/// **Jedes Programm der Suite `beweiser` stimmt mit dem Interpreter
/// ueberein.** Die Liste steht im Manifest des Korpus (`Suiten`, FB-378),
/// nicht hier: Ein Programm, das dort in die Suite kommt, braucht hier
/// einen Fall, sonst scheitert der Test, statt es still auszulassen.
#[test]
fn every_program_of_the_prover_suite_agrees() {
    let programs = takt_conformance::suites::programs("beweiser");
    assert!(!programs.is_empty(), "die Suite `beweiser` ist leer");
    for name in programs {
        let (stim, ticks) =
            case(name).unwrap_or_else(|| panic!("`{name}` steht in der Suite `beweiser`, hier fehlt sein Fall"));
        agree(name, &stim, ticks);
    }
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

/// 11.3: Jede implizite Pruefung, die das Modell kennt, ist eine
/// Pruefstelle mit dem Namen aus `takt check --checks`.
#[test]
fn implicit_checks_become_proof_sites() {
    let src = "\
system:
    language = 1
    tick = 1 ms

output n : int in 0..999 @ hw(\"o/n\") with safe = 0

machine m:
    var a : int in 0..200 = 100
    initial RUN
    state RUN:
        loop:
            a = a + 60
            n = a
";
    let options = takt_sema::Options {
        policy: takt_diag::Policy::default(),
        build: takt_sema::Build::Sim,
        profile: None,
        ..Default::default()
    };
    let out = takt_sema::compile(src, &options);
    let p = out.program.expect("Programm");
    let model = encode(&p).expect("kodierbar");
    let site = model.checks.iter().find(|c| c.kind == "range").expect("die Range-Pruefung ist eine Stelle");
    assert_eq!(site.machine, "m");
    assert!(
        out.report.sites.iter().any(|s| s.span.start == site.start),
        "dieselbe Stelle, die `takt check --checks` nennt"
    );
}
