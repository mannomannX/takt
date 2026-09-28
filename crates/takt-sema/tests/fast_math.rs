//! Die schnellen Naeherungen der Bibliothek (4.2, 11.4; M10 Schritt 19):
//! Ihre dokumentierte Fehlerschranke haelt ueber den Parameterbereich und
//! in beiden Breiten. Das Programm zieht die Stellen mit einem festen
//! linearen Kongruenzgenerator, damit ein Fehlschlag sich wiederholen
//! laesst, und gibt Stellen und Ergebnisse aus; die Referenz rechnet der
//! Test in `f64` fuer den Wert, den das Programm tatsaechlich sah. Die
//! Bitgleichheit mit dem erzeugten Code prueft der Differentialtest
//! (Korpus 97).

use takt_diag::Policy;
use takt_interp::{RunOptions, Trace, run};
use takt_sema::{Build, Options};

const N: usize = 64;

/// Je Stelle die Eingaben `a`, `b` und das Ergebnis `r` aus `ticks` Ticks.
/// `body` setzt `r[i]` aus `a[i]` und `b[i]`; `a` und `b` sind
/// gleichverteilt in ihren Bereichen.
fn samples(width: &str, a: (f64, f64), b: (f64, f64), body: &str, ticks: u64) -> Vec<(f64, f64, f64)> {
    let src = format!(
        "system:\n    language = 1\n    tick = 1 ms\n    float = {width}\n\n\
output a : [{N}] float @ sim(\"o/a\")
output b : [{N}] float @ sim(\"o/b\")
output r : [{N}] float @ sim(\"o/r\")

machine m:
    var seed : int in 0..2147483647 = 12345
    initial RUN
    state RUN:
        loop:
            for i in range({N}):
                seed = (seed * 1103515245 + 12345) % 2147483648
                var u : float = (seed as float) / 2147483648.0
                seed = (seed * 1103515245 + 12345) % 2147483648
                var v : float = (seed as float) / 2147483648.0
                var x : float in {a0:?}..{a1:?} = {a0:?} + u * {ad:?}
                var y : float in {b0:?}..{b1:?} = {b0:?} + v * {bd:?}
                a[i] = x
                b[i] = y
                r[i] = {body}
",
        a0 = a.0,
        a1 = a.1,
        ad = a.1 - a.0,
        b0 = b.0,
        b1 = b.1,
        bd = b.1 - b.0,
    );
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "unerwartete Fehler:\n{}\n{src}", errors.join("\n"));
    let p = out.program.expect("Programm");
    let trace = run(&p, &Trace::default(), &RunOptions { ticks, ..Default::default() }).expect("Lauf").trace.render();
    // Der Trace schreibt ein `f32` in der kuerzesten Form, die als `f32`
    // zurueckkommt; als `f64` gelesen waere sie eine andere Zahl.
    let exact = |x: f64| if width == "f32" { f64::from(x as f32) } else { x };
    let column = |name: &str| -> Vec<Vec<f64>> {
        let tag = format!(" out {name} [");
        trace
            .lines()
            .filter_map(|l| l.split_once(&tag).map(|(_, v)| v.trim_end_matches(']')))
            .map(|v| v.split(", ").map(|x| exact(x.parse::<f64>().expect("Zahl"))).collect())
            .collect()
    };
    let (xa, xb, xr) = (column("a"), column("b"), column("r"));
    assert!(!xr.is_empty() && xa.len() == xr.len() && xb.len() == xr.len(), "{trace}");
    xa.iter().zip(&xb).zip(&xr).flat_map(|((a, b), r)| a.iter().zip(b).zip(r).map(|((a, b), r)| (*a, *b, *r))).collect()
}

/// Der groesste absolute Fehler gegen `f`.
fn absolute(s: &[(f64, f64, f64)], f: impl Fn(f64, f64) -> f64) -> f64 {
    s.iter().map(|(a, b, r)| (r - f(*a, *b)).abs()).fold(0.0, f64::max)
}

#[test]
fn sin_fast_and_cos_fast_keep_their_bounds() {
    for (width, bound) in [("f64", 5e-9), ("f32", 2e-7)] {
        let wide = (-1.0e4, 1.0e4);
        let near = (-4.0, 4.0);
        let sin = absolute(&samples(width, wide, near, "sin_fast(x)", 60), |a, _| a.sin());
        let cos = absolute(&samples(width, wide, near, "cos_fast(x)", 60), |a, _| a.cos());
        let sin_near = absolute(&samples(width, near, near, "sin_fast(x)", 60), |a, _| a.sin());
        let cos_near = absolute(&samples(width, near, near, "cos_fast(x)", 60), |a, _| a.cos());
        for (name, err) in [("sin", sin), ("cos", cos), ("sin nahe 0", sin_near), ("cos nahe 0", cos_near)] {
            assert!(err <= bound, "{name} in {width}: {err:e} > {bound:e}");
        }
    }
}

#[test]
fn exp_fast_keeps_its_relative_bound() {
    for (width, bound) in [("f64", 5e-9), ("f32", 3e-7)] {
        let s = samples(width, (-80.0, 80.0), (-1.0, 1.0), "exp_fast(x)", 60);
        let err = s.iter().map(|(a, _, r)| ((r - a.exp()) / a.exp()).abs()).fold(0.0, f64::max);
        assert!(err <= bound, "exp in {width}: {err:e} > {bound:e}");
    }
}

#[test]
fn atan2_fast_keeps_its_bound() {
    for (width, bound) in [("f64", 2e-8), ("f32", 5e-7)] {
        let unit = absolute(&samples(width, (-1.0, 1.0), (-1.0, 1.0), "atan2_fast(x, y)", 60), f64::atan2);
        let wide = absolute(&samples(width, (-1.0e6, 1.0e6), (-1.0, 1.0), "atan2_fast(x, y)", 60), f64::atan2);
        for (name, err) in [("atan2", unit), ("atan2 mit grossem y", wide)] {
            assert!(err <= bound, "{name} in {width}: {err:e} > {bound:e}");
        }
    }
}

/// `atan2_fast(0, 0)` ist null, nicht `NonFinite`.
#[test]
fn atan2_fast_of_the_origin_is_zero() {
    let s = samples("f64", (0.0, 0.0), (0.0, 0.0), "atan2_fast(x * 0.0, y * 0.0)", 1);
    assert!(s.iter().all(|(_, _, r)| *r == 0.0), "{s:?}");
}
