//! `map<K, V, N>` (3.9): Einfuegen, Lesen, Entfernen und Iteration in
//! Slot-Reihenfolge — die Sondierung aus `takt_native::map`, im
//! Interpreter ueber Werte.

use takt_diag::Policy;
use takt_interp::{RunOptions, Trace, run};
use takt_mir::Program;
use takt_sema::{Build, Options};

const HEAD: &str = "system:\n    language = 1\n    tick = 10 ms\n\n";

fn compile(body: &str) -> Result<Program, Vec<String>> {
    let src = format!("{HEAD}{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    if errors.is_empty() { Ok(out.program.expect("Programm")) } else { Err(errors) }
}

fn ok(body: &str) -> Program {
    compile(body).unwrap_or_else(|e| panic!("unerwartete Fehler:\n{}", e.join("\n")))
}

fn trace(p: &Program, ticks: u64) -> String {
    run(p, &Trace::default(), &RunOptions { ticks, ..Default::default() }).expect("Lauf").trace.render()
}

#[test]
fn insert_get_and_remove_behave_like_a_map() {
    let p = ok("
output n : int in 0..8 @ hw(\"o/n\") with safe = 0
output v : int in 0..1000 @ hw(\"o/v\") with safe = 0
output full : bool @ hw(\"o/full\") with safe = true
output gone : bool @ hw(\"o/gone\") with safe = false
machine m:
    var seen : map<int, int, 4> = default
    var ok   : bool = false
    initial RUN
    state RUN:
        enter:
            ok = seen.insert(3, 30)
            ok = seen.insert(7, 70)
            ok = seen.insert(11, 110)
            ok = seen.insert(3, 31)
            n = seen.len
            v = seen.get(3).or(0)
            ok = seen.insert(15, 150)
            full = seen.insert(19, 190)
            gone = seen.remove(7)
            ok = seen.remove(7)
");
    let t = trace(&p, 1);
    assert!(t.contains("t=0 out n 3"), "{t}");
    assert!(t.contains("t=0 out v 31"), "ersetzt:\n{t}");
    assert!(t.contains("t=0 out full false"), "voll lehnt ab:\n{t}");
    assert!(t.contains("t=0 out gone true"), "{t}");
}

#[test]
fn iteration_visits_the_slots_in_order() {
    let p = ok("
output sum : int in 0..10000 @ hw(\"o/sum\") with safe = 0
output first : int in 0..100 @ hw(\"o/first\") with safe = 0
machine m:
    var seen : map<int, int, 8> = default
    var ok   : bool = false
    var acc  : int in 0..10000 = 0
    var head : int in 0..100 = 0
    var seen_first : bool = false
    initial RUN
    state RUN:
        enter:
            ok = seen.insert(5, 50)
            ok = seen.insert(3, 30)
            ok = seen.insert(11, 110)
            for (k, v) in seen:
                acc = acc + v
                if not seen_first:
                    head = k
                    seen_first = true
            sum = acc
            first = head
");
    let t = trace(&p, 1);
    assert!(t.contains("t=0 out sum 190"), "{t}");
    // Der erste Slot ist der mit der kleinsten Hash-Position (FNV-1a ueber
    // die auf 8 Byte aufgefuellte Form).
    let pos = |k: i64| takt_native::map::fnv1a(&k.to_le_bytes()) as usize % 8;
    let expected = [5i64, 3, 11].into_iter().min_by_key(|k| pos(*k)).expect("ein Schluessel");
    assert!(t.contains(&format!("t=0 out first {expected}")), "{t}");
}

#[test]
fn a_float_key_is_refused() {
    let e = compile(
        "
output n : int in 0..8 @ hw(\"o/n\") with safe = 0
machine m:
    var by_temp : map<float, int, 8> = default
    initial RUN
    state RUN:
        loop:
            n = by_temp.len
",
    )
    .expect_err("Fehler erwartet");
    assert!(e.join("\n").contains("SC-57"), "{e:?}");
}
