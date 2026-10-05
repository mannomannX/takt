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
//!
//! **Das Protokoll (INT-028).** Ein bestandener Lauf traegt sich mit dem
//! Fingerabdruck der Implementierung (`elem.rs`, `big.rs`, `table.rs`),
//! Datum und Commit in `exhaustive_runs.txt` ein. Im gewoehnlichen Lauf
//! prueft eine Stichprobe jeder Funktion gegen dasselbe Orakel, und die
//! Annahme des Orakels — die `f64`-Bibliothek der Plattform irrt um weniger
//! als 16 ulp — wird an den Zufallsvektoren der Referenz gemessen.

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
/// Ein vollstaendiger Lauf (`record`) traegt sich danach in die Ratsche ein.
fn settle(name: &str, outcome: Outcome) {
    settle_sample(name, outcome, true);
}

fn settle_sample(name: &str, outcome: Outcome, record: bool) {
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
    if record {
        record_run(name, outcome.near.len());
    }
}

/// Die Dateien, deren Inhalt die Ergebnisse der Funktionen bestimmt.
const IMPLEMENTATION: [&str; 3] = ["src/elem.rs", "src/big.rs", "src/table.rs"];

fn runs_path() -> String {
    format!("{}/tests/exhaustive_runs.txt", env!("CARGO_MANIFEST_DIR"))
}

/// FNV-1a ueber die Implementierung, Zeilenenden vereinheitlicht: Derselbe
/// Stand gibt unter Windows und Linux denselben Abdruck.
fn fingerprint() -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for file in IMPLEMENTATION {
        let path = format!("{}/{file}", env!("CARGO_MANIFEST_DIR"));
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        for b in file.bytes().chain(text.bytes().filter(|b| *b != b'\r')) {
            h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
    }
    format!("{h:016x}")
}

