//! Matrizen fester Groesse, uniforme Form (3.11): Typen, Literale,
//! Operatoren, Methoden und `solve` im Interpreter; Pruefung 30 und 42.

use takt_diag::Policy;
use takt_interp::{RunOptions, Trace, run};
use takt_mir::Program;
use takt_sema::{Build, Options};

const HEAD: &str = "system:\n    language = 1\n    tick = 1 ms\n\n";

fn compile(body: &str) -> Result<(Option<Program>, Vec<String>), Vec<String>> {
    let src = format!("{HEAD}{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
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
    assert!(e.contains("SC-3") && e.contains("`float[K]`") && e.contains("`float[K^2]`"), "{e}");
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
    // `solve` auf derselben Matrix faultet ebenso (3.11).
    let t = trace(
        "
output d : float @ hw(\"o/d\") with safe = 0
machine m:
    var a : mat<2, 2> = [[1, 2], [2, 4]]
    initial RUN
    state RUN:
        loop:
            var x = solve(a, [[1], [2]])
            d = x[0, 0]
",
    );
    assert!(t.contains("fault m Arithmetic(Singular)"), "{t}");
}

/// Laufzeitverhalten (3.11, 4.1): `cholesky` einer indefiniten Matrix ist
/// `none`, die Determinante einer singulaeren ist 0 ohne Fault, und ein
/// Produkt, das ueberlaeuft, faultet `Arithmetic(NonFinite)`.
#[test]
fn indefinite_singular_and_overflowing_matrices_behave_as_3_11_says() {
    let t = trace(
        "
output pd  : bool  @ hw(\"o/pd\")  with safe = true
output d   : float @ hw(\"o/d\")   with safe = 7
output big : float @ hw(\"o/big\") with safe = 0
machine m:
    fault -> SAFE
    var a : mat<2, 2> = [[1, 2], [2, 1]]
    var s : mat<2, 2> = [[1, 2], [2, 4]]
    var g : mat<2, 2> = [[1.0e200, 0], [0, 1]]
    initial RUN
    state RUN:
        loop:
            pd = a.cholesky().valid
            d = s.det()
            var h = g * g
            big = h[0, 0]
    state SAFE:
        enter:
            big = 7
",
    );
    for line in [
        "t=0 fault m Arithmetic(NonFinite) \"Matrixergebnis nicht endlich\" -> SAFE",
        "t=0 out pd false",
        "t=0 out d 0.0",
        "t=0 out big 7.0",
    ] {
        assert!(t.contains(line), "`{line}` fehlt:\n{t}");
    }
    assert_eq!(t.matches(" fault ").count(), 1, "nur das Produkt faultet:\n{t}");
}

/// 3.11, Literal: Bilden die Elementeinheiten ein aeusseres Produkt, ist das
/// Literal ohne Deklaration typisierbar — hier `mat[(m, m/s), (1, 1/s)]`.
#[test]
fn an_outer_product_literal_types_without_a_declaration() {
    let ok = "
output a : float[m/s^2] @ hw(\"o/a\") with safe = 0
machine m:
    initial RUN
    state RUN:
        loop:
            var q = [[1 m, 2 m/s], [3 m/s, 4 m/s^2]]
            a = q[1, 1]
";
    let t = trace(ok);
    assert!(t.contains("t=0 out a 4.0 m/s^2"), "{t}");
}

/// Tabelle 10, Pruefung 34: Ein Literal, dessen Elementeinheiten kein
/// aeusseres Produkt bilden, ist ein Fehler dieser Pruefung.
#[test]
fn a_literal_that_is_no_outer_product_is_check_34() {
    let bad = "
output a : float[m] @ hw(\"o/a\") with safe = 0
machine m:
    initial RUN
    state RUN:
        loop:
            var q = [[1 m, 1 m/s], [1 m/s, 1 m]]
            a = q[0, 0]
";
    let e = compile(bad).expect_err("kein aeusseres Produkt").join("\n");
    assert!(e.contains("SC-34"), "{e}");
}

/// `corpus-try/108_singular_solve.takt` im Interpreter: `solve` rechnet,
/// solange die Determinante 3, 2, 1 ist, und faultet `Arithmetic(Singular)`
/// genau im Tick, in dem sie 0 wird; `cholesky` der indefiniten Matrix ist
/// `none` und faellt auf die Einheitsmatrix zurueck. Der Scratch ist der
/// von `solve` einer 2x2 (56 Byte), wie im Kalman-Schritt.
#[test]
fn solve_faults_in_the_tick_its_matrix_becomes_singular() {
    let src = include_str!("../../../corpus-try/108_singular_solve.takt");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
    let p = takt_sema::compile(src, &options).program.expect("Programm");
    assert_eq!(p.machines[0].layout.scratch_bytes, Some(56));
    let t = run(&p, &Trace::default(), &RunOptions { ticks: 5, ..Default::default() }).expect("Lauf").trace.render();
    for line in [
        "t=0 out x1 0.3333333333333333",
        "t=0 out l10 0.0",
        "t=0 out pd false",
        "t=1 out x0 0.0",
        "t=1 out x1 0.5",
        "t=2 out x0 -1.0",
        "t=2 out x1 1.0",
        "t=3 fault m Arithmetic(Singular) \"Matrix singulaer\" -> SAFE",
        "t=3 out x0 7.0",
    ] {
        assert!(t.contains(line), "`{line}` fehlt:\n{t}");
    }
}
