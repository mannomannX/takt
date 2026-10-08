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

/// Die Blaetter eines Texts oder einer Bytefolge im Stimulus: Text wie
/// `parse_value` ihn liest (eine Zeile ueber ihrer Kapazitaet gekuerzt),
/// Bytes als `0x…`; `None` fuer andere Typen.
fn text_leaves(p: &Program, ty: TypeId, text: &str, base: &str) -> Option<Vec<(String, Val)>> {
    let (bytes, cap, truncated) = match p.types.get(ty) {
        Type::Str { cap } | Type::Line { cap } => match takt_interp::trace::parse_value(text, ty, p).ok()? {
            takt_interp::Value::Str(s) => (s.into_bytes(), *cap, None),
            takt_interp::Value::Line { text, truncated } => (text.into_bytes(), *cap, Some(truncated)),
            _ => return None,
        },
        Type::Bytes { cap } => {
            let hex = text.trim().strip_prefix("0x")?;
            let bytes: Option<Vec<u8>> =
                (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok()).collect();
            (bytes?, *cap, None)
        }
        _ => return None,
    };
    let mut out = vec![(format!("{base}.len"), Val::Int(bytes.len() as i64))];
    for i in 0..cap as usize {
        out.push((format!("{base}[{i}]"), Val::Int(bytes.get(i).copied().map_or(0, i64::from))));
    }
    if let Some(t) = truncated {
        out.push((format!("{base}.truncated"), Val::Bool(t)));
    }
    Some(out)
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
        // Der Trace schreibt ein `f32` in seiner kuerzesten Form; gelesen wird es in `f32`.
        (Val::F32(x), Val::F64(y)) | (Val::F64(y), Val::F32(x)) => x.to_bits() == (y as f32).to_bits(),
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
                        } else if let Some(leaves) = text_leaves(p, elem, text, &format!("{base}.v")) {
                            inputs.extend(leaves.into_iter().map(|(n, v)| ((k, n), v)));
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
            .map(|v| v.name.clone())
            .zip(eval::eval_all(model.state.iter().map(|v| if k == 0 { &v.init } else { &v.next }), &env))
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
    // Ausgabestroeme: Was der Treiber abholt, steht je Tick als eigene Zeile
    // (8.8); ohne Zeile hat er nichts abgeholt.
    let tx: Vec<String> = p
        .channels
        .iter()
        .filter(|c| c.dir == Direction::Output && matches!(p.types.get(c.ty), Type::Stream(_)))
        .map(|c| c.name.clone())
        .collect();
    for k in 0..=ticks {
        let lines: Vec<_> =
            std::mem::take(&mut after_end).into_iter().chain(r.trace.lines.iter().filter(|l| l.tick == k)).collect();
        let mut ended = false;
        let mut sent: BTreeMap<String, Vec<i64>> = tx.iter().map(|o| (o.clone(), Vec::new())).collect();
        for l in lines {
            if ended && l.tick == k {
                after_end.push(l);
                continue;
            }
            match &l.kind {
                LineKind::End { .. } => ended = true,
                LineKind::Output { channel, value } if tx.contains(channel) => {
                    let items = split(value.strip_prefix('[').and_then(|v| v.strip_suffix(']')).unwrap_or(value));
                    let bytes = items.iter().map(|b| i64::from_str_radix(b.trim_start_matches("0x"), 16));
                    let bytes: Result<Vec<i64>, _> = bytes.collect();
                    sent.insert(channel.clone(), bytes.unwrap_or_else(|_| panic!("{name} t={k}: `{value}`")));
                }
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
        for (o, bytes) in &sent {
            let got = |part: &str| s.get(&format!("s.tx.{o}.{part}")).copied();
            assert_eq!(got("sent.len"), Some(Val::Int(bytes.len() as i64)), "{name} t={k}: Laenge von `{o}`");
            for (j, b) in bytes.iter().enumerate() {
                assert_eq!(got(&format!("sent[{j}]")), Some(Val::Int(*b)), "{name} t={k}: Byte {j} von `{o}`");
            }
            compared += 1;
        }
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
        // Zeilen vom Rand (Schritt 27c-2): je Tick eine, wie `MAXPT` erlaubt.
        "100_dispatch.takt" => {
            let lines = [
                "code 42",
                "code 600",
                "x ERR 12345 y",
                "WARN now",
                "key:value",
                "plain",
                "ERR -7",
                "a:b:c",
                "äöü:wörd",
                "code 499",
                "code -5",
            ];
            (lines.iter().enumerate().map(|(k, l)| format!("t={} in rx {l:?}\n", k + 1)).collect(), 20)
        }
        // Der letzte Sektor liegt ausserhalb von `last` und faultet im Handler.
        "23_patterns.takt" => {
            let lines = ["READY", "Erasing sector 12", "noise", "Erasing sector 7", "Erasing sector 1234"];
            (lines.iter().enumerate().map(|(k, l)| format!("t={} in rx_log {l:?}\n", 2 * k + 1)).collect(), 16)
        }
        "49_record_streams.takt" => (
            "t=1 in edges Pulse(true, 3)\nt=2 in edges Pulse(false, 2)\nt=3 in edges Pulse(false, 7)\n\
             t=5 in rx \"go 5\"\nt=6 in rx \"go 12\" t=55000000\nt=7 in rx \"no\"\nt=8 in rx \"stop\"\n\
             t=9 in edges Pulse(true, 9)\n"
                .to_string(),
            14,
        ),
        "104_linear_has.takt" | "117_many_text_handlers.takt" | "51_text_into_bytes.takt" => (String::new(), 30),
        // Funktionen aus `libtaktm`, Einheiten und Bits (Schritt 27a-3).
        "101_correct_math.takt"
        | "102_correct_math_f32.takt"
        | "103_math_domains.takt"
        | "115_affine_unit.takt"
        | "67_bitfield_access.takt"
        | "86_units.takt"
        | "97_fast_math.takt" => (String::new(), 20),
        // Ergebnisse, `every` und `check … for` in einer Schleife (Schritt 27a-4).
        "84_defaults.takt" | "93_confirmations.takt" | "54_inout.takt" => (String::new(), 40),
        // Eine Tabelle mit `interp`: die Zellspannung laeuft ueber alle Abschnitte.
        "02_units_and_data.takt" => {
            let stim: String = (0..=40)
                .map(|k| {
                    format!(
                        "t={k} in oven_t {} degC
t={k} in cell_v {} V
",
                        150 + k * 3,
                        2.8 + f64::from(k) * 0.04
                    )
                })
                .collect();
            (stim, 40)
        }
        // Generische Funktionen mit Schleifen und `break` (Schritt 27a-3).
        "62_type_generics.takt" => (
            (0..=20)
                .map(|k| {
                    format!(
                        "t={k} in a {}
t={k} in b {}.5 V
",
                        (k * 13) % 101,
                        k % 5
                    )
                })
                .collect(),
            20,
        ),
        // Ein Antrieb mit Park-Transformation, Sinus und Kosinus; die Eingaben
        // je Tick, denn sie veralten nach zwei.
        "11_foc_drive.takt" => {
            let mut stim = String::new();
            for k in 0..=40 {
                let theta = f64::from(k % 60) * 0.1;
                stim.push_str(&format!(
                    "t={k} in i_u 1.5 A\nt={k} in i_v -0.5 A\nt={k} in v_dc 48 V\nt={k} in theta_elec {theta}\n\
                     t={k} in omega_mech 100 1/s\nt={k} in temp_inverter 25 degC\nt={k} in temp_motor 30 degC\n"
                ));
            }
            stim.push_str("t=2 cmd cmd_start\n");
            (stim, 40)
        }
        // Bytes vom Bus: der Handler setzt ein Bitfeld.
        "12_bitfields.takt" => ((1..=8).map(|k| format!("t={k} in can_rx {}\n", k * 3)).collect(), 20),
        // Ausgabestroeme (Schritt 27c-3): Sendepuffer, Abholen je Tick,
        // `free`, `idle`, `sent`, ein `sim`-gespeister Eingabestrom.
        "24_send_has.takt" => {
            let lines = ["no error", "an ERR here", "ERR", "plain", "ERRERR"];
            (lines.iter().enumerate().map(|(k, l)| format!("t={} in rx {l:?}\n", 2 * k + 1)).collect(), 14)
        }
        "43_sent.takt" => ("t=2 cmd go\n".to_string(), 12),
        "25_format.takt" | "79_byte_literals.takt" | "92_idle_streams.takt" | "118_tx_idle.takt" => (String::new(), 40),
        // Interne Stroeme (Schritt 27c): Ring, Cursor je Leser, Handler je Ebene.
        "106_machine_handler.takt" | "72_handler_levels.takt" | "53_stream_kinds.takt" | "88_capture_segments.takt" => {
            (String::new(), 40)
        }
        // Drahtformat und Ausschnitte (Schritt 27a-5).
        "52_padding_fields.takt" | "13_framing.takt" | "76_stream_views.takt" => (String::new(), 30),
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

const INPUT_STREAM_STIMULUS: &str = "t=0 in rx Frame(8, true)\nt=1 in rx Frame(7, false)\nt=1 in rx Frame(3, true)\nt=2 in rx Frame(7, true) t=15000000\nt=3 in rx Frame(5, false)\nt=4 in rx 0x\nt=4 in rx Frame(4, true)\nt=5 in rx Frame(9, true)\nt=5 in rx Frame(1, true)\nt=5 in rx Frame(2, true)\nt=6 in rx Frame(6, true)\nt=12 in rx Frame(7, false) t=118000000\nt=20 in rx Frame(9, true)\nt=21 in rx Frame(1, true)\nt=21 in rx Frame(2, true)\nt=21 in rx Frame(3, true)\nt=22 in rx Frame(4, true)\nt=22 in rx Frame(5, true)\nt=22 in rx Frame(6, true)\n";

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

/// Text (3.9, 8.7): Zeilen vom Rand mit Mustern ueber `int`, `hex`, `word`,
/// `str<N>` und `{_}`, `matches` und `has`, ein Ueberlauf in `int`, Text
/// ausserhalb von ASCII, eine gekuerzte Zeile, `starts_with`, `contains`,
/// ein Formatstring in einen internen Zeilenstrom und `matches … as m` auf
/// einem Wert.
const TEXT_STREAM: &str = r#"system:
    language = 1
    tick     = 10 ms

input  rx   : stream<line<24>> @ hw("u/rx") with max_rate = 300 Hz, capacity = 4
stream<line<24>> echo with capacity = 4

output code  : int in -999999..999999 @ sim("code")
output words : int in 0..999          @ sim("words")
output lens  : int in 0..9999         @ sim("lens")
output keyed : bool                   @ sim("keyed")
output tail  : int in 0..99           @ sim("tail")
output cut   : int in 0..999          @ sim("cut")
output heard : int in 0..999          @ sim("heard")
output hexed : int in 0..99999999     @ sim("hexed")
output odd   : int in 0..999          @ sim("odd")

machine reader:
    var w   : int in 0..999 = 0
    var c   : int in 0..999 = 0
    var o   : int in 0..999 = 0
    var key : str<64> = ""

    initial RUN

    state RUN:
        loop:
            keyed = key == "alpha"
            if key matches "al{rest:str<8>}" as m:
                tail = m.rest.len

        on rx matches "code {n:int}" as e:
            code = min(max(e.n, -999999), 999999)
            send echo, "c{e.n}"

        on rx matches "hex {h:hex}" as e:
            hexed = min(e.h, 99999999)

        on rx has "key={k:word};" as e:
            key = e.k
            w = (w + 1) % 1000
            words = w

        on rx has "<{s:str<4>}>" as e:
            lens = e.s.len + 100 * e.text.len

        on rx as e when e.text.truncated:
            c = (c + 1) % 1000
            cut = c

        on rx as e when e.text.starts_with("x") or e.text.contains("yz"):
            o = (o + 1) % 1000
            odd = o

machine listener:
    var n : int in 0..999 = 0

    initial RUN

    state RUN:
        on echo matches "c{v:int}" as e:
            n = (n + 1) % 1000
            heard = min(max(e.v, 0), 999)
"#;

const TEXT_STREAM_STIMULUS: &str = "t=1 in rx \"code 42\"\nt=1 in rx \"code -7\"\nt=2 in rx \"code 9223372036854775807\"\nt=2 in rx \"code 9223372036854775808\"\nt=3 in rx \"hex 0xff\"\nt=3 in rx \"hex 1A\"\nt=4 in rx \"a key=alpha; b\"\nt=4 in rx \"key=beta;\"\nt=5 in rx \"<äö>\"\nt=5 in rx \"<äöü>\"\nt=5 in rx \"zz<ab>\"\nt=6 in rx \"this line is much longer than twenty-four bytes\"\nt=7 in rx \"xenon\"\nt=7 in rx \"abyzc\"\nt=8 in rx \"key=alphabet;\"\nt=9 in rx \"code +15\"\nt=9 in rx \"code 12x\"\nt=10 in rx \"hex 0x\"\nt=11 in rx \"key=alpha;\"\n";

#[test]
fn a_text_stream_agrees() {
    agree_program("TEXT_STREAM", &compile("TEXT_STREAM", TEXT_STREAM), TEXT_STREAM_STIMULUS, 14);
}

/// Sendepuffer (8.8): `overflow = drop` verwirft, was nicht passt, ein
/// zu grosser `send` faultet den Schreiber, der Puffer leert sich danach
/// weiter; ein `sim`-gespeister Bytestrom liest, was der Treiber abholt.
const TX_STREAMS: &str = r#"system:
    language = 1
    tick     = 10 ms

input  frames     : stream<bytes<4>> @ hw("bus/frames") with capacity = 4, max_rate = 100 Hz
output frames_sim : stream<bytes<4>> @ sim("bus/frames") with max_rate = 400 Hz, capacity = 8
output tx         : stream<u8>       @ hw("u/tx")       with max_rate = 100 Hz, capacity = 4
output lossy      : stream<u8>       @ hw("u/lossy")    with max_rate = 100 Hz, capacity = 3, overflow = drop

output seen : int in 0..999 @ sim("seen")
output size : int in 0..99  @ sim("size")
output head : int in 0..255 @ sim("head")
output room : int in 0..9   @ sim("room")

machine pump:
    var k : int in 0..255 = 0
    var b : bytes<4> = default

    initial RUN

    state RUN:
        loop:
            k = (k + 1) % 200
            b.clear()
            b.push(k as u8)
            if k % 2 == 0:
                b.push(0x7F)
            send frames_sim, b
            send lossy, [1, 2]
            room = lossy.free
        after 50 ms: -> HOT

    state HOT:
        loop:
            send tx, [1, 2, 3]

machine reader:
    var n : int in 0..999 = 0

    initial RUN

    state RUN:
        on frames as f:
            n = (n + 1) % 1000
            seen = n
            size = f.data.len
            head = f.data[0] as int
"#;

#[test]
fn send_buffers_agree() {
    agree_program("TX_STREAMS", &compile("TX_STREAMS", TX_STREAMS), "", 12);
}

/// Ganzzahl-Primitive in ihrer Breite (wrapping, saturating auch ueber
/// `i64` hinaus, Rotation mit beliebigem Betrag), Bits, Rundung auf eine
/// ganze Zahl und eine affine Einheit.
const PRIMITIVES: &str = r#"system:
    language = 1
    tick     = 10 ms

input  k : int in -300..300 @ hw("i/k")
input  f : float in -1000.0..1000.0 @ hw("i/f")
input  t : float[degC] in -50..100 degC @ hw("i/t")

output w8   : i8         @ sim("w8")
output s8   : u8         @ sim("s8")
output s64  : int        @ sim("s64")
output rl   : u8         @ sim("rl")
output rr   : i16        @ sim("rr")
output bit  : bool       @ sim("bit")
output bits : int        @ sim("bits")
output wb   : u8         @ sim("wb")
output r    : int        @ sim("r")
output fl   : int        @ sim("fl")
output ce   : int        @ sim("ce")
output kelvin : float[K] @ sim("kelvin")

machine m:
    initial RUN

    state RUN:
        loop:
            var a : i8 = ((k % 100) as i8)
            var b : u8 = (((k + 300) % 256) as u8)
            w8 = wrapping_add(a, 100 as i8)
            s8 = saturating_add(b, 200 as u8)
            s64 = saturating_sub(k * 30000000000000000, 9000000000000000000)
            rl = rotl(b, (k + 300) % 13)
            rr = rotr(((k * 97) as i16), k + 303)
            bit = b.bit((k + 300) % 8)
            bits = (k * 1000003).bits(20, 4)
            wb = b.with_bit(3, k % 2 == 0)
            r = round(f / 7.0)
            fl = floor(f / 3.0)
            ce = ceil(f * 1.5)
            kelvin = t.to(K)
"#;

#[test]
fn primitives_agree() {
    let stim: String = (0..=40)
        .map(|j| {
            let k = (j * 37) % 601 - 300;
            let f = f64::from((j * 91) % 2001 - 1000) / 3.0;
            format!("t={j} in k {k}\nt={j} in f {f:?}\nt={j} in t {} degC\n", (j * 7) % 150 - 50)
        })
        .collect();
    agree_program("PRIMITIVES", &compile("PRIMITIVES", PRIMITIVES), &stim, 40);
}

/// Ergebnisse `T!E` (3.8): `OK`, `ERR`, `.ok`, `.err`, `.or`, `case` ueber
/// beide Varianten, der Standardwert `OK(…)` und ein Auspacken eines
/// Fehlers, das faultet.
const RESULTS: &str = r#"system:
    language = 1
    tick     = 10 ms

enum Why:
    SMALL
    BIG

input  k : int in -50..50 @ hw("i/k")

output okay  : bool          @ sim("okay")
output value : int           @ sim("value")
output why   : int in 0..9   @ sim("why")
output got   : int           @ sim("got")
output last  : int           @ sim("last")

fn classify(x: int) -> int!Why:
    if x < -20:
        return ERR(SMALL)
    if x > 20:
        return ERR(BIG)
    return OK(x * 2)

machine m:
    var r : int!Why = default
    var n : int = 0

    initial RUN

    state RUN:
        loop:
            r = classify(k)
            okay = r.ok
            value = r.or(-1)
            why = 1 if r.err.or(BIG) == SMALL else (2 if r.err.valid else 0)
            match r:
                case OK(v):
                    got = v
                case ERR(e):
                    got = 100 if e == SMALL else 200
            n = n + 1
            if n == 30:
                last = r
"#;

#[test]
fn results_agree() {
    let stim: String = (0..=40).map(|j| format!("t={j} in k {}\n", (j * 17) % 101 - 50)).collect();
    agree_program("RESULTS", &compile("RESULTS", RESULTS), &stim, 40);
}

/// Das Drahtformat (3.7): `decode` mit Konstante, Range, Diskriminante,
/// Laengenfeld und `align`, verschachtelt in anderer Byte-Reihenfolge;
/// `encode` des dekodierten und eines geaenderten Records; Ausschnitte
/// mit berechneten Grenzen, einer davon faultet.
const WIRE: &str = r#"system:
    language = 1
    tick     = 10 ms

enum Kind layout u8: PING = 0x01, DATA = 0x02, ACK = 0x7F
enum Bad: BROKEN

record Inner layout big:
    a : i16
    b : bool

record Frame layout little, align = 4:
    magic : u16 = 0xA55A
    kind  : Kind
    level : u8 in 0..100
    temp  : i16
    inner : Inner
    pair  : [2] u8
    n     : u8
    data  : bytes<6> with len = n
    tail  : u16

input  rx : stream<bytes<24>> @ hw("bus/rx") with max_rate = 100 Hz, capacity = 2

output ok    : bool                 @ sim("ok")
output kind  : int in 0..9          @ sim("kind")
output temp  : int in -40000..40000 @ sim("temp")
output inner : int in -40000..40000 @ sim("inner")
output tail  : int in 0..70000      @ sim("tail")
output sum   : int in 0..99999      @ sim("sum")
output size  : int in 0..99         @ sim("size")
output echo  : int in 0..999999     @ sim("echo")
output moved : int in 0..999999     @ sim("moved")
output part  : int in 0..99         @ sim("part")
output tip   : int in 0..999        @ sim("tip")

fn unpack(b: bytes<24>) -> Frame!Bad:
    var d = Frame.decode(b)
    if not d.valid:
        return ERR(BROKEN)
    var h = d
    return OK(h)

fn weigh(e: bytes<20>) -> int in 0..999999:
    var acc : int in 0..999999 = 0
    for i in range(20):
        if i >= e.len:
            break
        acc = (acc + (e[i] as int) * (i + 1)) % 100000
    return acc

machine m:
    fault -> RECOVER
    var count : int in 0..9999 = 0

    initial RUN

    state RUN:
        on rx as f:
            count = (count + 1) % 10000
            match unpack(f.data):
                case OK(fr):
                    ok = true
                    kind = 1 if fr.kind == PING else (2 if fr.kind == DATA else 3)
                    temp = fr.temp as int
                    inner = (fr.inner.a as int) + (1000 if fr.inner.b else 0)
                    tail = fr.tail as int
                    sum = fr.data.len * 1000 + (fr.n as int)
                    var e = fr.encode()
                    size = e.len
                    echo = weigh(e)
                    var g = fr
                    g.data.clear()
                    g.tail = 0xBEEF
                    moved = weigh(g.encode())
                case ERR(x):
                    ok = false
            var s = f.data[2..f.data.len]
            part = s.len
            var t = f.data[1..(f.data[0] as int) % 8]
            tip = t.len * 100 + ((s[0] as int) if s.len > 0 else 0)

    state RECOVER:
        enter:
            ok = false
        after 20 ms: -> RUN
"#;

#[test]
fn the_wire_format_agrees() {
    let frames = [
        // Gueltig, drei Nutzbytes, `align` fuellt auf 20 Byte.
        "5aa5023200ff38ff010708 03 aabbcc 3412 000000",
        // Zu kurz: das Auffuellen fehlt.
        "5aa5023200ff38ff010708 03 aabbcc 3412",
        // Falsche Konstante.
        "5aa4023200ff38ff010708 03 aabbcc 3412 000000",
        // Unbekannte Diskriminante.
        "5aa5053200ff38ff010708 03 aabbcc 3412 000000",
        // Ausserhalb der Range.
        "5aa5026500ff38ff010708 03 aabbcc 3412 000000",
        // Laenge ueber der Obergrenze.
        "5aa5023200ff38ff010708 07 aabbccddeeff11 3412 00",
        // Keine Nutzbytes: 14 Byte, aufgefuellt auf 16.
        "5aa57f0a0080000001ff00 00 cdab 0000",
        // Volle Nutzlast.
        "5aa5010000010000000102 06 010203040506 ffff",
        // Der Ausschnitt `[1..]` reicht hinter das Ende und faultet.
        "07aa",
        "",
    ];
    let mut stim = String::new();
    for (k, f) in frames.iter().enumerate() {
        stim.push_str(&format!("t={} in rx 0x{}\n", 3 * k + 1, f.replace(' ', "")));
    }
    agree_program("WIRE", &compile("WIRE", WIRE), &stim, 40);
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
