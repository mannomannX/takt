//! Metamorphe Relationen ueber den erzeugten Eingaben (M11 Schritt 29b,
//! FB-382): Orakel, die nicht aus derselben Quelle rechnen wie die Laeufe,
//! die sie pruefen.
//!
//! Der Vergleich der drei Ausfuehrer (`differential.rs`) sieht keinen
//! Fehler, den alle teilen — einen in Sema oder MIR, eine gemeinsame
//! Fehllesart der Referenz. Eine Relation stellt dasselbe Programm zweimal
//! unter Bedingungen, deren Ergebnis die Referenz gleichsetzt, und verlangt
//! gleiche Traces. Je Korpusprogramm und Lauf ([`takt_conformance::cases`]):
//!
//! - **Reihenfolge der Schritte** (Satz 9.4.1): Der Trace ist eine Funktion
//!   der Inputs, nicht der Reihenfolge, in der die Maschinen schreiten.
//! - **Schlaf** (Satz 9.9.1): Mit Schlaf ueber ruhige Ticks entsteht
//!   derselbe Trace wie ohne.
//! - **`sim`- gegen `hw`-Bindung** (8.3): Was ein Modell ueber eine
//!   `sim`-Bindung liefert, wirkt wie derselbe Wert vom Rand. Der
//!   Hardware-Build laesst die Modelle weg; seine Eingaben sind die
//!   Lieferungen des Simulationslaufs.
//!
//! Die IEEE-Umgebung des Wirts (4.2, `embed.rs`) und Praefix und Instanzen
//! (12.11, `several_programs.rs`) gehoeren dem Produktrahmen; sie laufen dort
//! ueber den Korpus ohne Eingaben, ueber die erzeugten erst, wenn der
//! Produktrahmen einen Stimulus treibt (M11 Schritt 9).

use std::collections::BTreeSet;

use takt_interp::trace::{LineKind, SampleText, TraceLine, render_line, value_text};
use takt_interp::value::{Quality, Sample};
use takt_interp::{Run, RunOptions, RunResult, Trace};
use takt_mir::program::{Binding, Direction, Program};
use takt_mir::types::Type;
use takt_mir::{ChannelId, MachineId};
use takt_rt_core::{Clock, Policy, Profile, Runtime, Watchdog};
use takt_sema::Build;

/// Die Programme: die Suite `vergleich` und die Beispiele, wie im Vergleich
/// der drei Ausfuehrer.
fn korpus() -> Vec<&'static str> {
    let mut out = takt_conformance::suites::programs("vergleich");
    out.extend(takt_conformance::suites::EXAMPLES);
    out
}

/// Die Startwerte der permutierten Reihenfolgen.
const SEEDS: [u64; 2] = [1, 0x9e37_79b9];

/// Das Programm `name` aus `corpus-try/`, fuer `build` uebersetzt; `Err`
/// mit den Fehlern der Sema.
fn compile(name: &str, build: Build) -> Result<Program, String> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus-try").join(name);
    let src = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let dir = path.parent().unwrap_or(std::path::Path::new("."));
    let mut channel_imports = std::collections::BTreeMap::new();
    for file in takt_sema::channel_imports(&src) {
        let text = std::fs::read_to_string(dir.join(&file)).map_err(|e| format!("{file}: {e}"))?;
        channel_imports.insert(file, text);
    }
    let options = takt_sema::Options { build, channel_imports, ..Default::default() };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    match out.program {
        Some(p) if errors.is_empty() => Ok(p),
        _ => Err(errors.join("\n")),
    }
}

/// Ein Lauf des Interpreters; `Err` nennt den Abbruch.
fn run(p: &Program, stimulus: &Trace, options: &RunOptions) -> Result<RunResult, String> {
    takt_interp::run(p, stimulus, options).map_err(|e| format!("{e:?}"))
}

/// Die erste Abweichung zweier Traces, Zeile fuer Zeile.
fn first_difference(a: &str, b: &str) -> Option<String> {
    let (la, lb): (Vec<&str>, Vec<&str>) = (a.lines().collect(), b.lines().collect());
    (0..la.len().max(lb.len())).find(|&i| la.get(i) != lb.get(i)).map(|i| {
        let line = |l: &[&str]| l.get(i).copied().unwrap_or("(Ende)").to_string();
        format!("Zeile {}: `{}` gegen `{}`", i + 1, line(&la), line(&lb))
    })
}

