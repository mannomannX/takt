//! `persist` im erzeugten Code (5.9, Satz 9.4.4).
//!
//! Zwei Richtungen: `_persist_snapshot` liefert dieselben Bytes wie der
//! Interpreter, und `_persist_restore` fuehrt eine Journal-Nutzlast zu
//! denselben Outputs. Beides ueber `35_persist.takt`, das drei Typen
//! persistiert: Skalar, Record, Array.

mod common;

use takt_conformance::run::compare;
use takt_interp::nvm::Nvm;
use takt_interp::{RunOptions, Trace, Value, run};
use takt_mir::Program;

const TICKS: u64 = 40;

fn program() -> Program {
    corpus("35_persist.takt")
}

fn corpus(name: &str) -> Program {
    let path = format!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try/{}"), name);
    let src = std::fs::read_to_string(&path).expect("lesbar");
    let options = takt_sema::Options {
        policy: takt_diag::Policy::default(),
        build: takt_sema::Build::Sim,
        profile: None,
        ..Default::default()
    };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    out.program.expect("Programm")
}

/// Die `persist`-Zeile eines Traces, ohne Tick.
fn persist_line(trace: &str) -> Option<String> {
    trace.lines().find_map(|l| l.split_once(" persist ").map(|(_, hex)| hex.trim().to_string()))
}

fn interpreted(p: &Program, nvm: Nvm) -> String {
    run(p, &Trace::default(), &RunOptions { ticks: TICKS, nvm, ..Default::default() }).expect("Lauf").trace.render()
}

/// Eine Nutzlast mit Werten, die keinem Default gleichen.
fn foreign_payload(p: &Program) -> Vec<u8> {
    let m = p.machines.iter().find(|m| !m.persist.is_empty()).expect("persist");
    let by_name = |name: &str| {
        let pv = m.persist.iter().find(|pv| m.vars[pv.var.index()].name == name).expect(name);
        (pv.type_hash, m.vars[pv.var.index()].ty)
    };
    let (h_cycles, t_cycles) = by_name("cycles");
    let (h_health, t_health) = by_name("health");
    let (h_trials, t_trials) = by_name("trials");
    let cycles = Value::Int(500);
    let health = Value::Record(vec![Value::Bool(false), Value::Int(7)]);
    let trials = Value::Array(vec![Value::Int(4), Value::Int(5)]);
    Nvm::payload(p, &[(h_cycles, &cycles, t_cycles), (h_health, &health, t_health), (h_trials, &trials, t_trials)])
        .expect("kodierbar")
}

#[test]
fn the_snapshot_matches_the_interpreter_byte_for_byte() {
    let Some(clang) = common::clang() else { return };
    let p = program();
    let native =
        common::run_native_persist(&clang, &p, "35_persist_snapshot", TICKS, &[]).unwrap_or_else(|e| panic!("{e}"));
    let interp = interpreted(&p, Nvm::new());
    let want = persist_line(&interp).expect("Interpreter schreibt keine persist-Zeile");
    let got = persist_line(&native).expect("Rahmen schreibt keine persist-Zeile");
    assert_eq!(got, want, "Snapshot weicht ab\n--- Interpreter ---\n{interp}\n--- nativ ---\n{native}");
    assert!(compare(&interp, &native).is_empty());
}

#[test]
fn a_restored_payload_drives_both_sides_to_the_same_outputs() {
    let Some(clang) = common::clang() else { return };
    let p = program();
    let payload = foreign_payload(&p);

    let mut nvm = Nvm::new();
    nvm.from_program_payload(&p, &payload);
    let interp = interpreted(&p, nvm);
    assert!(interp.contains("out count 500"), "Interpreter hat nicht geladen:\n{interp}");

    let native =
        common::run_native_persist(&clang, &p, "35_persist_restore", TICKS, &payload).unwrap_or_else(|e| panic!("{e}"));
    let diffs = compare(&interp, &native);
    assert!(
        diffs.is_empty(),
        "{} Abweichungen: {diffs:?}\n--- Interpreter ---\n{interp}\n--- nativ ---\n{native}",
        diffs.len()
    );
    assert_eq!(persist_line(&native), persist_line(&interp), "Snapshot nach dem Laden weicht ab");
}

