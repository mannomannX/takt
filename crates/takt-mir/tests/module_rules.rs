//! SYN-036: Je Modul ohne eigenen Test die Kernregel — an Programmen aus
//! der Sema, wie die Module sie im Compiler sehen.

use takt_mir::Program;
use takt_mir::expr::{ExprKind, Repr};
use takt_mir::fns::CostClass;
use takt_mir::stmt::{Place, StmtKind};

fn compile(src: &str) -> Program {
    let options = takt_sema::Options { build: takt_sema::Build::Sim, ..Default::default() };
    let out = takt_sema::compile(src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "unerwartete Fehler:\n{}", errors.join("\n"));
    out.program.expect("Programm")
}

const HEAD: &str = "system:
    language = 1
    tick = 1 ms
";

/// 3.4, Lemma 3.4 (narrow.rs): Ein `int` wird genau dann 32 Bit breit, wenn
/// sein bewiesenes Intervall in `i32` liegt — `i32::MIN` und `i32::MAX`
/// eingeschlossen, eins darueber oder darunter nicht.
#[test]
fn narrowing_stops_exactly_at_the_i32_bounds() {
    let p = compile(&format!(
        "{HEAD}
machine m:
    var a : int in -2147483648..2147483647 = 0
    var b : int in -2147483649..0 = 0
    var c : int in 0..2147483648 = 0
    var x : int = 0
    initial A
    state A:
        loop:
            x = a
            x = b
            x = c
"
    ));
    let m = &p.machines[0];
    let reprs: Vec<(String, Option<Repr>)> = m.states[0]
        .loop_block
        .stmts
        .iter()
        .filter_map(|s| match &s.kind {
            StmtKind::Assign { target: Place::Var(_), value } => match &value.kind {
                ExprKind::Var(v) => Some((m.vars[v.index()].name.clone(), value.repr)),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(
        reprs,
        vec![("a".into(), Some(Repr::I32)), ("b".into(), Some(Repr::I64)), ("c".into(), Some(Repr::I64))]
    );
}

/// 9.4.3 (cost.rs): Eine Integer-Operation zaehlt in der Klasse ihrer
/// Darstellung. Dieselbe Maschine mit verengtem und mit breitem Zaehler
/// verschiebt genau ihre Operationen von `i64` nach `i32`.
#[test]
fn an_integer_operation_costs_in_its_representation() {
    let cost = |range: &str| {
        let p = compile(&format!(
            "{HEAD}
machine m:
    var a : int in {range} = 0
    initial A
    state A:
        loop:
            if a < 100:
                a = a + 1
"
        ));
        takt_mir::analysis::cost::activation(&p, &p.machines[0]).total
    };
    let (narrow, wide) = (cost("0..1000"), cost("0..5000000000"));
    let moved = narrow.of(CostClass::I32) - wide.of(CostClass::I32);
    assert!(moved > 0, "verengt rechnet in i32: {narrow:?} gegen {wide:?}");
    assert_eq!(wide.of(CostClass::I64) - narrow.of(CostClass::I64), moved, "{narrow:?} gegen {wide:?}");
}

/// 9.4.3 (budget.rs): Der Bericht nennt `B_m` aus derselben Rechnung wie
/// das Budget, und je Klasse treibt der Zustand mit der Spitze sie. Der
/// teuerste Tick eines Zustands schliesst den Uebergang samt dem Eintritt
/// des Ziels ein (5.2): `COSTLY` geht nach `CHEAP`, nicht umgekehrt, sonst
/// truege der Eintritt seine Kosten in `CHEAP`.
#[test]
fn the_cost_report_names_the_state_that_drives_a_class() {
    let p = compile(&format!(
        "{HEAD}
machine m:
    var a : float = 0.0
    var n : int in 0..10 = 0
    initial COSTLY
    state COSTLY:
        loop:
            a = a * 1.5 * a * 2.5 * a * 0.5
            n = 1
        when n == 1: -> CHEAP
    state CHEAP:
        loop:
            n = 2
"
    ));
    let report = takt_mir::analysis::budget::report(&p);
    let m = &report.machines[0];
    assert_eq!(m.activation, takt_mir::analysis::cost::activation(&p, &p.machines[0]).total);
    let costly = m.states.iter().find(|s| s.name == "COSTLY").expect("COSTLY");
    assert!(costly.drives.contains(&CostClass::F64), "{:?}", m.states);
    let cheap = m.states.iter().find(|s| s.name == "CHEAP").expect("CHEAP");
    assert!(!cheap.drives.contains(&CostClass::F64), "{:?}", m.states);
}

/// 12.3 (stack.rs): Die Tiefe ist der laengste Pfad durch die gerufenen
/// Funktionen; fehlt der Rahmen einer erreichbaren Funktion, ist sie
/// unbekannt.
#[test]
fn the_stack_depth_is_the_longest_call_path() {
    let p = compile(&format!(
        "{HEAD}
fn inner(x: int) -> int:
    return x + 1

fn outer(x: int) -> int:
    return inner(x) * 2

machine m:
    var a : int = 0
    initial A
    state A:
        loop:
            a = outer(a)
"
    ));
    let index = |name: &str| p.fns.iter().position(|f| f.name == name).expect("Funktion");
    let mut frames = vec![None; p.fns.len()];
    frames[index("outer")] = Some(100);
    frames[index("inner")] = Some(40);
    let d = takt_mir::analysis::stack::depth(&p, &frames, &[]).expect("Tiefe");
    assert_eq!(d.bytes, 140);
    assert_eq!(d.path, vec!["outer".to_string(), "inner".to_string()]);
    frames[index("inner")] = None;
    assert_eq!(takt_mir::analysis::stack::depth(&p, &frames, &[]), None, "ein fehlender Rahmen");
}

/// 13.4 (requirements.rs): Jede `req`-ID sammelt ihre Stellen in
/// Quellreihenfolge, `check` wie `verify`.
#[test]
fn a_requirement_collects_every_site_that_checks_it() {
    let p = compile(&format!(
        "{HEAD}
input  p : int in 0..100 @ sim(\"i/p\")

machine m:
    initial A
    state A:
        loop:
            check p < 90, \"zu hoch\" req \"SR-1\"
            check p > 5, \"zu tief\" req \"SR-2\"
            check p < 95, \"viel zu hoch\" req \"SR-1\"
"
    ));
    let index = takt_mir::requirements::index(&p);
    assert_eq!(index.len(), 2);
    let sr1 = &index.by_req["SR-1"];
    assert_eq!(sr1.len(), 2);
    assert!(sr1[0].span.start < sr1[1].span.start, "Quellreihenfolge");
    assert!(sr1.iter().all(|s| s.kind == takt_mir::requirements::SiteKind::Check && s.machine_name == "m"));
}

/// 3.12 (capability.rs): `numeric` traegt `int` und `float`, `eq` keinen
/// Record mit Fliesskomma, `ord` auch `bool`.
#[test]
fn capabilities_follow_their_type_sets() {
    use takt_mir::capability::{Capability, contains_float, holds};
    use takt_mir::types::{FloatWidth, IntWidth, Type};
    let p = compile(&format!(
        "{HEAD}
record Plain:
    a : int
record Mixed:
    a : int
    b : float

machine m:
    var x : Plain = default
    var y : Mixed = default
    initial A
    state A:
        loop:
            pass
"
    ));
    let find = |want: &dyn Fn(&Type) -> bool| takt_mir::TypeId(p.types.list.iter().position(want).expect("Typ") as u32);
    let int = find(&|t| matches!(t, Type::Int { width: IntWidth::I64, unit: None, range: None }));
    let float = find(&|t| matches!(t, Type::Float { width: FloatWidth::F64, unit: None, range: None }));
    let boolean = find(&|t| matches!(t, Type::Bool));
    let record = |name: &str| {
        let r = p.records.iter().position(|r| r.name == name).expect("Record");
        find(&|t| matches!(t, Type::Record(id) if id.index() == r))
    };
    assert!(holds(&p, int, Capability::Numeric) && holds(&p, float, Capability::Numeric));
    assert!(!holds(&p, boolean, Capability::Numeric) && holds(&p, boolean, Capability::Ord));
    assert!(holds(&p, record("Plain"), Capability::Eq));
    assert!(!holds(&p, record("Mixed"), Capability::Eq) && contains_float(&p, record("Mixed"), 0));
    assert!(holds(&p, record("Mixed"), Capability::Pod));
}

/// visit.rs: Der Durchlauf erreicht jede Anweisung — `enter`, `loop`,
/// `exit`, Uebergangsaktionen, Handler und die Rumpfe darin.
#[test]
fn the_visit_reaches_every_statement() {
    let p = compile(&format!(
        "{HEAD}
input  rx : stream<line<32>> @ hw(\"uart0/rx\") with max_rate = 1 kHz, framing = lines, capacity_bytes = 128

machine m:
    var n : int in 0..100 = 0
    initial A
    state A:
        enter:
            log \"enter\"
        loop:
            if n < 10:
                log \"loop\"
        on rx matches \"go\":
            log \"handler\"
        when n == 5:
            log \"action\"
            -> B
        exit:
            log \"exit\"
    state B:
        loop:
            log \"b\"
"
    ));
    let mut logs = Vec::new();
    takt_mir::visit::for_each_stmt(&p.machines[0], &mut |s| {
        if let StmtKind::Observe(takt_mir::stmt::Observe::Log(f)) = &s.kind {
            logs.push(format!("{f:?}"));
        }
    });
    for want in ["enter", "loop", "exit", "handler", "action", "\"b\""] {
        assert!(logs.iter().any(|l| l.contains(want)), "`{want}` nicht erreicht: {logs:?}");
    }
    assert_eq!(logs.len(), 6, "{logs:?}");
}

/// 5.9 (bytes.rs): Die kanonische Form ist Little-Endian in der Breite des
/// Typs; eine Diskriminante hat immer acht Byte.
#[test]
fn the_canonical_form_is_little_endian_in_its_width() {
    use takt_mir::types::{FloatWidth, IntWidth};
    let mut e = takt_mir::bytes::Encoder::new();
    e.int(-2, IntWidth::I16);
    e.int(0x0102_0304, IntWidth::U32);
    e.float(1.5f32.to_bits().into(), FloatWidth::F32);
    e.bool(true);
    e.discriminant(3);
    e.len(2);
    assert_eq!(
        e.bytes,
        [&[0xfe, 0xff][..], &[4, 3, 2, 1], &1.5f32.to_le_bytes(), &[1], &3i64.to_le_bytes(), &2u32.to_le_bytes()]
            .concat()
    );
}