/// Rechnet `check` fuer jedes Programm des Korpus auf Faeden; ein Kern
/// bleibt frei. `check` liefert die Befunde eines Programms.
fn each_program(check: impl Fn(&'static str) -> Vec<String> + Sync) -> Vec<String> {
    let queue = std::sync::Mutex::new(korpus());
    let found = std::sync::Mutex::new(Vec::new());
    let cores = std::thread::available_parallelism().map_or(2, |n| n.get());
    std::thread::scope(|scope| {
        for _ in 0..(cores - 1).clamp(1, 4) {
            let worker = || {
                loop {
                    let next = queue.lock().expect("Warteschlange").pop();
                    let Some(name) = next else { break };
                    let out = check(name);
                    found.lock().expect("Befunde").extend(out);
                }
            };
            // Sema und Interpreter gehen tief: 64 MiB Stapel je Faden.
            std::thread::Builder::new().stack_size(64 << 20).spawn_scoped(scope, worker).expect("Faden");
        }
    });
    let mut found = found.into_inner().expect("Befunde");
    found.sort();
    found
}

/// Die Laeufe eines Programms mit ihrem Stimulus.
fn cases(name: &str, p: &Program) -> Vec<(takt_conformance::cases::Case, Trace)> {
    takt_conformance::cases::cases(name, p)
        .into_iter()
        .map(|c| {
            let stimulus = Trace::parse(&c.stimulus).unwrap_or_else(|e| panic!("{name} ({}): {e}", c.label));
            (c, stimulus)
        })
        .collect()
}

/// **Die Reihenfolge der Schritte aendert nichts** (Satz 9.4.1): Jeder Lauf
/// jedes Programms ergibt mit permutierter Schrittordnung — wo die Referenz
/// sie freilaesst, also ausser `follows` — denselben Trace.
#[test]
fn the_order_of_the_steps_changes_nothing() {
    let failed = each_program(|name| {
        let p = compile(name, Build::Sim).unwrap_or_else(|e| panic!("{name}: {e}"));
        let mut out = Vec::new();
        for (case, stimulus) in cases(name, &p) {
            let options = RunOptions { ticks: case.ticks, ..Default::default() };
            let base = run(&p, &stimulus, &options).map(|r| r.trace.render());
            for seed in SEEDS {
                let permuted = RunOptions { order_seed: Some(seed), ..options.clone() };
                match (&base, run(&p, &stimulus, &permuted).map(|r| r.trace.render())) {
                    (Ok(a), Ok(b)) if *a == b => {}
                    (Err(a), Err(b)) if *a == b => {}
                    (Ok(a), Ok(b)) => out.push(format!(
                        "{name} ({}), Reihenfolge {seed:#x}: {}",
                        case.label,
                        first_difference(a, &b).unwrap_or_default()
                    )),
                    (a, b) => out.push(format!("{name} ({}), Reihenfolge {seed:#x}: {a:?} gegen {b:?}", case.label)),
                }
            }
        }
        out
    });
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

/// Eine Uhr, die nur springt, wenn die Schleife wartet: Schlaf kostet
/// keine Zeit des Tests.
struct Virtual(i64);

impl Clock for Virtual {
    fn now(&self) -> i64 {
        self.0
    }

    fn wait_until(&mut self, deadline: i64) {
        self.0 = self.0.max(deadline);
    }
}

struct Quiet;

impl Watchdog for Quiet {
    fn kick(&mut self) {}
}

/// Ein Lauf in der Schleife aus 12.1 ueber `ticks` Ticks; mit `may_sleep`
/// verschlaeft sie ruhige Ticks (9.9).
fn looped(p: &Program, stimulus: &Trace, ticks: u64, may_sleep: bool) -> Result<RunResult, String> {
    let run = Run::new(p, stimulus, &RunOptions { ticks, ..Default::default() }).map_err(|e| format!("{e:?}"))?;
    let profile = Profile { may_sleep, ..Profile::LINUX_RT };
    let mut rt = Runtime::new(run, Virtual(0), Quiet, (), profile, p.config.tick, Policy::default());
    while rt.tick_number() < ticks {
        rt.step();
    }
    rt.program.finish().map_err(|e| format!("{e:?}"))
}

/// **Schlaf aendert nichts** (Satz 9.9.1): Jeder Lauf ergibt in der
/// Schleife, die ruhige Ticks verschlaeft, denselben Trace wie in der, die
/// jeden rechnet. Ohne verschlafenen Tick waere die Relation leer; die
/// Laeufe muessen zusammen schlafen.
#[test]
fn sleeping_changes_nothing() {
    let slept = std::sync::atomic::AtomicU64::new(0);
    let failed = each_program(|name| {
        let p = compile(name, Build::Sim).unwrap_or_else(|e| panic!("{name}: {e}"));
        let mut out = Vec::new();
        for (case, stimulus) in cases(name, &p) {
            let (awake, asleep) = (looped(&p, &stimulus, case.ticks, false), looped(&p, &stimulus, case.ticks, true));
            match (awake, asleep) {
                (Ok(awake), Ok(asleep)) => {
                    slept.fetch_add(asleep.slept, std::sync::atomic::Ordering::Relaxed);
                    if awake.slept > 0 {
                        out.push(format!("{name} ({}): ohne Schlaf {} Ticks verschlafen", case.label, awake.slept));
                    }
                    if let Some(d) = first_difference(&awake.trace.render(), &asleep.trace.render()) {
                        out.push(format!("{name} ({}): mit Schlaf anders als ohne: {d}", case.label));
                    }
                }
                (Err(a), Err(b)) if a == b => {}
                (a, b) => out.push(format!(
                    "{name} ({}): ohne Schlaf {}, mit {}",
                    case.label,
                    a.map_or_else(|e| e, |_| "ok".into()),
                    b.map_or_else(|e| e, |_| "ok".into())
                )),
            }
        }
        out
    });
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    assert!(slept.into_inner() > 0, "kein Lauf hat geschlafen; die Relation prueft nichts");
}

/// Die skalaren `hw`-Inputs, die ein Modell ueber eine `sim`-Bindung speist
/// (8.3); `None`, wenn das Programm ausserhalb der Relation liegt: ohne
/// solchen Input, mit einem Strom, den ein Modell speist — seine Elemente
/// tragen die Commit-Zeit des Modells, keinen Wert —, oder mit einem
/// Registerport, dessen Modell im Hardware-Build fehlt (12.10).
fn sim_fed(p: &Program) -> Option<Vec<ChannelId>> {
    if !p.ports.is_empty() {
        return None;
    }
    let mut fed = Vec::new();
    for (i, inp) in p.channels.iter().enumerate() {
        let Binding::Hw(addr) = &inp.binding else { continue };
        if inp.dir != Direction::Input {
            continue;
        }
        let modelled = p.channels.iter().any(|o| o.dir == Direction::Output && o.binding == Binding::Sim(addr.clone()));
        if !modelled {
            continue;
        }
        if matches!(p.types.get(inp.ty), Type::Stream(_)) {
            return None;
        }
        fed.push(ChannelId(i as u32));
    }
    (!fed.is_empty()).then_some(fed)
}

/// Die Lieferung eines Modells als Zeile des Stimulus (12.5): ihr Wert,
/// gut, an der Tickgrenze (`Image::apply_sim_bindings`); `None` fuer die
/// Lieferung eines Treibers, der seinen Vertrag brach.
fn delivered(sample: &Sample, ty: takt_mir::TypeId, p: &Program) -> Option<SampleText> {
    let value = sample.value.as_ref().filter(|_| sample.quality == Quality::Good)?;
    Some(SampleText {
        value: Some(value_text(value, ty, p)),
        quality: None,
        reason: None,
        age: None,
        t: None,
        seq: None,
    })
}

/// Gehoert die Zeile zum Steuerprogramm? Was die Modelle tun — ihre
/// Zustaende, Faults, Ausgaben —, kennt der Hardware-Build nicht.
fn of_the_program(line: &TraceLine, models: &BTreeSet<String>, model_outputs: &BTreeSet<String>) -> bool {
    match &line.kind {
        LineKind::State { machine, .. }
        | LineKind::Published { machine, .. }
        | LineKind::Signal { machine, .. }
        | LineKind::Job { machine, .. }
        | LineKind::Fault { machine, .. }
        | LineKind::Log { machine, .. }
        | LineKind::Alert { machine, .. }
        | LineKind::Measure { machine, .. }
        | LineKind::Verify { machine, .. }
        | LineKind::Verdict { machine, .. } => !models.contains(machine),
        LineKind::Output { channel, .. } => !model_outputs.contains(channel),
        _ => true,
    }
}

/// **Ein Wert ueber die `sim`-Bindung wirkt wie derselbe Wert vom Rand**
/// (8.3): Der Simulationslauf zeichnet je Tick auf, was die Modelle an den
/// `hw`-Inputs liefern; der Hardware-Build bekommt genau das als Stimulus,
/// ohne Modelle. Was das Steuerprogramm tut, ist in beiden Laeufen gleich.
/// Programme, deren Hardware-Build die Sema ablehnt, liegen ausserhalb.
#[test]
fn a_value_over_a_sim_binding_acts_like_the_same_value_from_the_edge() {
    let checked = std::sync::atomic::AtomicUsize::new(0);
    let failed = each_program(|name| {
        let p = compile(name, Build::Sim).unwrap_or_else(|e| panic!("{name}: {e}"));
        let Some(fed) = sim_fed(&p) else { return Vec::new() };
        let Ok(hw) = compile(name, Build::Hw) else { return Vec::new() };
        checked.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let models: BTreeSet<String> = (0..p.machines.len())
            .filter(|&i| p.is_plant_model(MachineId(i as u32)))
            .map(|i| p.machines[i].name.clone())
            .collect();
        let model_outputs: BTreeSet<String> = p
            .channels
            .iter()
            .filter(|c| c.owner.is_some_and(|m| models.contains(&p.machines[m.index()].name)))
            .map(|c| c.name.clone())
            .collect();
        let program_lines = |t: &Trace| -> String {
            t.lines
                .iter()
                .filter(|l| of_the_program(l, &models, &model_outputs))
                .map(|l| format!("{}\n", render_line(l)))
                .collect()
        };
        let mut out = Vec::new();
        for (case, stimulus) in cases(name, &p) {
            let options = RunOptions { ticks: case.ticks, inputs: true, ..Default::default() };
            let simulated = match run(&p, &stimulus, &options) {
                Ok(r) => r,
                Err(e) => {
                    out.push(format!("{name} ({}): der Simulationslauf bricht ab: {e}", case.label));
                    continue;
                }
            };
            // Was der Stimulus selbst setzt, behaelt der Input (8.3); der Rest
            // sind die Lieferungen der Modelle.
            let set: BTreeSet<(u64, String)> = stimulus
                .lines
                .iter()
                .filter_map(|l| match &l.kind {
                    LineKind::Input { channel, .. } => Some((l.tick, channel.clone())),
                    _ => None,
                })
                .collect();
            let mut replay = stimulus.clone();
            for (tick, seen) in simulated.inputs.iter().enumerate() {
                let tick = tick as u64;
                for c in &fed {
                    let channel = p.channels[c.index()].name.clone();
                    let Some(sample) = seen.get(c.index()).and_then(|s| s.delivery.as_ref()) else { continue };
                    if set.contains(&(tick, channel.clone())) {
                        continue;
                    }
                    // Ein Treiber, der seinen Vertrag brach, liefert `Bad` mit
                    // Grund `Driver` (12.6, Zeile 2); das tut er im Hardware-Build
                    // aus demselben Stimulus.
                    if let Some(sample) = delivered(sample, p.channels[c.index()].ty, &p) {
                        replay.lines.push(TraceLine { tick, kind: LineKind::Input { channel, sample } });
                    }
                }
            }
            replay.lines.sort_by_key(|l| l.tick);
            let replayed = match run(&hw, &replay, &RunOptions { ticks: case.ticks, ..Default::default() }) {
                Ok(r) => r,
                Err(e) => {
                    out.push(format!("{name} ({}): der Hardware-Build bricht ab: {e}", case.label));
                    continue;
                }
            };
            let (a, b) = (program_lines(&simulated.trace), program_lines(&replayed.trace));
            if let Some(d) = first_difference(&a, &b) {
                out.push(format!("{name} ({}): `sim` gegen `hw`: {d}", case.label));
            }
        }
        out
    });
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    // Eine Relation, die kein Programm trifft, bestaende immer.
    let checked = checked.into_inner();
    assert!(checked >= 5, "nur {checked} Programme mit Modell an einer `sim`-Bindung");
}