#[test]
fn a_payload_with_an_out_of_range_value_is_rejected_on_both_sides() {
    // 5.9: ungueltige Werte ergeben den Default. Der erzeugte Code prueft
    // die Range ebenso wie der Interpreter, sonst driftete s0 (9.4.4).
    let Some(clang) = common::clang() else { return };
    let p = program();
    let m = p.machines.iter().find(|m| !m.persist.is_empty()).expect("persist");
    let pv = m.persist.iter().find(|pv| m.vars[pv.var.index()].name == "cycles").expect("cycles");
    let ty = m.vars[pv.var.index()].ty;
    // `cycles : int in 0..1000` — 5000 liegt ausserhalb.
    let payload = Nvm::payload(&p, &[(pv.type_hash, &Value::Int(5000), ty)]).expect("kodierbar");

    let mut nvm = Nvm::new();
    nvm.from_program_payload(&p, &payload);
    let interp = interpreted(&p, nvm);
    assert!(interp.contains("out count 3"), "Interpreter nahm den Default nicht:\n{interp}");

    let native =
        common::run_native_persist(&clang, &p, "35_persist_range", TICKS, &payload).unwrap_or_else(|e| panic!("{e}"));
    let diffs = compare(&interp, &native);
    assert!(diffs.is_empty(), "{} Abweichungen: {diffs:?}\n--- nativ ---\n{native}", diffs.len());
}

/// 11.2: Ein Enum mit Feldern liegt im Journal als Diskriminante und
/// Felder in kanonischer Form, dahinter Nullen — bytegleich auf beiden
/// Seiten.
#[test]
fn a_variant_with_fields_is_persisted_byte_for_byte() {
    let Some(clang) = common::clang() else { return };
    let p = corpus("81_persist_variants.takt");
    let native = common::run_native_persist(&clang, &p, "81_snapshot", TICKS, &[]).unwrap_or_else(|e| panic!("{e}"));
    let interp = interpreted(&p, Nvm::new());
    let want = persist_line(&interp).expect("Interpreter schreibt keine persist-Zeile");
    let got = persist_line(&native).expect("Rahmen schreibt keine persist-Zeile");
    assert_eq!(got, want, "Snapshot weicht ab\n--- Interpreter ---\n{interp}\n--- nativ ---\n{native}");
    assert!(compare(&interp, &native).is_empty());
}

/// Ein geladener Stand mit `MOVE(-7, 3)` treibt beide Seiten gleich, und
/// der Schnappschuss danach ist wieder bytegleich.
#[test]
fn a_restored_variant_with_fields_drives_both_sides_the_same() {
    let Some(clang) = common::clang() else { return };
    let p = corpus("81_persist_variants.takt");
    let m = p.machines.iter().find(|m| !m.persist.is_empty()).expect("persist");
    let by_name = |name: &str| {
        let pv = m.persist.iter().find(|pv| m.vars[pv.var.index()].name == name).expect(name);
        (pv.type_hash, m.vars[pv.var.index()].ty)
    };
    let (h_last, t_last) = by_name("last");
    let (h_count, t_count) = by_name("count");
    let last = Value::Enum { variant: 1, fields: vec![Value::Int(-7), Value::Int(3)] };
    let count = Value::Int(500);
    let payload = Nvm::payload(&p, &[(h_last, &last, t_last), (h_count, &count, t_count)]).expect("kodierbar");

    let mut nvm = Nvm::new();
    nvm.from_program_payload(&p, &payload);
    let interp = interpreted(&p, nvm);
    assert!(interp.contains("out cmd MOVE(-7, 3)") && interp.contains("out seen 500"), "nicht geladen:\n{interp}");

    let native =
        common::run_native_persist(&clang, &p, "81_restore", TICKS, &payload).unwrap_or_else(|e| panic!("{e}"));
    let diffs = compare(&interp, &native);
    assert!(diffs.is_empty(), "{} Abweichungen: {diffs:?}\n--- nativ ---\n{native}", diffs.len());
    assert_eq!(persist_line(&native), persist_line(&interp), "Snapshot nach dem Laden weicht ab");
}

/// Die Gestalten aus `takt-sema/tests/canonical_shape.rs` ohne `vec`: Enum
/// mit Feldern, verschachtelte Records, Arrays, `str`, `bytes` und `map`.
/// Das Programm liest seine `persist`-Variablen nie und schreibt sie nie,
/// also ist der Schnappschuss am Ende genau der geladene Stand.
const SHAPES: &str = "system:
    language = 1
    tick = 1 ms

enum Mode: OFF, ON(level: u8), FAULT(code: u16, hold: bool)

record Inner:
    good : bool
    hold : Duration
    gain : f32

record Big:
    mode  : Mode
    xs    : [3] i16
    name  : str<5>
    data  : bytes<4>
    inner : Inner
    scale : f64

