//! Erschoepfender Test der korrekt gerundeten `f32`-Funktionen (4.2, 13.8).
//!
//! Laeuft nur auf Verlangen, im Release-Profil und eine Funktion nach der
//! anderen, auf allen Kernen bis auf einen; alle zusammen brauchen auf fuenf
//! Kernen etwa 40 Minuten:
//!
//! ```text
//! cargo test -p libtaktm --release --test exhaustive -- --ignored --test-threads=1
//! ```
//!
//! **Die einstelligen Funktionen ueber alle 2^32 Argumente.** Orakel ist die
//! `f64`-Funktion der Plattform. Liegt ihr Wert um mehr als 2^-48 relativ —
//! 16 ulp von `f64` — von jeder Rundungsgrenze von `f32` entfernt, bestimmt
//! er das korrekt gerundete Ergebnis; eine Plattformbibliothek, die in `f64`
//! um mehr als 16 ulp irrt, waere kaputt. Naeher an einer Grenze entscheidet
//! die unabhaengige Referenz aus `tools/libtaktm.py`: Solche Argumente
//! muessen in `hard_f32.txt` stehen, das `reference.rs` bei jedem Lauf
//! prueft. So ist jedes Argument entweder vom Orakel oder von der Referenz
//! belegt.
//!
//! **`atan2` und `pow` ueber 10^8 Paare** aus einem Generator mit festem
//! Startwert, gegen dasselbe Orakel — 2^64 Paare sind nicht erschoepfend zu
//! pruefen.
//!
//! Fehlt ein naher Fall in `hard_f32.txt`, schreibt der Test die Eingaben
//! nach `$CARGO_TARGET_TMPDIR/hard_f32_<funktion>.txt`; die Referenz macht
//! daraus Vektorzeilen:
//! `python tools/libtaktm.py round < … >> crates/libtaktm/tests/hard_f32.txt`.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

mod common;

/// Die Breite der Fehlerschranke des Orakels, relativ.
const MARGIN: f64 = 1.0 / (1u64 << 48) as f64;

/// Was das Orakel ueber ein Argument sagt.
enum Verdict {
    /// Das korrekt gerundete Bitmuster, oder NaN.
    Decided(Option<u32>),
    /// Zu nahe an einer Rundungsgrenze.
    Near,
}

fn oracle(p: f64) -> Verdict {
    if p.is_nan() {
        return Verdict::Decided(None);
    }
    let lo = (p * (1.0 - MARGIN)) as f32;
    let hi = (p * (1.0 + MARGIN)) as f32;
    if lo.to_bits() == hi.to_bits() { Verdict::Decided(Some(lo.to_bits())) } else { Verdict::Near }
}

/// Ergebnis eines Laufs: Abweichungen und nahe Faelle.
#[derive(Default)]
struct Outcome {
    wrong: Vec<String>,
    near: Vec<Vec<u32>>,
}

/// Verteilt `count` Aufgaben auf alle Kerne bis auf einen, der fuer anderes
/// frei bleibt; `job(i)` prueft Aufgabe i.
fn run(count: u64, job: impl Fn(u64, &mut Outcome) + Sync) -> Outcome {
    let next = AtomicU64::new(0);
    let all = Mutex::new(Outcome::default());
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).saturating_sub(1).max(1);
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                let mut local = Outcome::default();
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= count {
                        break;
                    }
                    job(i, &mut local);
                }
                let mut all = all.lock().expect("kein Faden ist gestorben");
                all.wrong.extend(local.wrong);
                all.near.extend(local.near);
            });
        }
    });
    all.into_inner().expect("kein Faden ist gestorben")
}

/// Prueft ein Argument (oder Paar) und vermerkt Abweichung oder Naehe.
fn judge(name: &str, args: &[u32], ours: f32, theirs: f64, out: &mut Outcome) {
    match oracle(theirs) {
        Verdict::Decided(None) if ours.is_nan() => {}
        Verdict::Decided(Some(bits)) if ours.to_bits() == bits => {}
        Verdict::Decided(want) => {
            if out.wrong.len() < 20 {
                let args: Vec<String> = args.iter().map(|a| format!("{a:08x}")).collect();
                let want = want.map_or("NaN".to_string(), |b| format!("{b:08x}"));
                out.wrong.push(format!("f32 {name}: {} -> {want}, erhalten {:08x}", args.join(" "), ours.to_bits()));
            }
        }
        Verdict::Near => out.near.push(args.to_vec()),
    }
}

/// Die nahen Faelle muessen in `hard_f32.txt` stehen, mit unserem Ergebnis.
fn settle(name: &str, outcome: Outcome) {
    assert!(outcome.wrong.is_empty(), "{name}: Abweichungen vom Orakel:\n{}", outcome.wrong.join("\n"));
    let hard: HashMap<Vec<u64>, common::Vector> = common::load("hard_f32.txt")
        .into_iter()
        .filter(|v| !v.f64 && v.fun == name)
        .map(|v| (v.args.clone(), v))
        .collect();
    let mut missing = Vec::new();
    for args in &outcome.near {
        let key: Vec<u64> = args.iter().map(|a| u64::from(*a)).collect();
        match hard.get(&key) {
            Some(v) => assert_eq!(common::apply(v), Some(v.want), "{}", v.text),
            None => {
                let text: Vec<String> = args.iter().map(|a| format!("{a:08x}")).collect();
                missing.push(format!("f32 {name}: {}", text.join(" ")));
            }
        }
    }
    if !missing.is_empty() {
        let path = format!("{}/hard_f32_{name}.txt", env!("CARGO_TARGET_TMPDIR"));
        std::fs::write(&path, missing.join("\n") + "\n").expect("Zielverzeichnis beschreibbar");
        panic!(
            "{name}: {} nahe Faelle fehlen in hard_f32.txt, Eingaben in {path}; \
             `python tools/libtaktm.py round < {path} >> crates/libtaktm/tests/hard_f32.txt`",
            missing.len()
        );
    }
    println!("{name}: {} nahe Faelle, alle in hard_f32.txt", outcome.near.len());
}

