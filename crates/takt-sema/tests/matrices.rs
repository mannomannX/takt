//! Matrizen fester Groesse, uniforme Form (3.11): Typen, Literale,
//! Operatoren, Methoden und `solve` im Interpreter; Pruefung 30 und 42.

use takt_diag::Policy;
use takt_interp::{RunOptions, Trace, run};
use takt_mir::Program;
use takt_sema::{Build, Options};

const HEAD: &str = "system:\n    language = 1\n    tick = 1 ms\n\n";

fn compile(body: &str) -> Result<(Option<Program>, Vec<String>), Vec<String>> {
    let src = format!("{HEAD}{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    let warnings: Vec<String> = out.diagnostics.iter().filter(|d| !d.is_error()).map(|d| format!("{d}")).collect();
    if errors.is_empty() { Ok((out.program, warnings)) } else { Err(errors) }
}

fn trace(body: &str) -> String {
    let (p, _) = compile(body).expect("uebersetzt");
    run(&p.expect("Programm"), &Trace::default(), &RunOptions { ticks: 1, ..Default::default() })
        .expect("Lauf")
        .trace
        .render()
}

const KALMAN: &str = "
output s11 : float @ hw(\"o/s11\") with safe = 0
output k10 : float @ hw(\"o/k10\") with safe = 0
output d   : float @ hw(\"o/d\")   with safe = 0
output x1  : float @ hw(\"o/x1\")  with safe = 0
output c10 : float @ hw(\"o/c10\") with safe = 0
output e   : float @ hw(\"o/e\")   with safe = 0
output pd  : bool  @ hw(\"o/pd\")  with safe = false

const I2 : mat<2, 2> = [[1, 0], [0, 1]]
const H  : mat<1, 2> = [[1, 0]]

machine m:
    var p : mat<2, 2> = [[2, 1], [1, 3]]
    initial RUN
    state RUN:
        loop:
            var s : mat<1, 1> = H * p * H.transpose()
            var k = p * H.transpose() * s.inv()
            var q = (I2 - k * H) * p
            s11 = s[0, 0]
            k10 = k[1, 0]
            d = p.det()
            var x = solve(p, [[1], [2]])
            x1 = x[1, 0]
            var c = p.cholesky()
            pd = c.valid
            c10 = c.or(I2)[1, 0]
            e = q[0, 1]
            p[0, 1] = 0.5 * p[1, 0]
";

#[test]
fn the_scratch_of_the_largest_operation_is_recorded() {
    let (p, _) = compile(KALMAN).expect("uebersetzt");
    // `solve` einer 2×2 in f64: LU 32 Byte, Loesung 16 Byte, zwei Zeilenindizes.
    assert_eq!(p.expect("Programm").machines[0].layout.scratch_bytes, Some(56));
}

#[test]
fn a_kalman_step_computes_through_the_library() {
    let t = trace(KALMAN);
    for line in [
        "out s11 2.0",
        "out k10 0.5",
        "out d 5.0",
        "out x1 0.6",
        "out pd true",
        "out c10 0.7071067811865475",
        "out e 0.0",
    ] {
        assert!(t.contains(&format!("t=0 {line}\n")), "{line} fehlt:\n{t}");
    }
}

const KALMAN_DIM: &str = "
unitvec X = (m, m/s)
unitvec Z = (m)
output pos : float[m] @ hw(\"o/pos\") with safe = 0
output vel : float[m/s] @ hw(\"o/vel\") with safe = 0
output p11 : float[m^2/s^2] @ hw(\"o/p11\") with safe = 0
const F : mat[X, 1/X] = [[1, (10 ms).as(s)], [0 1/s, 1]]
const H : mat[Z, 1/X] = [[1, (0 s).as(s)]]
const I : mat[X, 1/X] = [[1, (0 s).as(s)], [0 1/s, 1]]
const Q : mat[X, X] = [[0 m^2, 0 m^2/s], [0 m^2/s, 0 m^2/s^2]]
const R : mat[Z, Z] = [[1 m^2]]

machine m:
    var x : vec[X] = [0 m, 1 m/s]
    var p : mat[X, X] = [[1 m^2, 0 m^2/s], [0 m^2/s, 1 m^2/s^2]]
    var z : vec[Z] = [2 m]
    initial RUN
    state RUN:
        loop:
            x = F * x
            p = F * p * F.transpose() + Q
            var s = H * p * H.transpose() + R
            var k = p * H.transpose() * s.inv()
            x = x + k * (z - H * x)
            p = (I - k * H) * p
            pos = x[0, 0]
            vel = x[1, 0]
            p11 = p[1, 1]
";

#[test]
fn a_dimensioned_kalman_filter_is_unit_checked_and_runs() {
    let t = trace(KALMAN_DIM);
    for prefix in ["out pos 1.00504", "out vel 1.00994", "out p11 0.99995"] {
        assert!(t.contains(&format!("t=0 {prefix}")), "{prefix} fehlt:\n{t}");
    }
}

#[test]
fn unit_tuples_are_checked_by_hart() {
    let head = "
unitvec X = (m, m/s)
output pos : float[m] @ hw(\"o/pos\") with safe = 0
const F : mat[X, 1/X] = [[1, (10 ms).as(s)], [0 1/s, 1]]
machine m:
    var x : vec[X] = [0 m, 1 m/s]
    var p : mat[X, X] = [[1 m^2, 0 m^2/s], [0 m^2/s, 1 m^2/s^2]]
    var i : int in 0..1 = 0
    initial RUN
    state RUN:
        loop:
";
    for (body, want) in [
        ("var a = p * F\n", "Spalteneinheiten"),
        ("var a = p + F\n", "passen nicht"),
        ("pos = x[i, 0]\n", "variabler Index"),
        ("var l = F.cholesky()\n", "symmetrische Einheiten"),
        ("var y = solve(p, F.transpose())\n", "Zeileneinheiten"),
    ] {
        let e = compile(&format!("{head}            {body}")).expect_err(body).join("\n");
        assert!(e.contains("SC-34") && e.contains(want), "{body}:\n{e}");
    }
    let ok = format!(
        "{head}            var l = p.cholesky()\n            var ok = l.valid\n            var y = solve(p, x)\n            pos = y[0, 0] * (1 m^2)\n"
    );
    let e = compile(&ok);
    assert!(e.is_ok(), "{e:?}");
}

#[test]
fn shapes_are_checked_at_compile_time() {
    for (body, want) in [
        ("var a : mat<2, 3> = [[1, 2, 3], [4, 5, 6]]\n            var b = a * a\n", "Spalten = Zeilen"),
        ("var a : mat<2, 3> = [[1, 2, 3], [4, 5, 6]]\n            var b = a.inv()\n", "quadratische"),
        (
            "var a : mat<2, 2> = [[1, 2], [3, 4]]\n            var b : mat<2, 1> = [[1], [2]]\n            var c = a + b\n",
            "gleiche Form",
        ),
        ("var a : mat<2, 2> = [[1, 2], [3, 4]]\n            var x = a[2, 0]\n", "Zeile `2` nicht in 0..1"),
        ("var a : mat<2, 2> = [[1, 2, 3], [3, 4, 5]]\n", "2 Elemente je Zeile"),
        (
            "var a : mat<2, 2> = [[1, 2], [3, 4]]\n            var i : int in 0..2 = 0\n            var x = a[i, 0]\n",
            "Zeile `int in 0..2` nicht in 0..1",
        ),
        (
            "var a : mat<2, 2> = [[1, 2], [3, 4]]\n            var y = solve(a, [[1], [2], [3]])\n",
            "2 Zeilen erwartet, 3 gefunden",
        ),
    ] {
        let src = format!("machine m:\n    initial RUN\n    state RUN:\n        loop:\n            {body}");
        let e = compile(&src).expect_err(body).join("\n");
        assert!(e.contains("SC-30") && e.contains(want), "{body}:\n{e}");
    }
}

#[test]
fn units_multiply_through_matrices() {
    let ok = "
output w : float[K^2] @ hw(\"o/w\") with safe = 0
machine m:
    var g : mat<2, 2>[K] = [[1 K, 0 K], [0 K, 1 K]]
    initial RUN
    state RUN:
        loop:
            var h = g * g
            w = h[0, 0]
            var v : mat<2, 2>[K] = h.cholesky().or(g) * 1.0
            var z : mat<2, 2> = v / (2 K)
";
    let e = compile(ok);
    assert!(e.is_ok(), "{e:?}");
    let bad = "
output w : float[K] @ hw(\"o/w\") with safe = 0
machine m:
    var g : mat<2, 2>[K] = [[1 K, 0 K], [0 K, 1 K]]
    initial RUN
    state RUN:
        loop:
            w = (g * g)[0, 0]
";
    let e = compile(bad).expect_err("K^2 ist kein K").join("\n");
    assert!(e.contains("K"), "{e}");
}

#[test]
fn large_matrices_get_a_lint_and_singular_ones_fault() {
    let (_, warnings) = compile("machine m:\n    var a : mat<17, 2> = default\n    initial RUN\n    state RUN:\n        loop:\n            pass\n").expect("uebersetzt");
    assert!(warnings.iter().any(|w| w.contains("SC-42") && w.contains("17×2")), "{warnings:?}");
    let t = trace(
        "
output d : float @ hw(\"o/d\") with safe = 0
machine m:
    var a : mat<2, 2> = [[1, 2], [2, 4]]
    initial RUN
    state RUN:
        loop:
            d = a.det()
            var b = a.inv()
            d = b[0, 0]
",
    );
    assert!(t.contains("fault m Arithmetic") && t.contains("singulaer"), "{t}");
}
