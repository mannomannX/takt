//! Vektordateien neben den Tests: `<breite> <funktion>: <argument…> -> <ergebnis>`.

use std::fs;

/// Eine Vektorzeile `<breite> <funktion>: <argument…> -> <ergebnis>`.
pub struct Vector {
    pub text: String,
    pub f64: bool,
    pub fun: String,
    pub args: Vec<u64>,
    pub want: u64,
}

/// Liest eine Vektordatei neben diesem Test.
pub fn load(name: &str) -> Vec<Vector> {
    let path = format!("{}/tests/{name}", env!("CARGO_MANIFEST_DIR"));
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|line| {
            let bad = || -> ! { panic!("{name}: `{line}` ist keine Vektorzeile") };
            let (head, tail) = line.split_once(':').unwrap_or_else(|| bad());
            let (width, fun) = head.split_once(' ').unwrap_or_else(|| bad());
            let (args, want) = tail.split_once("->").unwrap_or_else(|| bad());
            let hex = |s: &str| u64::from_str_radix(s.trim(), 16).unwrap_or_else(|_| bad());
            Vector {
                text: line.to_string(),
                f64: width == "f64",
                fun: fun.to_string(),
                args: args.split_whitespace().map(hex).collect(),
                want: hex(want),
            }
        })
        .collect()
}

/// Rechnet einen Vektor nach; `None` fuer eine unbekannte Funktion.
pub fn apply(v: &Vector) -> Option<u64> {
    let a = |i: usize| f64::from_bits(v.args.get(i).copied().unwrap_or(0));
    let b = |i: usize| f32::from_bits(v.args.get(i).copied().unwrap_or(0) as u32);
    let wide = |x: f64| Some(x.to_bits());
    let narrow = |x: f32| Some(u64::from(x.to_bits()));
    match (v.f64, v.fun.as_str()) {
        (true, "exp") => wide(libtaktm::exp_f64(a(0))),
        (true, "log") => wide(libtaktm::log_f64(a(0))),
        (true, "sin") => wide(libtaktm::sin_f64(a(0))),
        (true, "cos") => wide(libtaktm::cos_f64(a(0))),
        (true, "tan") => wide(libtaktm::tan_f64(a(0))),
        (true, "asin") => wide(libtaktm::asin_f64(a(0))),
        (true, "acos") => wide(libtaktm::acos_f64(a(0))),
        (true, "atan") => wide(libtaktm::atan_f64(a(0))),
        (true, "atan2") => wide(libtaktm::atan2_f64(a(0), a(1))),
        (true, "pow") => wide(libtaktm::pow_f64(a(0), a(1))),
        (true, "fma") => wide(libtaktm::fma_f64(a(0), a(1), a(2))),
        (false, "exp") => narrow(libtaktm::exp_f32(b(0))),
        (false, "log") => narrow(libtaktm::log_f32(b(0))),
        (false, "sin") => narrow(libtaktm::sin_f32(b(0))),
        (false, "cos") => narrow(libtaktm::cos_f32(b(0))),
        (false, "tan") => narrow(libtaktm::tan_f32(b(0))),
        (false, "asin") => narrow(libtaktm::asin_f32(b(0))),
        (false, "acos") => narrow(libtaktm::acos_f32(b(0))),
        (false, "atan") => narrow(libtaktm::atan_f32(b(0))),
        (false, "atan2") => narrow(libtaktm::atan2_f32(b(0), b(1))),
        (false, "pow") => narrow(libtaktm::pow_f32(b(0), b(1))),
        (false, "fma") => narrow(libtaktm::fma_f32(b(0), b(1), b(2))),
        _ => None,
    }
}
