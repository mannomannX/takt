//! Implizite Pruefungen im erzeugten Code (4.1, 3.4).
//!
//! Was die Intervallanalyse nicht wegbeweist, steht als Zweig in den
//! Fault-Trampolin; was sie beweist, hinterlaesst keinen. Beides liest
//! sich an der IR ab; dass die Zweige auch *richtig* faulten, prueft der
//! Differenztest an `75_implicit_checks.takt`.

use takt_mir::program::Program;

mod common;

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
    out.program.unwrap_or_else(|| panic!("{name}: kein Programm"))
}

/// Die Rumpftexte aller Funktionen einer Maschine.
fn functions_of(ir: &str, machine: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for line in ir.lines() {
        if line.starts_with("define") {
            inside = line.contains(&format!("@{machine}_"));
        }
        if inside {
            out.push_str(line);
            out.push('\n');
        }
        if line == "}" {
            inside = false;
        }
    }
    out
}

#[test]
fn every_unproven_check_becomes_a_branch_and_every_proven_one_none() {
    let p = corpus("75_implicit_checks.takt");
    let ir = common::ir_of(&p);

    let ovf = functions_of(&ir, "overflow_u8");
    assert!(ovf.contains("@llvm.uadd.with.overflow.i8("), "u8-Addition mit Ueberlaufpruefung:\n{ovf}");

    let div = functions_of(&ir, "div_zero");
    assert!(div.contains("geprueft_div_"), "Divisor gegen null geprueft:\n{div}");
    assert!(!div.contains("geprueft_ovf_"), "12 / 1..3 kann nicht ueberlaufen:\n{div}");

    let shift = functions_of(&ir, "shift_amount");
    assert!(shift.contains("geprueft_shift_"), "Schiebebetrag geprueft:\n{shift}");
    assert!(shift.contains("trunc i64"), "der Betrag kommt auf die Breite des Werts:\n{shift}");

    let conv = functions_of(&ir, "convert_loss");
    assert!(conv.contains("geprueft_conv_"), "`as u8` geprueft:\n{conv}");
    assert!(conv.contains("icmp sle i64"), "die Obergrenze 255 wird geprueft:\n{conv}");

    let idx = functions_of(&ir, "index_write");
    assert!(idx.contains("index_ok"), "der Index der Zuweisungsstelle wird geprueft:\n{idx}");

    let fine = functions_of(&ir, "proven_fine");
    for mark in ["geprueft_", "index_ok", "with.overflow", "; Range"] {
        assert!(!fine.contains(mark), "`proven_fine` traegt `{mark}`:\n{fine}");
    }
    // Lemma 3.4: `n + 1` mit `n in 0..100` rechnet in 32 Bit.
    let hot = functions_of(&ir, "proven_fine_loop");
    assert!(
        hot.contains("add i32"),
        "die Addition laeuft in i32:
{hot}"
    );
    assert!(
        !hot.contains("add i64"),
        "keine 64-Bit-Addition im `loop:`:
{hot}"
    );
}

/// Der Platz einer Pruefungsart in [`CASES`]. Ein `match` ohne Platzhalter:
/// Eine neue Art in `CheckedKind` uebersetzt hier erst mit ihrem Fall.
fn slot(kind: &takt_mir::expr::CheckedKind) -> usize {
    use takt_mir::expr::CheckedKind as K;
    match kind {
        K::DivZero => 0,
        K::Overflow => 1,
        K::NonFinite => 2,
        K::Domain => 3,
        K::Index { .. } => 4,
        K::Range(_) => 5,
        K::Convert => 6,
        K::Shift => 7,
        K::Valid => 8,
        K::Missing => 9,
    }
}

/// Je Art: Name, Variablen und Anweisung, die die Analyse nicht beweisen
/// kann und die im Lauf faultet, Variablen und Anweisung, die sie beweist,
/// die Art des Faults und die Marke im IR (`geprueft_<art>_`), wo es eine
/// gibt.
const CASES: [(&str, &str, &str, &str, &str, &str, &str); 10] = [
    (
        "div",
        "var k : int in 0..9 = 0",
        "r = 100 / k",
        "var k : int in 1..9 = 1",
        "r = 100 / k",
        "Arithmetic(DivZero)",
        "geprueft_div_",
    ),
    (
        "ovf",
        "var k : int = 9223372036854775807",
        "r = k + 1",
        "var k : int in 0..9 = 9",
        "r = k + 1",
        "Arithmetic(Overflow)",
        "with.overflow",
    ),
    (
        "fin",
        "var a : float = 1e300",
        "y = a * a",
        "var a : float in 0.0..10.0 = 9.0",
        "y = a * a",
        "Arithmetic(NonFinite)",
        "geprueft_fin_",
    ),
    (
        "dom",
        "var a : float = -1.0",
        "y = sqrt(a)",
        "var a : float in 0.0..10.0 = 4.0",
        "y = sqrt(a)",
        "Arithmetic(Domain)",
        "geprueft_dom_",
    ),
    (
        "index",
        "var i : int in 0..9 = 7\n    var xs : [4] int = [1, 2, 3, 4]",
        "r = xs[i]",
        "var i : int in 0..3 = 3\n    var xs : [4] int = [1, 2, 3, 4]",
        "r = xs[i]",
        "RangeFault",
        "geprueft_index_",
    ),
    (
        "range",
        "var w : int in 0..20 = 9\n    var v : int in 0..10 = 0",
        "v = w + 5\n            r = v",
        "var w : int in 0..5 = 4\n    var v : int in 0..10 = 0",
        "v = w + 5\n            r = v",
        "RangeFault",
        "geprueft_range_",
    ),
    (
        "conv",
        "var w : int = 300",
        "r = (w as u8) as int",
        "var w : int in 0..255 = 200",
        "r = (w as u8) as int",
        "RangeFault",
        "geprueft_conv_",
    ),
    (
        "shift",
        "var s : int = 70",
        "r = 1 << s",
        "var s : int in 0..10 = 3",
        "r = 1 << s",
        "RangeFault",
        "geprueft_shift_",
    ),
    (
        "valid",
        "var u : int = 0",
        "r = level",
        "var u : int = 0",
        "if level.valid:\n                r = level",
        "SensorFault",
        "",
    ),
    (
        "missing",
        "var o : int? = none",
        "r = o",
        "var o : int? = none",
        "if o.valid:\n                r = o",
        "MissingValue",
        "",
    ),
];

