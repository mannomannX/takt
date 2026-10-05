//! Konstantenvariablen in Generics (Referenz 3.12, v1.1; Pruefung 52):
//! `[const N in a..b]` in Typen, `range(N)` und als Konstante in der
//! Rechnung; aus den Argumenten abgeleitet oder explizit, monomorphisiert
//! je Wert.

use takt_diag::Policy;
use takt_interp::{RunOptions, Trace, run};
use takt_mir::Program;
use takt_sema::{Build, Options};

const HEAD: &str = "system:\n    language = 1\n    tick = 1 ms\n\n";

fn compile(body: &str) -> (Option<Program>, Vec<String>) {
    let src = format!("{HEAD}{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
    let out = takt_sema::compile(&src, &options);
    let diags: Vec<String> = out.diagnostics.iter().map(|d| format!("{d}")).collect();
    let errors = out.diagnostics.iter().any(|d| d.is_error());
    (if errors { None } else { out.program }, diags)
}

fn ok(body: &str) -> Program {
    let (p, diags) = compile(body);
    p.unwrap_or_else(|| panic!("unerwartete Fehler:\n{}", diags.join("\n")))
}

fn errors(body: &str) -> String {
    let (p, diags) = compile(body);
    assert!(p.is_none(), "Fehler erwartet");
    diags.join("\n")
}

fn trace(p: &Program, ticks: u64) -> String {
    run(p, &Trace::default(), &RunOptions { ticks, ..Default::default() }).expect("Lauf").trace.render()
}

const TOTAL: &str = "
fn total[const N in 1..64](values: [N] int) -> int:
    var s : int = 0
    for i in range(N):
        s = s + values[i]
    return s
";

#[test]
fn a_constant_is_inferred_from_the_capacity_of_the_argument() {
    // 3.12: `fn first[const N](v: vec<T, N>)` — hier ueber ein Array.
    let p = ok(&format!(
        "{TOTAL}
output s4 : int in 0..9999 @ hw(\"o/s4\") with safe = 0
output s8 : int in 0..9999 @ hw(\"o/s8\") with safe = 0

machine m:
    var x : [4] int = [1, 2, 3, 4]
    var y : [8] int = [1, 1, 1, 1, 1, 1, 1, 1]
    initial RUN
    state RUN:
        loop:
            s4 = total(x)
            s8 = total(y)
"
    ));
    let names: Vec<&str> = p.fns.iter().map(|f| f.name.as_str()).filter(|n| n.starts_with("total[")).collect();
    assert_eq!(names, ["total[4]", "total[8]"], "eine Instanz je Kapazitaet");
    let t = trace(&p, 2);
    assert!(t.contains("out s4 10") && t.contains("out s8 8"), "{t}");
}

#[test]
fn an_explicit_constant_instantiates_a_block() {
    // 11.4: `block window_mean[U, const N in 1..1024]()`; `N` ist in der
    // Instanz eine Konstante, auch in der Rechnung.
    let p = ok("
block window_mean[U, const N in 1..1024]():
    var buf  : [N] float[U] = default
    var next : int in 0..N = 0
    step(x: float[U]) -> float[U]:
        buf[next] = x
        next = next + 1
        if next >= N:
            next = 0
        var s : float[U] = 0 U
        for i in range(N):
            s = s + buf[i]
        return s / (N as float)

output y : float[bar] @ hw(\"o/y\") with safe = 0

machine m:
    var w = window_mean[bar, 4]()
    var p : float[bar] = 0 bar
    initial RUN
    state RUN:
        loop:
            p = p + 4 bar
            y = w.step(p)
");
    let names: Vec<&str> = p.blocks.iter().map(|b| b.name.as_str()).collect();
    assert!(names.contains(&"window_mean[bar, 4]"), "{names:?}");
    let t = trace(&p, 5);
    // 4, 8, 12, 16 -> Mittel 10; nach dem fuenften Schritt (20): (20+8+12+16)/4 = 14
    assert!(t.contains("out y 10.0") && t.contains("out y 14.0"), "{t}");
}

#[test]
fn the_range_of_a_constant_is_checked_at_instantiation() {
    // Pruefung 52.
    let e = errors(
        "fn total[const N in 1..8](values: [N] int) -> int:
    return 0

output a : int @ hw(\"o/a\") with safe = 0

machine m:
    var x : [16] int = default
    initial RUN
    state RUN:
        loop:
            a = total(x)
",
    );
    assert!(e.contains("SC-52") && e.contains("ausserhalb 1..8"), "{e}");
}

#[test]
fn a_constant_needs_a_constant_argument() {
    // Pruefung 52: an `const N` steht keine Einheit.
    let e = errors(&format!(
        "{TOTAL}
output a : int @ hw(\"o/a\") with safe = 0

machine m:
    var x : [4] int = default
    initial RUN
    state RUN:
        loop:
            a = total[bar](x)
"
    ));
    assert!(e.contains("SC-52") && e.contains("verlangt eine Konstante"), "{e}");
}

#[test]
fn a_named_constant_binds_a_constant_variable() {
    // `[SIZE]` liest der Parser als Einheit; an `const N` ist es die Konstante.
    let p = ok(&format!(
        "const SIZE : int = 4
{TOTAL}
output a : int @ hw(\"o/a\") with safe = 0

machine m:
    var x : [4] int = [2, 2, 2, 2]
    initial RUN
    state RUN:
        loop:
            a = total[SIZE](x)
"
    ));
    assert!(t_has(&trace(&p, 2), "out a 8"));
}

fn t_has(t: &str, line: &str) -> bool {
    t.contains(line)
}

#[test]
fn a_buffer_can_be_optional() {
    // FB-94: `bytes<N>?` ist ein Typ wie jedes `T?` (3.8).
    let p = ok("fn head(b: bytes<4>?) -> int:
    if b.valid:
        return 1
    return 0

output n : int @ hw(\"o/n\") with safe = 0

machine m:
    var x : bytes<4>? = none
    initial RUN
    state RUN:
        loop:
            n = head(x)
");
    assert!(t_has(&trace(&p, 2), "out n 0"));
}

const SMALL: &str = "fn total[const N in 1..8](values: [N] int) -> int:
    var s : int = 0
    for i in range(N):
        s = s + values[i]
    return s

output a : int @ hw(\"o/a\") with safe = 0
";

/// Ein Programm, das `total` mit einem Argument der Laenge `len` ruft.
fn calls_total(len: u32, call: &str) -> String {
    format!(
        "{SMALL}
machine m:
    var x : [{len}] int = default
    initial RUN
    state RUN:
        loop:
            a = {call}
"
    )
}

#[test]
fn the_range_of_a_constant_holds_at_its_edges() {
    // Pruefung 52: 1 und 8 liegen in `1..8`, 9 nicht — abgeleitet wie
    // explizit.
    for len in [1, 8] {
        let p = ok(&calls_total(len, "total(x)"));
        assert!(p.fns.iter().any(|f| f.name == format!("total[{len}]")), "N = {len}");
        ok(&calls_total(len, &format!("total[{len}](x)")));
    }
    for call in ["total(x)", "total[9](x)"] {
        let e = errors(&calls_total(9, call));
        assert!(e.contains("[SC-52]") && e.contains("ausserhalb 1..8"), "{call}: {e}");
    }
    let e = errors(&calls_total(1, "total[0](x)"));
    assert!(e.contains("[SC-52]") && e.contains("ausserhalb 1..8"), "N = 0: {e}");
}

#[test]
fn an_explicit_constant_must_match_the_capacity_of_the_argument() {
    let e = errors(&calls_total(4, "total[3](x)"));
    assert!(e.contains("[SC-3]") && e.contains("[3] int") && e.contains("[4] int"), "{e}");
    let e = errors(&format!("const SIZE : int = 5\n{}", calls_total(4, "total[SIZE](x)")));
    assert!(e.contains("[SC-3]") && e.contains("[5] int"), "{e}");
}

#[test]
fn a_malformed_constant_variable_is_rejected_at_the_template() {
    let e = errors(&calls_total(4, "total(x)").replace("const N in 1..8", "const N in 8..1"));
    assert!(e.contains("[SC-52]") && e.contains("Untergrenze groesser als Obergrenze"), "{e}");
    let e = errors(&calls_total(4, "total(x)").replace("const N in 1..8", "const N in 1..8, const N in 1..8"));
    assert!(e.contains("[SC-52]") && e.contains("Variable `N` doppelt"), "{e}");
}

#[test]
fn equal_capacities_share_one_instance_and_a_constant_needs_no_range() {
    // 3.12: eine MIR-Funktion je Instanz — zwei Aufrufe mit `[4] int` sind
    // eine, sonst zaehlte das Budget sie doppelt.
    let p = ok(&format!(
        "{TOTAL}
fn count[const N](values: [N] int) -> int:
    return N

output b : int @ hw(\"o/b\") with safe = 0
output c : int @ hw(\"o/c\") with safe = 0

machine m:
    var x : [4] int = [1, 2, 3, 4]
    var y : [4] int = [4, 4, 4, 4]
    initial RUN
    state RUN:
        loop:
            b = total(x) + total(y)
            c = count(x)
"
    ));
    let names: Vec<&str> = p.fns.iter().map(|f| f.name.as_str()).filter(|n| n.starts_with("total[")).collect();
    assert_eq!(names, ["total[4]"], "eine Instanz");
    let t = trace(&p, 1);
    assert!(t.contains("out b 26") && t.contains("out c 4"), "{t}");
}
