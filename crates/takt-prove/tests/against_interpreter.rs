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
        // Die Laenge, die belegten Plaetze, dahinter null.
        Type::Bytes { cap } => {
            let items = split(text.strip_prefix('[')?.strip_suffix(']')?);
            out.push((format!("{base}.len"), Val::Int(items.len() as i64)));
            for i in 0..*cap as usize {
                let byte = match items.get(i) {
                    Some(t) => i64::from_str_radix(t.strip_prefix("0x")?, 16).ok()?,
                    None => 0,
                };
                out.push((format!("{base}[{i}]"), Val::Int(byte)));
            }
        }
        Type::Vec { elem, .. } => {
            let items = split(text.strip_prefix('[')?.strip_suffix(']')?);
            out.push((format!("{base}.len"), Val::Int(items.len() as i64)));
            for (i, t) in items.into_iter().enumerate() {
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
    // Eingaben des Modells: Commands im Tick ihrer Zeile, Inputs halten,
    // Stromelemente je Tick in ihrer Reihenfolge (8.6) — ohne eigenen
    // Zeitstempel mit dem Ende des Tick-Fensters, `0x` nicht dekodierbar.
    let mut inputs: BTreeMap<(u64, String), Val> = BTreeMap::new();
    let mut held: BTreeMap<String, Val> = BTreeMap::new();
    let element = |channel: &str| {
        let c = p.channels.iter().find(|c| c.name == channel && c.dir == Direction::Input)?;
        match p.types.get(c.ty) {
            Type::Stream(e) => Some(*e),
            _ => None,
        }
    };
    for k in 0..=ticks {
        let mut delivered: BTreeMap<String, i64> = BTreeMap::new();
        for l in stim.lines.iter().filter(|l| l.tick == k) {
            match &l.kind {
                LineKind::Command { name } => {
                    inputs.insert((k, format!("i.cmd.{name}")), Val::Bool(true));
                }
                LineKind::Input { channel, sample } => {
                    if let Some(elem) = element(channel) {
                        let j = delivered.entry(channel.clone()).or_insert(0);
                        let base = format!("i.stream.{channel}.{j}");
                        let boundary = i64::try_from(k).expect("Tick") * p.config.tick;
                        inputs.insert((k, format!("{base}.t")), Val::Int(sample.t.unwrap_or(boundary)));
                        let text = sample.value.as_deref().unwrap_or_default();
                        if text.trim() == "0x" {
                            inputs.insert((k, format!("{base}.bad")), Val::Bool(true));
                        } else {
                            let mut leaves = Vec::new();
                            leaves_of(p, elem, text, &format!("{base}.v"), &mut leaves)
                                .unwrap_or_else(|| panic!("{name} t={k}: `{text}` ist kein Element von `{channel}`"));
                            inputs.extend(leaves.into_iter().map(|(n, v)| ((k, n), v)));
                        }
                        *j += 1;
                    } else if let Some(v) = sample.value.as_deref().and_then(parse_val) {
                        held.insert(format!("i.{channel}"), v);
                    }
                }
                _ => {}
            }
        }
        for (n, v) in &held {
            inputs.insert((k, n.clone()), *v);
        }
        for (s, n) in delivered {
            inputs.insert((k, format!("i.stream.{s}.n")), Val::Int(n));
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
        // Jede Invariante des Modells (Typen, Lemmata, Zaehler der Monitore)
        // gilt in jedem Zustand eines Laufs; eine falsche verschwiege
        // Gegenbeispiele.
        for (i, inv) in model.invariants.iter().enumerate() {
            let holds = eval::eval(inv, &step);
            assert_eq!(holds, Val::Bool(true), "{name} t={k}: Invariante {i} verletzt");
        }
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
        // `check … for 5 ms`: vier Ticks ueber der Grenze faulten nicht,
        // elf schon; dazu ein Start.
        "03_sequences_and_faults.takt" => {
            let chamber = |k: u32| if (10..14).contains(&k) || (20..31).contains(&k) { 260 } else { 10 };
            let mut stim: String = (0..=40)
                .map(|k| format!("t={k} in chamber_p {} bar\nt={k} in supply_p 50 bar\n", chamber(k)))
                .collect();
            stim.push_str("t=2 cmd start\n");
            (stim, 40)
        }
        // `check … for 20 ms within 100 ms`: 25 Ticks ueber der Grenze.
        "14_latency.takt" => {
            let tank = |k: u32| if (10..35).contains(&k) { 390 } else { 100 };
            ((0..=50).map(|k| format!("t={k} in tank_p {} bar\n", tank(k))).collect(), 50)
        }
        "27_every.takt" | "75_implicit_checks.takt" => (String::new(), 40),
        // Interne Stroeme (Schritt 27c): Ring, Cursor je Leser, Handler je Ebene.
        "106_machine_handler.takt" | "72_handler_levels.takt" | "53_stream_kinds.takt" | "88_capture_segments.takt" => {
            (String::new(), 40)
        }
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

/// Ein Record-Strom vom Rand (8.6, 8.7): ein Record-Muster, ein Handler
/// mit Guard, ein Element, das sich nicht dekodieren laesst, ein Uebergang
/// mitten im Fenster, dessen Rest der Folgezustand liest, und zuletzt ein
/// Ueberlauf, der den Leser faultet.
const INPUT_STREAM: &str = r#"system:
    language = 1
    tick     = 10 ms

record Frame:
    id   : u8
    flag : bool

input  rx : stream<Frame> @ hw("bus/rx") with max_rate = 300 Hz, capacity = 4

output seen   : int in 0..9999  @ sim("seen")
output last   : int in 0..255   @ sim("last")
output sevens : int in 0..9999  @ sim("sevens")
output stats  : int in 0..99999 @ sim("stats")
output phase  : int in 0..9     @ sim("phase")
output stamp  : Duration        @ sim("stamp")

machine reader:
    var n : int in 0..9999 = 0
    var s : int in 0..9999 = 0

    initial LISTEN

    loop:
        stats = min(rx.dropped + rx.overflowed * 10 + rx.malformed * 100 + rx.count * 1000, 99999)

    state LISTEN:
        enter:
            phase = 1

        on rx matches Frame(id = 7) as e:
            s = (s + 1) % 10000
            sevens = s
            stamp = e.t

        on rx as e when e.data.flag:
            n = (n + 1) % 10000
            seen = n
            last = e.data.id as int
            if e.data.id == 9:
                -> HOLD

    state HOLD:
        enter:
            phase = 2
        after 30 ms: -> LISTEN
"#;

const INPUT_STREAM_STIMULUS: &str = "t=1 in rx Frame(7, false)\nt=1 in rx Frame(3, true)\nt=2 in rx Frame(7, true) t=15000000\nt=3 in rx Frame(5, false)\nt=4 in rx 0x\nt=4 in rx Frame(4, true)\nt=5 in rx Frame(9, true)\nt=5 in rx Frame(1, true)\nt=5 in rx Frame(2, true)\nt=6 in rx Frame(6, true)\nt=12 in rx Frame(7, false) t=118000000\nt=20 in rx Frame(9, true)\nt=21 in rx Frame(1, true)\nt=21 in rx Frame(2, true)\nt=21 in rx Frame(3, true)\nt=22 in rx Frame(4, true)\nt=22 in rx Frame(5, true)\nt=22 in rx Frame(6, true)\n";

#[test]
fn an_input_stream_agrees() {
    agree_program("INPUT_STREAM", &compile("INPUT_STREAM", INPUT_STREAM), INPUT_STREAM_STIMULUS, 30);
}

/// Das Fenster eines Bytestroms (8.6, 9.6): `count`, `peek`, `skip`, `for`
/// mit `break`, `drop_oldest`, ein `idle`-Zustand, der verwirft, und ein
/// Strom, der aus ihm weckt.
const STREAM_WINDOW: &str = r#"system:
    language = 1
    tick     = 10 ms

input  data : stream<u8> @ hw("bus/data") with max_rate = 200 Hz, capacity = 3, overflow = drop_oldest
input  bell : stream<u8> @ hw("bus/bell") with max_rate = 100 Hz, capacity = 2, wake = true

output total  : int in 0..99999 @ sim("total")
output peeked : int in 0..999   @ sim("peeked")
output counts : int in 0..999   @ sim("counts")
output missed : int in 0..999   @ sim("missed")
output rung   : int in 0..999   @ sim("rung")
output naps   : int in 0..999   @ sim("naps")

machine summer:
    var sum : int in 0..99999 = 0
    var r   : int in 0..999 = 0
    var z   : int in 0..999 = 0

    initial ACTIVE

    state ACTIVE:
        loop:
            counts = data.count
            peeked = data.peek().or(0) as int
            if data.count == 3 and sum < 70:
                data.skip()
            for b in data:
                sum = (sum + (b.data as int)) % 100000
                if b.data == 5:
                    break
            total = sum
            missed = min(data.dropped, 999)

        on bell as e:
            r = min(r + 1, 999)
            rung = r

        when data.count == 0 and sum > 50 and z < 2: -> NAP

    state NAP idle:
        enter:
            z = min(z + 1, 999)
            naps = z
        when bell.count > 0: -> ACTIVE
        after 40 ms: -> ACTIVE
"#;

const STREAM_WINDOW_STIMULUS: &str = "t=1 in data 1\nt=1 in data 5\nt=2 in data 2\nt=2 in data 3\nt=3 in data 4\nt=3 in data 6\nt=4 in data 7\nt=5 in data 9\nt=5 in data 10\nt=6 in data 11\nt=9 in data 12\nt=9 in data 13\nt=10 in data 14\nt=10 in data 15\nt=11 in bell 1\nt=13 in data 16\nt=15 in bell 2\nt=20 in data 5\nt=20 in data 7\nt=21 in data 8\nt=21 in data 9\nt=30 in data 5\nt=30 in data 5\nt=31 in data 5\nt=31 in data 5\nt=32 in data 5\nt=32 in data 5\nt=40 in bell 3\n";

#[test]
fn a_stream_window_agrees() {
    agree_program("STREAM_WINDOW", &compile("STREAM_WINDOW", STREAM_WINDOW), STREAM_WINDOW_STIMULUS, 45);
}

/// Ein interner Strom (8.6, 9.6): Record-Muster und `s as e` als Guard, eine
/// Bindung in den Aktionen, `until … timeout` in einer Sequenz und ein
/// `send`, der ueberlaeuft und den Schreiber faultet.
const INTERNAL_STREAM: &str = r#"system:
    language = 1
    tick     = 10 ms

record Msg:
    kind : int in 0..9
    v    : int in 0..999

input  burst : int in 0..5 @ hw("in/burst")
stream<Msg> q with capacity = 4, overflow = fault

output got    : int in 0..999  @ sim("got")
output others : int in 0..9999 @ sim("others")
output waited : int in 0..999  @ sim("waited")
output sent   : int in 0..9999 @ sim("sent")

machine writer:
    var k : int in 0..999 = 0
    var n : int in 0..9999 = 0

    initial RUN

    state RUN:
        loop:
            if burst >= 1:
                send q, Msg(kind = 1, v = k)
            if burst >= 2:
                send q, Msg(kind = 3, v = k)
            if burst >= 3:
                send q, Msg(kind = 2, v = k)
            if burst >= 4:
                send q, Msg(kind = 3, v = k + 1)
            n = (n + burst) % 10000
            sent = n
            k = (k + 1) % 900

machine taker:
    var c : int in 0..9999 = 0
    var w : int in 0..999 = 0

    initial WAIT

    state WAIT:
        when q matches Msg(kind = 3) as m:
            got = m.data.v
            -> PAUSE
        when q as e:
            c = (c + 1) % 10000
            others = c
            -> WAIT

    state PAUSE:
        sequence:
            until q as e timeout 30 ms
            w = min(e.data.v, 999)
            waited = w
            -> WAIT
"#;

#[test]
fn an_internal_stream_agrees() {
    let burst = |k: u32| match k {
        1 | 10 => 1,
        3 => 2,
        6 => 3,
        20 | 21 => 4,
        _ => 0,
    };
    let stim: String = (0..=30).map(|k| format!("t={k} in burst {}\n", burst(k))).collect();
    agree_program("INTERNAL_STREAM", &compile("INTERNAL_STREAM", INTERNAL_STREAM), &stim, 30);
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

/// Bytes und ein Vektor: anhaengen bis zur Kapazitaet, alles oder nichts,
/// leeren, lesen mit Index, Laenge, `for` ueber die belegten Plaetze.
const COLLECTIONS: &str = r#"system:
    language = 1
    tick     = 10 ms

output n     : int  @ hw("o/n")     with safe = 0
output total : int  @ hw("o/total") with safe = 0
output took  : bool @ hw("o/took")  with safe = false
output first : int  @ hw("o/first") with safe = 0
output buf   : bytes<6> @ sim("o/buf")

machine m:
    var k  : int in 0..99 = 0
    var b  : bytes<6> = default
    var vs : vec<int, 3> = default

    initial RUN

    state RUN:
        loop:
            k = (k + 1) % 100
            took = b.push((k % 256) as u8)
            if k % 4 == 0:
                var pair : bytes<6> = [1, 2]
                took = b.append(pair)
            if k % 7 == 0:
                b.clear()
            vs.push(k)
            if k % 5 == 0:
                vs.clear()
            n = b.len * 10 + vs.len
            var sum : int = 0
            for x in b:
                sum = sum + x as int
            for y in vs:
                sum = sum + y
            total = sum
            first = b[0] as int if b.len > 0 else -1
            buf = b
"#;

/// **Sammlungen** (M11 Schritt 27a): Bytes und Vektoren rechnen im Modell
/// wie im Interpreter; ein Index hinter der Laenge faultet in beiden.
#[test]
fn collections_agree() {
    agree_program("collections", &compile("collections", COLLECTIONS), "", 30);
    let past_len = COLLECTIONS.replace("first = b[0] as int if b.len > 0 else -1", "first = b[k % 6] as int");
    agree_program("past_len", &compile("past_len", &past_len), "", 12);
}

/// `m.state` in einem Segment einer Sequenz ist im Interpreter und im
/// erzeugten Code die erste Variante; das Modell hielt den Index des
/// Segments, und der fiel mit dem Code von `FAULTED` zusammen.
const SEQUENCE_STATE: &str = r#"system:
    language = 1
    tick     = 10 ms

output phase : int @ hw("o/phase") with safe = 0

machine a:
    initial IDLE

    state IDLE:
        after 20 ms: -> RUN

    state RUN:
        sequence:
            wait 30 ms
            wait 30 ms
            -> IDLE

machine b:
    initial WATCH

    state WATCH:
        loop:
            phase = 1 if a.state == RUN else (2 if a.state == IDLE else (3 if a.state == FAULTED else 0))
"#;

#[test]
fn the_state_of_a_sequence_segment_agrees() {
    agree_program("sequence_state", &compile("sequence_state", SEQUENCE_STATE), "", 20);
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