/// Das Datum `JJJJ-MM-TT` (UTC) aus der Systemzeit.
fn today() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    // Tage seit 1970 nach Jahr, Monat, Tag (proleptisch gregorianisch).
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Der Commit, auf dem der Lauf stand; `?` ohne git.
fn commit() -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map_or_else(|| "?".into(), |o| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

/// Traegt einen bestandenen Lauf ein; ein frueherer derselben Funktion
/// faellt heraus.
fn record_run(name: &str, near: usize) {
    static LOCK: Mutex<()> = Mutex::new(());
    let _guard = LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let path = runs_path();
    let old = std::fs::read_to_string(&path).unwrap_or_default();
    let mut lines: Vec<String> =
        old.lines().filter(|l| l.split_whitespace().next() != Some(name)).map(str::to_string).collect();
    lines.push(format!("{name} {} {near} {} {}", fingerprint(), today(), commit()));
    lines.sort();
    let head = "# Erschoepfende Laeufe (exhaustive.rs): Funktion, Fingerabdruck der Implementierung, \
                nahe Faelle, Datum, Commit.";
    std::fs::write(&path, format!("{head}\n{}\n", lines.join("\n"))).expect("Ratsche beschreibbar");
}

/// INT-028: Die Annahme des Orakels, gemessen: Die `f64`-Funktionen der
/// Plattform liegen an den Zufallsvektoren der Referenz um hoechstens 16 ulp
/// neben dem korrekt gerundeten Ergebnis — im Bereich der `f32`-Argumente,
/// den das Orakel bedient.
#[test]
fn the_platform_oracle_holds_its_margin() {
    let ulps = |a: f64, b: f64| (a.to_bits() as i64).wrapping_sub(b.to_bits() as i64).unsigned_abs();
    let in_range = |x: f64| x == 0.0 || (x.abs() <= f64::from(f32::MAX) && x.abs() >= 1e-45);
    let mut worst = (0u64, String::new());
    for v in common::load("random.txt").iter().filter(|v| v.f64) {
        let a = |i: usize| f64::from_bits(v.args.get(i).copied().unwrap_or(0));
        let theirs = match v.fun.as_str() {
            "exp" => a(0).exp(),
            "log" => a(0).ln(),
            "sin" => a(0).sin(),
            "cos" => a(0).cos(),
            "tan" => a(0).tan(),
            "asin" => a(0).asin(),
            "acos" => a(0).acos(),
            "atan" => a(0).atan(),
            "atan2" => a(0).atan2(a(1)),
            "pow" => a(0).powf(a(1)),
            other => panic!("unbekannte Funktion `{other}`"),
        };
        let want = f64::from_bits(v.want);
        if !v.args.iter().all(|b| in_range(f64::from_bits(*b))) || !want.is_finite() || !in_range(want) {
            continue;
        }
        let d = ulps(theirs, want);
        if d > worst.0 {
            worst = (d, v.text.clone());
        }
    }
    assert!(worst.0 <= 16, "die Plattform irrt um {} ulp: {}", worst.0, worst.1);
}

/// INT-028: Eine Stichprobe jeder Funktion gegen das Orakel, im
/// gewoehnlichen Lauf — jedes 65 537. Argument und 2^14 Paare. Nahe Faelle
/// muessen wie im vollen Lauf in `hard_f32.txt` stehen.
#[test]
fn a_sample_of_every_function_agrees_with_the_oracle() {
    type Unary = (&'static str, fn(f32) -> f32, fn(f64) -> f64);
    type Pair = (&'static str, fn(f32, f32) -> f32, fn(f64, f64) -> f64, fn(&mut u64) -> (u32, u32));
    let unary: [Unary; 8] = [
        ("exp", libtaktm::exp_f32, f64::exp),
        ("log", libtaktm::log_f32, f64::ln),
        ("sin", libtaktm::sin_f32, f64::sin),
        ("cos", libtaktm::cos_f32, f64::cos),
        ("tan", libtaktm::tan_f32, f64::tan),
        ("asin", libtaktm::asin_f32, f64::asin),
        ("acos", libtaktm::acos_f32, f64::acos),
        ("atan", libtaktm::atan_f32, f64::atan),
    ];
    for (name, ours, theirs) in unary {
        let mut out = Outcome::default();
        for bits in (0..=u32::MAX).step_by(65_537) {
            let x = f32::from_bits(bits);
            judge(name, &[bits], ours(x), theirs(f64::from(x)), &mut out);
        }
        settle_sample(name, out, false);
    }
    let pairs: [Pair; 2] =
        [("atan2", libtaktm::atan2_f32, f64::atan2, atan2_near), ("pow", libtaktm::pow_f32, f64::powf, pow_near)];
    for (name, ours, theirs, near) in pairs {
        let mut out = Outcome::default();
        let mut state = 0x2026_2808;
        for k in 0..(1u32 << 14) {
            let (a, b) = if k % 2 == 0 {
                let r = next(&mut state);
                (r as u32, (r >> 32) as u32)
            } else {
                near(&mut state)
            };
            let (x, y) = (f32::from_bits(a), f32::from_bits(b));
            judge(name, &[a, b], ours(x, y), theirs(f64::from(x), f64::from(y)), &mut out);
        }
        settle_sample(name, out, false);
    }
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

/// Paare fuer `atan2` aus dem Bereich ohne Ueber- und Unterlauf.
fn atan2_near(s: &mut u64) -> (u32, u32) {
    let a = ranged(s, -40, 40);
    (a, ranged(s, -40, 40))
}

/// Paare fuer `pow`: positive Basen mit beliebigem Exponenten, und jede
/// vierte eine negative Basis mit ganzzahligem Exponenten (INT-028).
fn pow_near(s: &mut u64) -> (u32, u32) {
    let x = ranged(s, -8, 8);
    if next(s) % 4 == 0 {
        let n = (next(s) % 81) as i32 - 40;
        ((x | 0x8000_0000), (n as f32).to_bits())
    } else {
        (x & 0x7fff_ffff, ranged(s, -4, 5))
    }
}

#[test]
#[ignore = "10^8 Paare, etwa eine Minute im Release-Profil"]
fn pairs_atan2() {
    binary("atan2", libtaktm::atan2_f32, f64::atan2, atan2_near);
}

#[test]
#[ignore = "10^8 Paare, etwa eine Minute im Release-Profil"]
fn pairs_pow() {
    binary("pow", libtaktm::pow_f32, f64::powf, pow_near);
}
