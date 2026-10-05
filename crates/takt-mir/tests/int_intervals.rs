//! Die Ganzzahl-Intervalle der Analyse sind sicher (3.4, 4.1): Fuer
//! zufaellige Intervalle an den Grenzen von `i64` und um die Null liegt
//! jedes exakte Ergebnis zweier Punkte darin im Intervall, das die Analyse
//! liefert. Eine bewiesene Pruefung faellt aus der MIR; ein zu enges
//! Intervall faende im Differential niemand.

use takt_mir::analysis::domain::{Domain, Interval, Intervals};

/// xorshift64*, damit ein Fehlschlag sich wiederholen laesst.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Ein Wert an einer der Kanten, die Fehler finden, oder zufaellig.
    fn edge(&mut self) -> i128 {
        const EDGES: [i128; 14] = [
            i64::MIN as i128,
            i64::MIN as i128 + 1,
            -(1 << 32),
            i32::MIN as i128,
            -1000,
            -2,
            -1,
            0,
            1,
            2,
            1000,
            i32::MAX as i128,
            i64::MAX as i128 - 1,
            i64::MAX as i128,
        ];
        match self.next() % 3 {
            0 => i128::from(self.next() as i64),
            _ => EDGES[(self.next() % EDGES.len() as u64) as usize],
        }
    }

    fn interval(&mut self) -> (i128, i128) {
        let (a, b) = (self.edge(), self.edge());
        (a.min(b), a.max(b))
    }

    /// Ein Punkt in `lo..=hi`: die Enden, die Null, wenn sie darin liegt,
    /// oder ein Wert dazwischen.
    fn within(&mut self, lo: i128, hi: i128) -> i128 {
        match self.next() % 4 {
            0 => lo,
            1 => hi,
            2 if lo <= 0 && 0 <= hi => 0,
            _ => lo + (self.next() as i128).rem_euclid(hi - lo + 1),
        }
    }
}

fn contains(i: Interval, v: i128) -> bool {
    match i {
        Interval::Int { lo, hi } => lo <= v && v <= hi,
        Interval::Top => true,
        _ => false,
    }
}

/// SYN-023: `+ - * / %`, Negation und Betrag ueber Intervalle an den
/// Grenzen: Liegen beide Operanden darin, liegt das exakte Ergebnis im
/// abstrakten — oder die Operation faultet (Divisor null).
#[test]
fn integer_intervals_contain_every_exact_result() {
    type Op = (&'static str, fn(Interval, Interval) -> Interval, fn(i128, i128) -> Option<i128>);
    let ops: [Op; 5] = [
        ("+", |a, b| a + b, |x, y| x.checked_add(y)),
        ("-", |a, b| a - b, |x, y| x.checked_sub(y)),
        ("*", |a, b| a * b, |x, y| x.checked_mul(y)),
        ("/", |a, b| a / b, |x, y| x.checked_div(y)),
        ("%", |a, b| a % b, |x, y| x.checked_rem(y)),
    ];
    let mut r = Rng(0x2026_1005_0023);
    let mut checked = 0;
    for _ in 0..20_000 {
        let ((al, ah), (bl, bh)) = (r.interval(), r.interval());
        let (a, b) = (Interval::Int { lo: al, hi: ah }, Interval::Int { lo: bl, hi: bh });
        for (name, abstract_op, exact) in &ops {
            let i = abstract_op(a, b);
            for _ in 0..4 {
                let (x, y) = (r.within(al, ah), r.within(bl, bh));
                let Some(v) = exact(x, y) else { continue };
                assert!(contains(i, v), "{x} {name} {y} = {v} liegt nicht in {i:?} ({a:?} {name} {b:?})");
                checked += 1;
            }
        }
        let x = r.within(al, ah);
        assert!(contains(-a, -x), "-{x} liegt nicht in {:?}", -a);
        assert!(contains(a.abs(), x.abs()), "|{x}| liegt nicht in {:?}", a.abs());
    }
    assert!(checked > 100_000, "nur {checked} Punkte geprueft");
}

/// SYN-023: `meet`, `join` und `without` erschoepfend auf kleinen
/// Bereichen: `meet` ist der Schnitt, `join` umfasst beide, `without`
/// nimmt hoechstens den genannten Punkt weg.
#[test]
fn meet_join_and_without_are_exact_on_small_ranges() {
    let all: Vec<Interval> =
        (-3..=3).flat_map(|lo| (lo..=3).map(move |hi| Interval::Int { lo, hi })).chain([Interval::Bottom]).collect();
    for &a in &all {
        for &b in &all {
            let (m, j) = (a.meet(b), Intervals::join(&a, &b));
            for x in -4..=4 {
                assert_eq!(contains(m, x), contains(a, x) && contains(b, x), "{x} in {a:?} meet {b:?} = {m:?}");
                if contains(a, x) || contains(b, x) {
                    assert!(contains(j, x), "{x} fehlt in {a:?} join {b:?} = {j:?}");
                }
            }
        }
        for c in -4..=4 {
            let w = a.without(c);
            for x in -4..=4 {
                assert_eq!(contains(w, x), contains(a, x) && (x != c || !is_end(a, c)), "{x}: {a:?} ohne {c} = {w:?}");
            }
        }
    }
}

/// Ist `c` eine Grenze von `a`? Nur ein Randpunkt laesst sich abschneiden.
fn is_end(a: Interval, c: i128) -> bool {
    matches!(a, Interval::Int { lo, hi } if lo == c || hi == c)
}
