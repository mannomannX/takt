//! `persist var` (Referenz 5.9, 9.10): der Anfangszustand s0 traegt die
//! geladenen Werte; ungueltige ergeben den Default plus `PersistReset`.
//!
//! Das Schreiben ist nicht Teil der Semantik (5.9: „Beobachtung"), also
//! prueft hier nichts das Journal — nur, was s0 vorfindet.

use takt_diag::Policy;
use takt_interp::nvm::Nvm;
use takt_interp::{RunOptions, Trace, Value, run};
use takt_mir::Program;
use takt_sema::{Build, Options};

const HEAD: &str = "system:\n    language = 1\n    tick = 1 ms\n\n";

fn compile(body: &str) -> Program {
    let src = format!("{HEAD}{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "unerwartete Fehler:\n{}", errors.join("\n"));
    out.program.expect("Programm")
}

/// Der Typ-Hash der einzigen `persist`-Variablen.
fn only_hash(p: &Program) -> u64 {
    let m = p.machines.iter().find(|m| !m.persist.is_empty()).expect("Maschine mit persist");
    m.persist[0].type_hash
}

fn simulate(p: &Program, nvm: Nvm, ticks: u64) -> String {
    let out = run(p, &Trace::default(), &RunOptions { ticks, nvm, ..Default::default() }).expect("Lauf");
    out.trace.render()
}

/// Ein Programm, das seine `persist`-Variable auf einen Output legt.
const COUNTER: &str = "
output n : int in 0..99 @ hw(\"o/n\") with safe = 0

machine m:
    persist var c : int in 0..99 = 7
    initial RUN
    state RUN:
        loop:
            n = c
";

#[test]
fn an_empty_store_keeps_the_default() {
    // 5.9: Der erste Start eines Geraets ist kein Fehler.
    let p = compile(COUNTER);
    let trace = simulate(&p, Nvm::new(), 2);
    assert!(trace.contains("out n 7"), "Default erwartet:\n{trace}");
    assert!(!trace.contains("PersistReset"), "kein Alert erwartet:\n{trace}");
}

#[test]
fn a_stored_value_overrides_the_default() {
    // 5.9: „s0 enthaelt die aus dem nichtfluechtigen Speicher geladenen Werte".
    let p = compile(COUNTER);
    let mut nvm = Nvm::new();
    nvm.put(only_hash(&p), Value::Int(42));
    let trace = simulate(&p, nvm, 2);
    assert!(trace.contains("out n 42"), "geladener Wert erwartet:\n{trace}");
    assert!(!trace.contains("PersistReset"), "kein Alert erwartet:\n{trace}");
}

#[test]
fn a_value_outside_the_range_falls_back_with_an_alert() {
    // 5.9: „Fehlende oder ungueltige Werte (Typ-Hash, Range) ergeben den
    // Default plus Alert `PersistReset`."
    let p = compile(COUNTER);
    let mut nvm = Nvm::new();
    nvm.put(only_hash(&p), Value::Int(500));
    let trace = simulate(&p, nvm, 2);
    assert!(trace.contains("out n 7"), "Default erwartet:\n{trace}");
    assert!(trace.contains("PersistReset"), "Alert erwartet:\n{trace}");
}

#[test]
fn a_value_of_the_wrong_shape_falls_back_with_an_alert() {
    let p = compile(COUNTER);
    let mut nvm = Nvm::new();
    nvm.put(only_hash(&p), Value::Bool(true));
    let trace = simulate(&p, nvm, 2);
    assert!(trace.contains("out n 7"), "Default erwartet:\n{trace}");
    assert!(trace.contains("PersistReset"), "Alert erwartet:\n{trace}");
}

#[test]
fn an_entry_under_a_foreign_hash_is_not_found() {
    // Der Hash ist der Schluessel: ein Eintrag aus einer anderen Firmware
    // wird nicht gelesen, und das ist kein Fehler, sondern ein erster Start.
    let p = compile(COUNTER);
    let mut nvm = Nvm::new();
    nvm.put(only_hash(&p) ^ 1, Value::Int(42));
    let trace = simulate(&p, nvm, 2);
    assert!(trace.contains("out n 7"), "Default erwartet:\n{trace}");
    assert!(!trace.contains("PersistReset"), "kein Alert erwartet:\n{trace}");
}

#[test]
fn the_hash_changes_with_the_range() {
    // Eine engere Range bedeutet etwas anderes ueber dasselbe Byte (3.4).
    let wide = compile(COUNTER);
    let narrow = compile(&COUNTER.replace("c : int in 0..99", "c : int in 0..9"));
    assert_ne!(only_hash(&wide), only_hash(&narrow));
}

#[test]
fn the_hash_changes_with_the_variable_name() {
    let a = compile(COUNTER);
    let b = compile(&COUNTER.replace("var c :", "var d :").replace("n = c", "n = d"));
    assert_ne!(only_hash(&a), only_hash(&b));
}

#[test]
fn the_hash_changes_with_the_machine_name() {
    let a = compile(COUNTER);
    let b = compile(&COUNTER.replace("machine m:", "machine other:"));
    assert_ne!(only_hash(&a), only_hash(&b));
}

#[test]
fn the_hash_ignores_the_default_value() {
    // Der Default waehlt nur, was ohne Eintrag gilt; er aendert nicht, was
    // ein gespeichertes Byte bedeutet.
    let a = compile(COUNTER);
    let b = compile(&COUNTER.replace("= 7", "= 8"));
    assert_eq!(only_hash(&a), only_hash(&b));
}

/// Rumpf hinter einer Typdeklaration, mit einer `persist`-Variablen.
fn with_type(decls: &str, var: &str) -> String {
    format!(
        "{decls}
output n : int in 0..9 @ hw(\"o/n\") with safe = 0

machine m:
    {var}
    initial RUN
    state RUN:
        loop:
            n = 1
"
    )
}

#[test]
fn nested_enums_do_not_collide() {
    // Ohne Variantenzahl vor der Liste schluckt eine geschachtelte Variante
    // die Bytes der naechsten Variante des aeusseren Enums: Zwei
    // verschiedene Typen bekaemen denselben Schluessel.
    let a = compile(&with_type("enum Ff: BB\nenum Ee: CC(f: Ff), DD\n", "persist var k : Ee = DD"));
    let b = compile(&with_type("enum Ff: BB, DD\nenum Ee: CC(f: Ff)\n", "persist var k : Ee = CC(f = BB)"));
    assert_ne!(only_hash(&a), only_hash(&b));
}

#[test]
fn the_hash_changes_with_the_enum_layout() {
    // `layout u8` ist die Byte-Breite des gespeicherten Werts (3.7); ein
    // Wechsel auf `u16` liest ein Byte als zwei.
    let a = compile(&with_type("enum Kind layout u8: A = 0, B = 1\n", "persist var k : Kind = A"));
    let b = compile(&with_type("enum Kind layout u16: A = 0, B = 1\n", "persist var k : Kind = A"));
    assert_ne!(only_hash(&a), only_hash(&b));
}

#[test]
fn the_hash_changes_with_the_unit_factor() {
    // Wird `unit spam = 2 m` zu `= 3 m`, bedeutet ein gespeichertes
    // `5 spam` danach 15 m statt 10 m (3.2).
    let a = compile(&with_type("unit spam = 2 m\n", "persist var k : float[spam] = 0 spam"));
    let b = compile(&with_type("unit spam = 3 m\n", "persist var k : float[spam] = 0 spam"));
    assert_ne!(only_hash(&a), only_hash(&b));
}

#[test]
fn a_value_outside_the_width_falls_back_instead_of_faulting() {
    // 5.9: ungueltige Werte ergeben den Default plus Alert. Ein `u8` ohne
    // `in a..b` traegt `range: None` — ohne Breitenpruefung haelt es 9999,
    // und die Maschine faultet im Tick 0, statt den Default zu nehmen.
    let p = compile(
        "
output n : int in 0..99 @ hw(\"o/n\") with safe = 0

machine m:
    persist var c : u8 = 7
    initial RUN
    state RUN:
        loop:
            n = c as int
",
    );
    let mut nvm = Nvm::new();
    nvm.put(only_hash(&p), Value::UInt(9999));
    let trace = simulate(&p, nvm, 2);
    assert!(trace.contains("out n 7"), "Default erwartet:\n{trace}");
    assert!(trace.contains("PersistReset"), "Alert erwartet:\n{trace}");
    assert!(!trace.contains("fault"), "kein Fault erwartet:\n{trace}");
}

#[test]
fn two_instances_of_one_template_get_different_keys() {
    // 5.11: „`persist var` in gescopten Instanzen ist erlaubt (Schluessel
    // enthaelt den Scope-Namen)". Teilten sich zwei Instanzen einen
    // Schluessel, laese jede den Wert der anderen.
    let p = compile(
        "
output a : int in 0..99 @ hw(\"o/a\") with safe = 0
output b : int in 0..99 @ hw(\"o/b\") with safe = 0

machine cell(out: output int in 0..99):
    persist var c : int in 0..99 = 0
    initial RUN
    state RUN:
        loop:
            out = c

instance c1 = cell(out = a)
instance c2 = cell(out = b)
",
    );
    let keys: Vec<u64> = p.machines.iter().flat_map(|m| m.persist.iter().map(|pv| pv.type_hash)).collect();
    assert_eq!(keys.len(), 2, "je Instanz ein Eintrag: {keys:?}");
    assert_ne!(keys[0], keys[1], "Instanzen teilen einen Schluessel");
}

#[test]
fn a_record_field_rename_changes_the_hash() {
    // Feldnamen gehoeren zur Bedeutung: `Result(passed, code)` und
    // `Result(ok, code)` beschreiben dieselben Bytes verschieden.
    const REC: &str = "
record Result:
    passed : bool
    code   : int in 0..255

output n : int in 0..99 @ hw(\"o/n\") with safe = 0

machine m:
    persist var last : Result = Result(passed = false, code = 0)
    initial RUN
    state RUN:
        loop:
            n = last.code
";
    let a = compile(REC);
    let b = compile(&REC.replace("passed", "is_ok"));
    assert_ne!(only_hash(&a), only_hash(&b));
}

#[test]
fn a_record_is_loaded_field_by_field() {
    const REC: &str = "
record Result:
    passed : bool
    code   : int in 0..255

output n : int in 0..99 @ hw(\"o/n\") with safe = 0

machine m:
    persist var last : Result = Result(passed = false, code = 0)
    initial RUN
    state RUN:
        loop:
            n = last.code
";
    let p = compile(REC);
    let mut nvm = Nvm::new();
    nvm.put(only_hash(&p), Value::Record(vec![Value::Bool(true), Value::Int(23)]));
    let trace = simulate(&p, nvm, 2);
    assert!(trace.contains("out n 23"), "geladenes Feld erwartet:\n{trace}");
}

#[test]
fn a_record_with_a_bad_field_is_rejected_as_a_whole() {
    const REC: &str = "
record Result:
    passed : bool
    code   : int in 0..255

output n : int in 0..99 @ hw(\"o/n\") with safe = 0

machine m:
    persist var last : Result = Result(passed = false, code = 9)
    initial RUN
    state RUN:
        loop:
            n = last.code
";
    let p = compile(REC);
    let mut nvm = Nvm::new();
    nvm.put(only_hash(&p), Value::Record(vec![Value::Bool(true), Value::Int(9999)]));
    let trace = simulate(&p, nvm, 2);
    assert!(trace.contains("out n 9"), "Default erwartet:\n{trace}");
    assert!(trace.contains("PersistReset"), "Alert erwartet:\n{trace}");
}

#[test]
fn theorem_9_4_1_holds_with_a_filled_store() {
    // 9.10: „`persist`-Variablen sind gewoehnliche Komponenten von V_m …
    // Damit ist Satz 9.4.1 unveraendert eine Aussage ueber (s0, (I_k))."
    // Geladene Werte gehoeren zu s0, also darf die Schrittfolge den Trace
    // auch mit gefuelltem Speicher nicht aendern.
    let p = compile(
        "
output a : int in 0..99 @ hw(\"o/a\") with safe = 0
output b : int in 0..99 @ hw(\"o/b\") with safe = 0

machine one:
    pub var seen : int in 0..99 = 0
    persist var c : int in 0..99 = 1
    initial RUN
    state RUN:
        loop:
            seen = c
            a = c

machine two:
    persist var d : int in 0..99 = 2
    initial RUN
    state RUN:
        loop:
            b = d + one.seen
",
    );
    let keys: Vec<u64> = p.machines.iter().flat_map(|m| m.persist.iter().map(|pv| pv.type_hash)).collect();
    assert_eq!(keys.len(), 2);
    let mut nvm = Nvm::new();
    nvm.put(keys[0], Value::Int(11));
    nvm.put(keys[1], Value::Int(22));

    let base = run(&p, &Trace::default(), &RunOptions { ticks: 20, nvm: nvm.clone(), ..Default::default() })
        .expect("Lauf")
        .trace
        .render();
    assert!(base.contains("out a 11"), "geladene Werte erwartet:\n{base}");
    for order in [5u64, 55, 555] {
        let other = run(
            &p,
            &Trace::default(),
            &RunOptions { ticks: 20, order_seed: Some(order), nvm: nvm.clone(), ..Default::default() },
        )
        .expect("Lauf")
        .trace
        .render();
        assert_eq!(other, base, "Reihenfolge {order}: der Trace haengt von der Schrittfolge ab");
    }
}

/// Ein Programm mit `persist var k : {ty} = {fallback}`, das `read` auf den
/// Output `x : {out}` legt; daraus der Wert in Tick 0 bei einem Speicher mit
/// `value` und ob `PersistReset` meldet.
fn load(decls: &str, ty: &str, fallback: &str, out: &str, read: &str, value: Value) -> (String, bool) {
    let p = compile(&format!(
        "{decls}
output x : {out} @ hw(\"o/x\") with safe = default

machine m:
    persist var k : {ty} = {fallback}
    initial RUN
    state RUN:
        loop:
            x = {read}
"
    ));
    let mut nvm = Nvm::new();
    nvm.put(only_hash(&p), value);
    let trace = simulate(&p, nvm, 1);
    let shown = trace.lines().find_map(|l| l.strip_prefix("t=0 out x ")).unwrap_or_default().to_string();
    (shown, trace.contains("PersistReset"))
}

/// 5.9 als Tabelle: Range-Grenzen gelten, knapp daneben gilt der Default
/// mit `PersistReset`; ebenso eine Variante, die das Enum nicht hat, ein
/// Array falscher Laenge und ein Gleitkommawert mit NaN- oder
/// Inf-Bitmuster — mit und ohne Range, denn `float` kennt nur endliche
/// Werte (4.1), und ein solches Feld ist keine gueltige Byteform.
#[test]
fn the_validation_table_of_5_9() {
    let int = |v: i64| load("", "int in 0..99", "7", "int in 0..99", "k", Value::Int(v));
    assert_eq!(int(0), ("0".into(), false));
    assert_eq!(int(99), ("99".into(), false));
    assert_eq!(int(100), ("7".into(), true));
    assert_eq!(int(-1), ("7".into(), true));

    let ranged = |v: f64| load("", "float in 0.0..10.0", "1.5", "float", "k", Value::F64(v));
    assert_eq!(ranged(10.0), ("10.0".into(), false));
    assert_eq!(ranged(f64::NAN), ("1.5".into(), true));
    assert_eq!(ranged(f64::INFINITY), ("1.5".into(), true));

    let enumerated = |variant: u32| {
        load("enum Mode: A, B, C\n", "Mode", "B", "Mode", "k", Value::Enum { variant, fields: Vec::new() })
    };
    assert_eq!(enumerated(2), ("C".into(), false));
    assert_eq!(enumerated(3), ("B".into(), true));

    let array = |n: i64| {
        let items = (0..n).map(|_| Value::Int(4)).collect();
        load("", "[3] int in 0..9", "[1, 2, 3]", "int in 0..9", "k[0]", Value::Array(items))
    };
    assert_eq!(array(3), ("4".into(), false));
    assert_eq!(array(2), ("1".into(), true));
}

/// Ohne Range: Auch hier sind NaN und Inf keine gueltige Byteform (5.9),
/// sonst gelangte ein nicht endlicher Wert in die Sprache (4.1).
#[test]
fn a_non_finite_float_without_a_range_falls_back_too() {
    for v in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let got = load("", "float", "1.5", "float", "k", Value::F64(v));
        assert_eq!(got, ("1.5".into(), true), "{v}");
    }
}

/// 5.9, Pruefung 23: Der Typ-Hash aendert sich mit allem, was die Bedeutung
/// der gespeicherten Bytes aendert — Breite, Gleitkommabreite, Array-Laenge,
/// Feld- und Variantenreihenfolge, Dauer gegen Zahl.
#[test]
fn every_change_of_meaning_changes_the_hash() {
    let hash = |decls: &str, var: &str| only_hash(&compile(&with_type(decls, var)));
    for (what, a, b) in [
        ("u8 -> u16", ("", "persist var k : u8 = 0"), ("", "persist var k : u16 = 0")),
        ("f32 -> f64", ("", "persist var k : f32 = 0.0"), ("", "persist var k : f64 = 0.0")),
        ("Array-Laenge", ("", "persist var k : [3] u8 = default"), ("", "persist var k : [4] u8 = default")),
        (
            "Feldreihenfolge",
            ("record Rec:\n    a : u8\n    b : u16\n", "persist var k : Rec = default"),
            ("record Rec:\n    b : u16\n    a : u8\n", "persist var k : Rec = default"),
        ),
        (
            "Variantenreihenfolge",
            ("enum Ev: A, B\n", "persist var k : Ev = A"),
            ("enum Ev: B, A\n", "persist var k : Ev = A"),
        ),
        ("Dauer gegen Zahl", ("", "persist var k : Duration = 0 ms"), ("", "persist var k : i64 = 0")),
    ] {
        assert_ne!(hash(a.0, a.1), hash(b.0, b.1), "{what}");
    }
}