output n : int in 0..9 @ hw(\"o/n\") with safe = 0

machine m:
    persist var big   : Big = default
    persist var table : map<u8, i32, 3> = default
    persist var modes : [2] Mode = default
    initial RUN
    state RUN:
        loop:
            n = 1
";

/// `vec<T, N>` als Variable und als Feld, mit Elementen fester und mit
/// Enum-Gestalt.
const VECTORS: &str = "system:
    language = 1
    tick = 1 ms

enum Mode: OFF, ON(level: u8), FAULT(code: u16, hold: bool)

record Tail:
    small : vec<u8, 3>
    gain  : f32

output n : int in 0..9 @ hw(\"o/n\") with safe = 0

machine m:
    persist var small : vec<i16, 3> = default
    persist var modes : vec<Mode, 2> = default
    persist var tail  : Tail = default
    initial RUN
    state RUN:
        loop:
            n = 1
";

/// xorshift64*, deterministisch: Ein Fehlschlag ist mit seinem Startwert
/// reproduzierbar.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }

    /// Ein endliches Bitmuster der Breite `bits`, oft ein Rand: Nullen beider
    /// Vorzeichen und der kleinste Subnormale.
    fn finite(&mut self, bits: u32) -> u64 {
        let sign = 1u64 << (bits - 1);
        let exp = if bits == 32 { 0x7f80_0000 } else { 0x7ff0_0000_0000_0000 };
        match self.below(4) {
            0 => [0, sign, 1, sign | 1][self.below(4) as usize],
            _ => loop {
                let b = self.next() >> (64 - bits);
                if b & exp != exp {
                    break b;
                }
            },
        }
    }
}

/// Ein zufaelliger gueltiger Wert des Typs `ty` (3.7, 3.9); eine `map`
/// fuellt der Interpreter selbst, damit die Slots in seiner Sondierordnung
/// liegen.
fn random_value(p: &Program, ty: takt_mir::TypeId, rng: &mut Rng) -> Value {
    use takt_mir::types::{FloatWidth, Type};
    match &p.types.list[ty.index()] {
        Type::Bool => Value::Bool(rng.next() & 1 == 1),
        Type::Int { width, range: None, .. } if width.signed() => {
            Value::Int((rng.next() as i64) >> (64 - width.bits()))
        }
        Type::Int { width, range: None, .. } => Value::UInt(rng.next() >> (64 - width.bits())),
        Type::Float { width: FloatWidth::F32, range: None, .. } => Value::F32(f32::from_bits(rng.finite(32) as u32)),
        Type::Float { width: FloatWidth::F64, range: None, .. } => Value::F64(f64::from_bits(rng.finite(64))),
        Type::Duration { range: None } => Value::Duration((rng.next() >> 20) as i64),
        Type::Enum(id) => {
            let variants = &p.enums[id.index()].variants;
            let variant = rng.below(variants.len() as u64) as usize;
            let fields = variants[variant].fields.iter().map(|f| random_value(p, f.ty, rng)).collect();
            Value::Enum { variant: variant as u32, fields }
        }
        Type::Record(id) => {
            Value::Record(p.records[id.index()].fields.iter().map(|f| random_value(p, f.ty, rng)).collect())
        }
        Type::Array { elem, len } => Value::Array((0..*len).map(|_| random_value(p, *elem, rng)).collect()),
        Type::Vec { elem, cap } => {
            let n = rng.below(u64::from(*cap) + 1);
            Value::Vec((0..n).map(|_| random_value(p, *elem, rng)).collect())
        }
        Type::Bytes { cap } => Value::Bytes((0..rng.below(u64::from(*cap) + 1)).map(|_| rng.next() as u8).collect()),
        Type::Str { cap } => {
            // Ein-, Zwei- und Dreibyte-Zeichen bis an die Schranke in Byte.
            let mut text = String::new();
            loop {
                let c = ['a', 'Z', '7', '\u{e4}', '\u{20ac}'][rng.below(5) as usize];
                if text.len() + c.len_utf8() > *cap as usize || rng.below(6) == 0 {
                    break Value::Str(text);
                }
                text.push(c);
            }
        }
        Type::Map { key, value, cap } => {
            let mut slots = vec![None; *cap as usize];
            for _ in 0..rng.below(u64::from(*cap) + 1) {
                let (k, v) = (random_value(p, *key, rng), random_value(p, *value, rng));
                takt_interp::maps::insert(p, *key, &mut slots, k, v).expect("Schluessel kodierbar");
            }
            Value::Map(slots)
        }
        other => panic!("kein Zufallswert fuer {other:?}"),
    }
}

/// Zufaellige gueltige Werte je Startwert kodiert der Interpreter
/// (`Nvm::payload`); der erzeugte Code liest sie mit seinem Leser, und sein
/// Schnappschuss am Ende — sein Kodierer — liefert dieselben Bytes, ebenso
/// der des Interpreters (5.9, Satz 9.4.4).
fn encoded_alike(src: &str, name: &str) {
    let Some(clang) = common::clang() else { return };
    let options = takt_sema::Options { build: takt_sema::Build::Sim, ..Default::default() };
    let out = takt_sema::compile(src, &options);
    let p = out.program.unwrap_or_else(|| panic!("{:?}", out.diagnostics));
    let m = p.machines.iter().find(|m| !m.persist.is_empty()).expect("persist");
    let vars: Vec<(u64, takt_mir::TypeId)> =
        m.persist.iter().map(|pv| (pv.type_hash, m.vars[pv.var.index()].ty)).collect();
    for seed in 1..=6u64 {
        let mut rng = Rng(0x5E31_0300 + seed);
        let values: Vec<Value> = vars.iter().map(|(_, ty)| random_value(&p, *ty, &mut rng)).collect();
        let entries: Vec<(u64, &Value, takt_mir::TypeId)> =
            vars.iter().zip(&values).map(|((hash, ty), v)| (*hash, v, *ty)).collect();
        let payload = Nvm::payload(&p, &entries).expect("kodierbar");
        let hex: String = payload.iter().map(|b| format!("{b:02x}")).collect();

        let mut nvm = Nvm::new();
        nvm.from_program_payload(&p, &payload);
        let interp = interpreted(&p, nvm);
        assert_eq!(persist_line(&interp).as_deref(), Some(hex.as_str()), "Startwert {seed}: {values:?}\n{interp}");
        let native = common::run_native_persist(&clang, &p, &format!("{name}_{seed}"), TICKS, &payload)
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(persist_line(&native).as_deref(), Some(hex.as_str()), "Startwert {seed}: {values:?}\n{native}");
        let diffs = compare(&interp, &native);
        assert!(diffs.is_empty(), "Startwert {seed}: {diffs:?}");
    }
}

/// **Zusammengesetzte Typen in kanonischer Byteform, bitgleich** (5.9,
/// Satz 9.4.4; SEM1-030): Records, Enums mit Feldern, Arrays, Text, Bytes
/// und `map` ([`SHAPES`]).
#[test]
fn random_composite_values_are_encoded_alike() {
    encoded_alike(SHAPES, "shapes");
}

/// **`vec<T, N>` ebenso** (3.9, 5.9; SEM1-030): Laenge und Elemente, als
/// Variable und als Feld ([`VECTORS`]).
#[test]
fn random_vectors_are_encoded_alike() {
    encoded_alike(VECTORS, "vectors");
}

/// Was ein Neustart aus dem Journal laedt: die Nutzlast des gueltigen
/// Eintrags mit der hoechsten Sequenz, oder nichts.
fn reload(nvm: takt_rt_core::FakeNvm<JOURNAL_SLOT>) -> Option<Vec<u8>> {
    let mut into = [0u8; JOURNAL_SLOT];
    match takt_rt_core::Journal::new(nvm, JOURNAL_HASH, 0).load(&mut into) {
        takt_rt_core::Loaded::Found { length, .. } => Some(into[..length as usize].to_vec()),
        takt_rt_core::Loaded::Empty => None,
    }
}

/// Ein Slot des Journals, gross genug fuer die Nutzlast von `35_persist`.
const JOURNAL_SLOT: usize = 512;

/// Der Logik-Hash, unter dem der Test schreibt und liest.
const JOURNAL_HASH: u64 = 0x35;

/// Ein Journal mit dem Stand `old`, dann ein Schreibvorgang mit `new`, dem
/// nach `cut` Bytes der Strom ausfaellt (8.11); `seed` macht den Rest des
/// abgebrochenen Vorgangs unbestimmt. Liefert, was der Neustart laedt, und
/// ob der Vorgang fertig wurde.
fn journal_after_cut(old: &[u8], new: &[u8], cut: Option<u32>, seed: Option<u32>) -> (Option<Vec<u8>>, bool) {
    let mut journal = takt_rt_core::Journal::new(takt_rt_core::FakeNvm::<JOURNAL_SLOT>::new(), JOURNAL_HASH, 0);
    let mut into = [0u8; JOURNAL_SLOT];
    assert_eq!(journal.load(&mut into), takt_rt_core::Loaded::Empty);
    let (mut stored, mut stored_len) = ([0u8; JOURNAL_SLOT], 0usize);
    assert!(journal.flush(old, &mut stored, &mut stored_len, || true), "der alte Stand steht");
    match (cut, seed) {
        (Some(n), Some(seed)) => journal.device_mut().cut_at_seeded(n, seed),
        (Some(n), None) => journal.device_mut().cut_at(n),
        (None, _) => {}
    }
    let mut polls = 0;
    let done = journal.flush(new, &mut stored, &mut stored_len, || {
        polls += 1;
        polls < 10_000
    });
    let mut nvm = journal.into_inner();
    nvm.power_on();
    (reload(nvm), done)
}

/// **Ein Stromausfall im Journal fuehrt beide Seiten zum selben Stand**
/// (5.9, 8.11, Satz 9.4.4; KON2-022): Das Journal von `takt-rt-core` haelt
/// einen Stand und schreibt einen zweiten; der Strom faellt nach jedem
/// moeglichen Byte aus, einmal sauber, einmal mit unbestimmtem Rest. Ein
/// Neustart laedt genau den alten oder den neuen Stand — der neue gewinnt
/// mit seiner hoeheren Sequenz, sobald sein Kopf steht —, und diese Bytes
/// fuehren Interpreter und erzeugten Restore zu denselben Outputs.
#[test]
fn a_power_cut_in_the_journal_restores_alike_on_both_sides() {
    let Some(clang) = common::clang() else { return };
    let p = program();
    let old = foreign_payload(&p);
    let new = {
        let m = p.machines.iter().find(|m| !m.persist.is_empty()).expect("persist");
        let entries: Vec<(u64, Value, takt_mir::TypeId)> = m
            .persist
            .iter()
            .map(|pv| {
                let var = &m.vars[pv.var.index()];
                let value = match var.name.as_str() {
                    "cycles" => Value::Int(700),
                    "health" => Value::Record(vec![Value::Bool(true), Value::Int(3)]),
                    _ => Value::Array(vec![Value::Int(9), Value::Int(1)]),
                };
                (pv.type_hash, value, var.ty)
            })
            .collect();
        let refs: Vec<(u64, &Value, takt_mir::TypeId)> = entries.iter().map(|(h, v, t)| (*h, v, *t)).collect();
        Nvm::payload(&p, &refs).expect("kodierbar")
    };
    assert_ne!(old, new);

    // Wie viele Bytes der zweite Vorgang schreibt: ein Lauf ohne Ausfall.
    let (whole, done) = journal_after_cut(&old, &new, None, None);
    assert!(done && whole.as_deref() == Some(new.as_slice()), "ohne Ausfall gilt der neue Stand");
    let mut probe = takt_rt_core::Journal::new(takt_rt_core::FakeNvm::<JOURNAL_SLOT>::new(), JOURNAL_HASH, 0);
    probe.load(&mut [0u8; JOURNAL_SLOT]);
    let (mut stored, mut stored_len) = ([0u8; JOURNAL_SLOT], 0usize);
    assert!(probe.flush(&old, &mut stored, &mut stored_len, || true));
    let before = probe.device().bytes_written();
    assert!(probe.flush(&new, &mut stored, &mut stored_len, || true));
    let bytes = probe.device().bytes_written() - before;

    let mut seen = std::collections::BTreeMap::new();
    for cut in 0..=bytes {
        for seed in [None, Some(cut.wrapping_mul(2_654_435_761))] {
            let (loaded, _) = journal_after_cut(&old, &new, Some(cut), seed);
            let loaded = loaded.unwrap_or_else(|| panic!("Schnitt nach {cut} Byte ({seed:?}): nichts geladen"));
            assert!(loaded == old || loaded == new, "Schnitt nach {cut} Byte ({seed:?}): weder alt noch neu");
            seen.entry(loaded).or_insert(cut);
        }
    }
    assert!(seen.contains_key(&old) && seen.contains_key(&new), "der Schnitt traf nie beide Staende");

    for (payload, cut) in seen {
        let mut nvm = Nvm::new();
        nvm.from_program_payload(&p, &payload);
        let interp = interpreted(&p, nvm);
        let want = if payload == old { "out count 500" } else { "out count 700" };
        assert!(interp.contains(want), "Schnitt nach {cut} Byte: `{want}` fehlt:\n{interp}");
        let native = common::run_native_persist(&clang, &p, &format!("35_cut_{cut}"), TICKS, &payload)
            .unwrap_or_else(|e| panic!("{e}"));
        let diffs = compare(&interp, &native);
        assert!(diffs.is_empty(), "Schnitt nach {cut} Byte: {diffs:?}\n--- nativ ---\n{native}");
    }
}
