//! Die Kodierung gegen den Interpreter (plan/m6.md 2.8): Das Modell laeuft
//! konkret mit denselben Eingaben, und Outputs wie Zustaende stimmen in
//! jedem Tick ueberein — ohne Solver.

use std::collections::BTreeMap;

use takt_interp::trace::LineKind;
use takt_interp::{RunOptions, Trace, run};
use takt_mir::program::{Binding, Direction};
use takt_mir::types::Type;
use takt_mir::{Program, TypeId};
use takt_prove::{Model, Val, encode, eval};

fn corpus(name: &str) -> Program {
    let path = format!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try/{}"), name);
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    compile(name, &src)
}

fn compile(name: &str, src: &str) -> Program {
    let options = takt_sema::Options {
        policy: takt_diag::Policy::default(),
        build: takt_sema::Build::Sim,
        profile: None,
        ..Default::default()
    };
    let out = takt_sema::compile(src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{name}:\n{}", errors.join("\n"));
    out.program.expect("Programm")
}

/// Die Blaetter eines Werts aus seiner Textform im Trace
/// (`takt_interp::format::display`), mit den Orten, die das Modell fuer sie
/// fuehrt: `Name(a, b)` fuer Records und Varianten mit Feldern, `[a, b]`,
/// `none` fuer ein leeres Optional, sonst sein Wert, Dauern mit Einheit, ein
/// Enum als Index seiner Variante.
fn leaves_of(p: &Program, ty: TypeId, text: &str, base: &str, out: &mut Vec<(String, Val)>) -> Option<()> {
    let text = text.trim();
    match p.types.get(ty) {
        Type::Record(r) => {
            let def = &p.records[r.index()];
            let inner = text.strip_prefix(def.name.as_str())?.strip_prefix('(')?.strip_suffix(')')?;
            for (f, t) in def.fields.iter().zip(split(inner)) {
                leaves_of(p, f.ty, t, &format!("{base}.{}", f.name), out)?;
            }
        }
        Type::Array { elem, .. } => {
            let inner = text.strip_prefix('[')?.strip_suffix(']')?;
            for (i, t) in split(inner).into_iter().enumerate() {
                leaves_of(p, *elem, t, &format!("{base}[{i}]"), out)?;
            }
        }
        Type::Optional(t) => {
            out.push((format!("{base}.has"), Val::Bool(text != "none")));
            if text != "none" {
                leaves_of(p, *t, text, &format!("{base}.value"), out)?;
            }
        }
        Type::Enum(e) => {
            let def = &p.enums[e.index()];
            let (name, args) = match text.split_once('(') {
                Some((n, rest)) => (n, Some(rest.strip_suffix(')')?)),
                None => (text, None),
            };
            let (i, v) = def.variants.iter().enumerate().find(|(_, v)| v.name == name)?;
            if def.variants.iter().all(|v| v.fields.is_empty()) {
                out.push((base.to_string(), Val::Int(i as i64)));
            } else {
                out.push((format!("{base}.tag"), Val::Int(i as i64)));
                for (f, t) in v.fields.iter().zip(args.map(split).unwrap_or_default()) {
                    leaves_of(p, f.ty, t, &format!("{base}.{}.{}", v.name, f.name), out)?;
                }
            }
        }
        Type::Duration { .. } => {
            let (n, unit) = text.split_once(' ')?;
            let factor = match unit {
                "d" => 86_400_000_000_000,
                "h" => 3_600_000_000_000,
                "min" => 60_000_000_000,
                "s" => 1_000_000_000,
                "ms" => 1_000_000,
                "us" => 1_000,
                "ns" => 1,
                _ => return None,
            };
            out.push((base.to_string(), Val::Int(n.parse::<i64>().ok()? * factor)));
        }
        _ => out.push((base.to_string(), parse_val(text)?)),
    }
    Some(())
}

/// Die Teile einer Liste `a, B(c, d), [e]` auf oberster Ebene.
fn split(text: &str) -> Vec<&str> {
    let (mut out, mut depth, mut start) = (Vec::new(), 0i32, 0);
    for (i, c) in text.char_indices() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            ',' if depth == 0 => {
                out.push(text[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    if !text.trim().is_empty() {
        out.push(text[start..].trim());
    }
    out
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
    agree_program(name, &corpus(name), stimulus, ticks);
}

fn agree_program(name: &str, p: &Program, stimulus: &str, ticks: u64) {
    let model: Model = encode(p).unwrap_or_else(|e| panic!("{name}: {}", e.what));
    let stim = Trace::parse(stimulus).expect("Stimulus");
    let r = run(p, &stim, &RunOptions { ticks, ..Default::default() }).expect("Lauf");
    let bound = sim_bindings(p);
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
    let channel_type = |name: &str| p.channels.iter().find(|c| c.name == name).map(|c| c.ty);
    let mut leaves: BTreeMap<String, String> = BTreeMap::new();
    let mut compared = 0usize;
    // Nach `end` stehen die Outputs auf `safe` (12.7); das Modell fuehrt das
    // im Zustand danach, also gelten diese Zeilen dort.
    let mut after_end: Vec<&takt_interp::trace::TraceLine> = Vec::new();
    for k in 0..=ticks {
        let lines: Vec<_> =
            std::mem::take(&mut after_end).into_iter().chain(r.trace.lines.iter().filter(|l| l.tick == k)).collect();
        let mut ended = false;
        for l in lines {
            if ended && l.tick == k {
                after_end.push(l);
                continue;
            }
            match &l.kind {
                LineKind::End { .. } => ended = true,
                LineKind::Output { channel, value } => {
                    let mut leaves = Vec::new();
                    channel_type(channel)
                        .and_then(|ty| leaves_of(p, ty, value, &format!("s.out.{channel}"), &mut leaves))
                        .unwrap_or_else(|| panic!("{name} t={k}: `{value}` von `{channel}` hat keinen Wert"));
                    // Der neue Wert ersetzt den alten ganz, auch die Felder einer anderen Variante.
                    let base = format!("s.out.{channel}");
                    outputs.retain(|loc, _| {
                        loc != &base && !loc.starts_with(&format!("{base}.")) && !loc.starts_with(&format!("{base}["))
                    });
                    outputs.extend(leaves);
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
        for (loc, want) in &outputs {
            let got = s.get(loc).copied().unwrap_or_else(|| panic!("{name} t={k}: `{loc}` fehlt im Modell"));
            assert!(same(got, *want), "{name} t={k}: `{loc}`: Modell {got:?}, Interpreter {want:?}");
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
        // Zusammengesetzte Werte (M11 Schritt 27a): Records, Arrays, Enums
        // mit Feldern, `case` mit Bindungen, Werten und Bereichen.
        "32_next_run_after.takt"
        | "35_persist.takt"
        | "80_payload_variants.takt"
        | "81_persist_variants.takt"
        | "83_durations.takt"
        | "95_boundary_ranges.takt"
        | "96_record_outputs.takt"
        | "113_case_ranges.takt" => (String::new(), 20),
        "89_fault_paths.takt" => ((0..=20).map(|k| format!("t={k} in p {} bar\n", (k * 7) % 100)).collect(), 20),
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

/// Ein Record mit Array, ein Array mit berechnetem Index beim Lesen und
/// Schreiben, ein Optional, eine Funktion ueber Records.
const COMPOSITE: &str = r#"system:
    language = 1
    tick     = 10 ms

record Pair:
    a : int
    b : [3] int

output sum  : int  @ hw("o/sum")  with safe = 0
output pick : int  @ hw("o/pick") with safe = 0
output same : bool @ hw("o/same") with safe = false
output pair : Pair @ sim("o/pair")

fn rotate(q: Pair) -> Pair:
    return Pair(a = q.b[0], b = [q.b[1], q.b[2], q.a % 100])

machine m:
    var k     : int in 0..99 = 0
    var xs    : [4] int in 0..999 = [1, 2, 3, 4]
    var p     : Pair = default
    var maybe : int? = none

    initial RUN

    state RUN:
        loop:
            k = (k + 1) % 100
            xs[k % 4] = (xs[k % 4] + k) % 1000
            p.a = p.a + 1
            p.b[k % 3] = k
            p = rotate(p) if k % 5 == 0 else p
            pair = p
            sum = xs[0] + xs[1] + xs[2] + xs[3]
            if k > 5:
                maybe = k
            same = pair == p and xs[1] != xs[2]
            pick = maybe.or(-1) + xs[k % 4]
"#;

/// **Zusammengesetzte Werte** (M11 Schritt 27a): Records, Arrays mit
/// berechnetem Index, Optionals und Funktionen ueber Records rechnen im
/// Modell wie im Interpreter; ein Index ausserhalb faultet in beiden im
/// selben Tick.
#[test]
fn composite_values_agree() {
    agree_program("composite", &compile("composite", COMPOSITE), "", 30);
    let out_of_range = COMPOSITE.replace("pick = maybe.or(-1) + xs[k % 4]", "pick = xs[k % 7]");
    agree_program("index", &compile("index", &out_of_range), "", 10);
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