/// Ein Programm, das in Tick 1 `statement` rechnet; ein Fault fuehrt nach
/// `SAFE` (probe 9), sonst steht probe 2.
fn checked_program(vars: &str, statement: &str) -> String {
    format!(
        "system:\n    language = 1\n    tick     = 10 ms\n\n\
         input  level : int in 0..100 @ hw(\"adc/level\") with max_age = 100 ms\n\
         output r     : int @ hw(\"o/r\") with safe = 0\n\
         output y     : float @ hw(\"o/y\") with safe = 0.0\n\
         output probe : int in 0..9 @ hw(\"o/probe\") with safe = 0\n\n\
         machine m:\n    fault -> SAFE\n    {vars}\n    initial RUN\n\
         \x20   state RUN:\n        enter:\n            probe = 1\n        after 10 ms: -> CALC\n\
         \x20   state CALC:\n        enter:\n            {statement}\n            probe = 2\n\
         \x20   state SAFE:\n        enter:\n            probe = 9\n"
    )
}

/// Die Pruefungsarten, die die MIR eines Programms traegt.
fn kinds(p: &Program) -> Vec<usize> {
    let mut out = Vec::new();
    for m in &p.machines {
        takt_mir::visit::for_each_expr_machine(m, &mut |e| {
            if let takt_mir::expr::ExprKind::Checked { kind, .. } = &e.kind {
                out.push(slot(kind));
            }
        });
    }
    out
}

/// **Je Pruefungsart ein unbewiesener und ein bewiesener Fall, mit Lauf**
/// (4.1, 3.4, 3.5, 3.8; KON2-020). Der unbewiesene traegt den `Checked`-Knoten
/// in der MIR und seine Marke im IR der Maschine und faultet in beiden
/// Implementierungen mit der Art aus 4.1; im bewiesenen steht keine Marke im
/// IR der Maschine — den Knoten darf die MIR behalten, die Bereiche an ihm
/// tragen den Beweis —, und er rechnet zu Ende. Damit haben die
/// Negativ-Asserts oben ihren positiven Anker.
#[test]
fn every_check_kind_faults_unproven_and_vanishes_proven() {
    let Some(clang) = common::clang() else { return };
    let mut covered = [false; CASES.len()];
    let mut failed = Vec::new();
    for (name, vars, unproven, proven_vars, proven, fault, mark) in CASES {
        for (label, vars, statement, checked) in
            [("unproven", vars, unproven, true), ("proven", proven_vars, proven, false)]
        {
            let src = checked_program(vars, statement);
            let options = takt_sema::Options { build: takt_sema::Build::Sim, ..Default::default() };
            let out = takt_sema::compile(&src, &options);
            let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
            if !errors.is_empty() {
                failed.push(format!("{name} {label}: {}", errors.join("; ")));
                continue;
            }
            let p = out.program.expect("Programm");
            let at = CASES.iter().position(|c| c.0 == name).expect("Fall");
            let present = kinds(&p).contains(&at);
            if checked && !present {
                failed.push(format!("{name} {label}: der Knoten fehlt in der MIR"));
            }
            covered[at] |= present && checked;
            let ir = functions_of(&common::ir_of(&p), "m");
            if !mark.is_empty() && ir.contains(mark) != checked {
                failed.push(format!(
                    "{name} {label}: die Marke `{mark}` steht {}im IR",
                    if checked { "nicht " } else { "" }
                ));
            }
            let interpreted = takt_interp::run(
                &p,
                &takt_interp::Trace::default(),
                &takt_interp::RunOptions { ticks: 3, ..Default::default() },
            )
            .expect("Lauf")
            .trace
            .render();
            let want = if checked { format!("t=1 fault m {fault}") } else { "t=1 out probe 2".to_string() };
            if !interpreted.contains(&want) {
                failed.push(format!("{name} {label}: `{want}` fehlt:\n{interpreted}"));
            }
            match common::run_native_all(&clang, &p, &format!("check_{name}_{label}"), 3) {
                Ok(native) => {
                    let diffs = takt_conformance::compare(&interpreted, &native);
                    if !diffs.is_empty() {
                        failed.push(format!("{name} {label}: {diffs:?}"));
                    }
                }
                Err(e) => failed.push(format!("{name} {label}: kein nativer Lauf: {e}")),
            }
        }
    }
    let missing: Vec<&str> = CASES.iter().zip(covered).filter(|(_, c)| !c).map(|(c, _)| c.0).collect();
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    assert!(missing.is_empty(), "ohne unbewiesenen Fall: {missing:?}");
}