/// Alle 2^32 Argumente einer einstelligen Funktion.
fn unary(name: &str, ours: fn(f32) -> f32, theirs: fn(f64) -> f64) {
    const CHUNK: u64 = 1 << 20;
    let outcome = run((1u64 << 32) / CHUNK, |i, out| {
        for bits in (i * CHUNK)..((i + 1) * CHUNK) {
            let bits = bits as u32;
            let x = f32::from_bits(bits);
            judge(name, &[bits], ours(x), theirs(f64::from(x)), out);
        }
    });
    settle(name, outcome);
}

/// xorshift64*, deterministisch.
fn next(state: &mut u64) -> u64 {
    *state ^= *state >> 12;
    *state ^= *state << 25;
    *state ^= *state >> 27;
    state.wrapping_mul(0x2545_F491_4F6C_DD1D)
}

/// Ein `f32` mit zufaelliger Mantisse und Exponent in `lo..=hi`.
fn ranged(state: &mut u64, lo: i32, hi: i32) -> u32 {
    let r = next(state);
    let e = lo + (r % (hi - lo + 1) as u64) as i32;
    ((r >> 32) as u32 & 0x8000_0000) | (((e + 127) as u32) << 23) | ((r >> 8) as u32 & 0x7f_ffff)
}

/// 10^8 Paare: die Haelfte rohe Bitmuster (Sonderwerte, Extreme), die
/// andere aus dem Bereich, in dem die Funktion nicht ueberlaeuft.
fn binary(name: &str, ours: fn(f32, f32) -> f32, theirs: fn(f64, f64) -> f64, near: fn(&mut u64) -> (u32, u32)) {
    const CHUNK: u64 = 1 << 16;
    const PAIRS: u64 = 100_000_000;
    let outcome = run(PAIRS.div_ceil(CHUNK), |i, out| {
        let mut state = 0x2026_1001 ^ (i + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        for k in 0..CHUNK {
            let (a, b) = if k % 2 == 0 {
                let r = next(&mut state);
                (r as u32, (r >> 32) as u32)
            } else {
                near(&mut state)
            };
            let (x, y) = (f32::from_bits(a), f32::from_bits(b));
            judge(name, &[a, b], ours(x, y), theirs(f64::from(x), f64::from(y)), out);
        }
    });
    settle(name, outcome);
}

#[test]
#[ignore = "erschoepfend, etwa vier Minuten im Release-Profil"]
fn exhaustive_exp() {
    unary("exp", libtaktm::exp_f32, f64::exp);
}

#[test]
#[ignore = "erschoepfend, etwa vier Minuten im Release-Profil"]
fn exhaustive_log() {
    unary("log", libtaktm::log_f32, f64::ln);
}

#[test]
#[ignore = "erschoepfend, etwa vier Minuten im Release-Profil"]
fn exhaustive_sin() {
    unary("sin", libtaktm::sin_f32, f64::sin);
}

#[test]
#[ignore = "erschoepfend, etwa vier Minuten im Release-Profil"]
fn exhaustive_cos() {
    unary("cos", libtaktm::cos_f32, f64::cos);
}

#[test]
#[ignore = "erschoepfend, etwa vier Minuten im Release-Profil"]
fn exhaustive_tan() {
    unary("tan", libtaktm::tan_f32, f64::tan);
}

#[test]
#[ignore = "erschoepfend, etwa vier Minuten im Release-Profil"]
fn exhaustive_asin() {
    unary("asin", libtaktm::asin_f32, f64::asin);
}

#[test]
#[ignore = "erschoepfend, etwa vier Minuten im Release-Profil"]
fn exhaustive_acos() {
    unary("acos", libtaktm::acos_f32, f64::acos);
}

#[test]
#[ignore = "erschoepfend, etwa vier Minuten im Release-Profil"]
fn exhaustive_atan() {
    unary("atan", libtaktm::atan_f32, f64::atan);
}

#[test]
#[ignore = "10^8 Paare, etwa eine Minute im Release-Profil"]
fn pairs_atan2() {
    binary("atan2", libtaktm::atan2_f32, f64::atan2, |s| {
        let a = ranged(s, -40, 40);
        (a, ranged(s, -40, 40))
    });
}

#[test]
#[ignore = "10^8 Paare, etwa eine Minute im Release-Profil"]
fn pairs_pow() {
    binary("pow", libtaktm::pow_f32, f64::powf, |s| {
        let x = ranged(s, -8, 8) & 0x7fff_ffff;
        (x, ranged(s, -4, 5))
    });
}
