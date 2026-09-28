//! Die Gleitkomma-Intervalle der Analyse sind sicher (3.4, 4.2, FB-327).
//!
//! Eine bewiesene Pruefung verschwindet aus der MIR, fuer Interpreter und
//! Codegen zugleich; ein falscher Beweis fiele im Differential nicht auf.
//! Darum hier die Eigenschaft selbst: Fuer zufaellige Intervalle und Punkte
//! darin liegt das, was das Programm rechnet — korrekt gerundet in `f64`
//! oder in `f32` —, im Intervall, das die Analyse dafuer liefert.

use takt_mir::analysis::domain::Interval;

/// Ein kleiner, fester Zufall: xorshift64*, damit ein Fehlschlag sich
/// wiederholen laesst.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Eine endliche Zahl aus allen Groessenordnungen, auch subnormal.
    fn float(&mut self) -> f64 {
        let scales = [0.0, 1e-310, 1e-300, 1e-38, 1e-6, 0.1, 1.0, 3.0, 1e3, 1e6, 1e30, 1e300];
        let s = scales[(self.next() % scales.len() as u64) as usize];
        let unit = (self.next() >> 11) as f64 / (1u64 << 53) as f64;
        let v = s * (0.5 + unit);
        if self.next() % 2 == 0 { v } else { -v }
    }

    /// Ein Punkt in `lo..hi`: die Enden oder ein Wert dazwischen.
    fn within(&mut self, lo: f64, hi: f64) -> f64 {
        match self.next() % 4 {
            0 => lo,
            1 => hi,
            _ => {
                let t = (self.next() >> 11) as f64 / (1u64 << 53) as f64;
                (lo + (hi - lo) * t).clamp(lo, hi)
            }
        }
    }
}

fn pair(r: &mut Rng) -> (f64, f64) {
    let (a, b) = (r.float(), r.float());
    (a.min(b), a.max(b))
}

fn contains(i: Interval, v: f64) -> bool {
    match i {
        Interval::Float { lo, hi } => lo <= v && v <= hi,
        Interval::Top => true,
        _ => false,
    }
}

/// Eine Operation: Name, abstrakt, in `f64`, in `f32`.
type Op = (&'static str, fn(Interval, Interval) -> Interval, fn(f64, f64) -> f64, fn(f32, f32) -> f32);

const OPS: [Op; 4] = [
    ("+", |a, b| a + b, |x, y| x + y, |x, y| x + y),
    ("-", |a, b| a - b, |x, y| x - y, |x, y| x - y),
    ("*", |a, b| a * b, |x, y| x * y, |x, y| x * y),
    ("/", |a, b| a / b, |x, y| x / y, |x, y| x / y),
];

#[test]
fn f64_arithmetic_stays_inside_its_interval() {
    let mut r = Rng(0x9E37_79B9_7F4A_7C15);
    for _ in 0..20_000 {
        let ((al, ah), (bl, bh)) = (pair(&mut r), pair(&mut r));
        let (a, b) = (Interval::floats(al, ah), Interval::floats(bl, bh));
        for (name, abstract_op, op, _) in OPS {
            let i = abstract_op(a, b);
            for _ in 0..4 {
                let (x, y) = (r.within(al, ah), r.within(bl, bh));
                let v = op(x, y);
                if v.is_finite() {
                    assert!(contains(i, v), "{x:e} {name} {y:e} = {v:e} liegt nicht in {i:?}");
                } else {
                    assert_eq!(i, Interval::Top, "{x:e} {name} {y:e} ist nicht endlich, aber {i:?}");
                }
            }
        }
    }
}

#[test]
fn f32_arithmetic_stays_inside_its_rounded_interval() {
    let mut r = Rng(0xD1B5_4A32_D192_ED03);
    for _ in 0..20_000 {
        // Die Grenzen liegen auf dem `f32`-Raster, wie Werte eines
        // `f32`-Programms.
        let grid = |v: f64| f64::from(v as f32);
        let ((al, ah), (bl, bh)) = (pair(&mut r), pair(&mut r));
        let (al, ah, bl, bh) = (grid(al), grid(ah), grid(bl), grid(bh));
        if ![al, ah, bl, bh].iter().all(|v| v.is_finite()) {
            continue;
        }
        let (a, b) = (Interval::floats(al, ah), Interval::floats(bl, bh));
        for (name, abstract_op, _, op) in OPS {
            let i = abstract_op(a, b).on_f32_grid();
            for _ in 0..4 {
                let (x, y) = (r.within(al, ah) as f32, r.within(bl, bh) as f32);
                let v = op(x, y);
                if v.is_finite() {
                    assert!(contains(i, f64::from(v)), "{x:e} {name} {y:e} = {v:e} liegt nicht in {i:?}");
                } else {
                    assert_eq!(i, Interval::Top, "{x:e} {name} {y:e} ist in f32 nicht endlich, aber {i:?}");
                }
            }
        }
    }
}

#[test]
fn a_square_root_stays_inside_its_interval() {
    let mut r = Rng(0x2545_F491_4F6C_DD1D);
    for _ in 0..20_000 {
        let (lo, hi) = pair(&mut r);
        let (lo, hi) = (lo.abs().min(hi.abs()), lo.abs().max(hi.abs()));
        let i = Interval::floats(lo, hi).sqrt();
        let i32 = Interval::floats(f64::from(lo as f32), f64::from(hi as f32)).sqrt().on_f32_grid();
        let x = r.within(lo, hi);
        assert!(contains(i, x.sqrt()), "sqrt({x:e}) liegt nicht in {i:?}");
        let x32 = r.within(lo, hi) as f32;
        if (f64::from(lo as f32)..=f64::from(hi as f32)).contains(&f64::from(x32)) {
            assert!(contains(i32, f64::from(x32.sqrt())), "sqrt({x32:e}) in f32 liegt nicht in {i32:?}");
        }
    }
}

#[test]
fn an_integer_converts_inside_its_interval() {
    // `int as float` rundet zum Naechsten; die Analyse rechnet die Grenzen
    // ebenso um (walk.rs, `ExprKind::Cast`).
    let mut r = Rng(0x0123_4567_89AB_CDEF);
    for _ in 0..20_000 {
        let (a, b) = (r.next() as i64 >> (r.next() % 64), r.next() as i64 >> (r.next() % 64));
        let (lo, hi) = (a.min(b), a.max(b));
        let i = Interval::floats(lo as f64, hi as f64);
        let n = if r.next() % 2 == 0 { lo } else { hi.saturating_sub((r.next() % 3) as i64).max(lo) };
        assert!(contains(i, n as f64), "{n} als f64 liegt nicht in {i:?}");
        assert!(contains(i.on_f32_grid(), f64::from(n as f32)), "{n} als f32 liegt nicht in {:?}", i.on_f32_grid());
    }
}

#[test]
fn an_empty_or_unbounded_float_interval_proves_nothing() {
    assert_eq!(Interval::floats(1.0, 0.0), Interval::Top);
    assert_eq!(Interval::floats(0.0, f64::INFINITY), Interval::Top);
    assert_eq!(Interval::floats(f64::NAN, 1.0), Interval::Top);
    assert_eq!(Interval::floats(f64::MAX, f64::MAX) + Interval::floats(f64::MAX, f64::MAX), Interval::Top);
    assert_eq!(Interval::floats(1.0, 2.0) / Interval::floats(-1.0, 1.0), Interval::Top);
    assert_eq!(Interval::floats(1e39, 2e39).on_f32_grid(), Interval::Top, "jenseits von `f32::MAX`");
}
