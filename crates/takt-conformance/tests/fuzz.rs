//! Der Fuzzer der Abnahme (13.8): erzeugte Programme statt geschriebener.
//!
//! **Warum ein Fuzzer, wenn der Korpus schon stimmt.** Der Korpus ist
//! geschrieben worden, um Sprachmerkmale zu zeigen — er trifft die Faelle,
//! an die jemand gedacht hat. Der Fuzzer trifft die anderen: Ausdruecke,
//! deren Klammerung niemand so schreiben wuerde, Zahlen an den Raendern
//! der Darstellung, Ketten von Operationen, die sich gegenseitig
//! aufheben.
//!
//! **Was er erzeugt, ist absichtlich eng.** Jedes Programm hat eine
//! Maschine, einen rechnenden Zustand und eine Zuweisung — nur der
//! *Ausdruck* wird gewuerfelt. Damit ist jede Abweichung auf einen Ausdruck
//! zurueckzufuehren, statt auf ein Zusammenspiel, das erst zu entwirren
//! waere. Die Variablen aendern sich jeden Tick, damit der Vergleich
//! Laufzeitarithmetik prueft und nicht die Konstantenfaltung.
//!
//! **Faults gehoeren dazu** (13.1 c, 4.1). Ueberlauf, Division durch null,
//! Schiebebetraege, Umwandlungen und nicht endliche Ergebnisse faulten; der
//! Fault fuehrt in einen Ruhezustand, der nach einem Tick zurueckkehrt, und
//! [`compare`] haelt Art und Tick jedes Faults gegeneinander.
//!
//! Der Generator ist ein xorshift; der Startwert kommt aus `TAKT_FUZZ_SEED`
//! (dezimal oder `0x…`), die Rundenzahl aus `TAKT_FUZZ_ROUNDS`, sonst fest.
//! Ein Fehlschlag nennt beide: Er ist reproduzierbar, und ein naechtlicher
//! Lauf mit anderem Startwert findet, was der feste nicht trifft.
//!
//! **Mit Eingaben** (M11 Schritt 29a): Ein zweiter Lauf liest eine Variable
//! jeden Tick aus einem Input und faehrt jedes Programm mit einem der
//! erzeugten Laeufe ([`takt_conformance::generated`]) — Grenzen der Range,
//! knapp jenseits, schlechte Qualitaet, Luecken, Spruenge. Der erste Lauf
//! bleibt ohne: Sein fester Startwert bewacht, was er gefunden hat.

use takt_conformance::compare;
use takt_conformance::stimulus::Stimulus;

/// xorshift64*, deterministisch.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }

    fn pick<'a>(&mut self, of: &[&'a str]) -> &'a str {
        of[self.below(of.len() as u64) as usize]
    }
}

/// Ein Blatt: meist eine Variable oder eine kleine Konstante, sonst ein
/// Rand der Darstellung — dort sitzen die Fehler, aber ein Ausdruck aus
/// lauter Raendern faultete nur noch.
fn leaf<'a>(rng: &mut Rng, vars: &[&'a str], small: &[&'a str], edges: &[&'a str]) -> &'a str {
    match rng.below(8) {
        0..=3 => rng.pick(vars),
        4..=6 => rng.pick(small),
        _ => rng.pick(edges),
    }
}

/// Ein Ganzzahlausdruck (`int`, 64 Bit) ueber `n`, `m` und Konstanten bis
/// an `i32` und `i64` (4.1): Arithmetik, Division und Rest, Schiebungen,
/// Bitoperationen, Vergleiche und Umwandlungen.
fn int_expr(rng: &mut Rng, depth: u32) -> String {
    if depth == 0 {
        let edges =
            ["2147483647", "(-2147483647 - 1)", "4294967295", "9223372036854775807", "(-9223372036854775807 - 1)"];
        return leaf(rng, &["n", "m"], &["0", "1", "-1", "3", "7", "100"], &edges).to_string();
    }
    let a = int_expr(rng, depth - 1);
    let b = int_expr(rng, depth - 1);
    match rng.below(17) {
        0 => format!("({a} + {b})"),
        1 => format!("({a} - {b})"),
        2 => format!("({a} * {b})"),
        3 => format!("({a} / {b})"),
        4 => format!("({a} % {b})"),
        5 => format!("min({a}, {b})"),
        6 => format!("max({a}, {b})"),
        7 => format!("({a} if {a} > {b} else {b})"),
        8 => format!("wrapping_add({a}, {b})"),
        9 => format!("({a} << {b})"),
        10 => format!("({a} >> {b})"),
        11 => format!("({a} & {b})"),
        12 => format!("({a} | {b})"),
        13 => format!("({a} ^ {b})"),
        14 => format!("(1 if {a} < {b} else 0)"),
        15 => format!("(({a}) as i32 as int)"),
        _ => format!("abs({a})"),
    }
}

/// Ein Fliesskommaausdruck in der Breite `float` (`f64`) oder `f32`.
///
/// Hier sitzt Satz 9.4.4: Jede Operation ist einzeln gerundet, und eine
/// andere Klammerung ergibt eine andere Zahl. Die Konstanten sind
/// absichtlich unrund — `0.1` ist der Fall, an dem sich eine
/// Dezimalschreibweise verraet —, dazu die Raender: der groesste endliche
/// Wert, der kleinste normale und Subnormale, `-0.0`.
fn float_expr(rng: &mut Rng, depth: u32, f32: bool) -> String {
    if depth == 0 {
        let (vars, edges): (&[&str], &[&str]) = if f32 {
            (&["y", "w"], &["3.4028235e38", "1.1754944e-38", "1e-45", "1e-40", "-0.0"])
        } else {
            (&["x", "z"], &["1e308", "2.2250738585072014e-308", "5e-324", "1e-310", "-0.0"])
        };
        return leaf(rng, vars, &["0.1", "1.0", "3.7", "0.0", "100.0"], edges).to_string();
    }
    let a = float_expr(rng, depth - 1, f32);
    let b = float_expr(rng, depth - 1, f32);
    let width = if f32 { "f32" } else { "float" };
    match rng.below(11) {
        0 => format!("({a} + {b})"),
        1 => format!("({a} - {b})"),
        2 => format!("({a} * {b})"),
        3 => format!("({a} / {b})"),
        4 => format!("min({a}, {b})"),
        5 => format!("max({a}, {b})"),
        6 => format!("({a} if {a} > {b} else {b})"),
        7 => format!("sqrt({a})"),
        8 => format!("abs({a})"),
        9 => format!("(-{a})"),
        _ => format!("(floor({a}) as {width})"),
    }
}

/// Die Art eines Programms: welche Breite der Ausdruck hat.
#[derive(Clone, Copy, Debug)]
enum Kind {
    Int,
    Float,
    F32,
}

/// Ein Programm um einen Ausdruck: Die Variablen laufen jeden Tick weiter,
/// auch im Ruhezustand, in den ein Fault fuehrt; nach einem Tick rechnet
/// die Maschine wieder.
fn program(kind: Kind, expr: &str) -> String {
    let (ty, safe, vars, step) = frame(kind);
    assemble(ty, safe, "", vars, step, expr)
}

/// Wie [`program`], aber die erste Variable kommt jeden Tick aus einem
/// Input derselben Range: Ein ungueltiger faultet beim Lesen (3.5), ein
/// guter traegt seine Grenze in den Ausdruck.
fn program_with_input(kind: Kind, expr: &str) -> String {
    let (ty, safe, vars, step) = frame(kind);
    let (input, read) = match kind {
        Kind::Int => ("input u : int in 0..100 @ hw(\"in/u\") with max_age = 30 ms\n", "            n = u\n"),
        Kind::Float => ("input v : float in 0.0..100.0 @ hw(\"in/v\") with max_age = 30 ms\n", "            x = v\n"),
        Kind::F32 => ("input g : f32 in 0.0..100.0 @ hw(\"in/g\") with max_age = 30 ms\n", "            y = g\n"),
    };
    let step = step.lines().skip(1).map(|l| format!("{l}\n")).collect::<String>();
    assemble(ty, safe, input, vars, &format!("{read}{step}"), expr)
}

/// Typ, Safe-Wert, Variablen und ihr Fortschritt je Art.
fn frame(kind: Kind) -> (&'static str, &'static str, &'static str, &'static str) {
    match kind {
        Kind::Int => (
            "int",
            "0",
            "    var n : int in 0..100 = 3\n    var m : int in 0..999 = 500\n",
            "            n = (n + 37) % 101\n            m = (m * 7 + 3) % 1000\n",
        ),
        Kind::Float => (
            "float",
            "0.0",
            "    var x : float in 0.0..100.0 = 3.7\n    var z : float in 0.0..100.0 = 0.5\n",
            "            x = (x + 13.7) if x < 80.0 else (x - 80.0)\n            z = (z * 1.37) if z < 50.0 else (z - 49.0)\n",
        ),
        Kind::F32 => (
            "f32",
            "0.0",
            "    var y : f32 in 0.0..100.0 = 1.5\n    var w : f32 in 0.0..100.0 = 0.5\n",
            "            y = (y + 13.7) if y < 80.0 else (y - 80.0)\n            w = (w * 1.37) if w < 50.0 else (w - 49.0)\n",
        ),
    }
}

fn assemble(ty: &str, safe: &str, input: &str, vars: &str, step: &str, expr: &str) -> String {
    format!(
        "system:\n    language = 1\n    tick     = 10 ms\n\n{input}\
         output r : {ty} @ hw(\"ui/r\") with safe = {safe}\n\n\
         machine f:\n    fault -> REST\n{vars}\n    initial RUN\n\
         \x20   state RUN:\n        loop:\n{step}            r = {expr}\n\
         \x20   state REST:\n        loop:\n{step}        after 10 ms: -> RUN\n"
    )
}

/// Uebersetzt ein Programm; `None`, wenn das Sema es ablehnt.
///
/// Eine Ablehnung ist die richtige Antwort, wenn ein Fehler feststeht —
/// eine Division durch die Konstante null, ein Schiebebetrag, der nie
/// passt (3.4) —; dass das nicht der Normalfall ist, sichert die Quote.
fn compile(src: &str) -> Option<takt_mir::Program> {
    let o = takt_sema::Options {
        policy: takt_diag::Policy::default(),
        build: takt_sema::Build::Sim,
        profile: None,
        ..Default::default()
    };
    let out = takt_sema::compile(src, &o);
    if out.diagnostics.iter().any(|d| d.is_error()) {
        return None;
    }
    out.program
}

const TICKS: u64 = 12;

/// Eine Einstellung aus der Umgebung: dezimal oder `0x…`, sonst `default`.
fn setting(name: &str, default: u64) -> u64 {
    let Ok(text) = std::env::var(name) else { return default };
    let parsed = match text.strip_prefix("0x") {
        Some(hex) => u64::from_str_radix(hex, 16),
        None => text.parse(),
    };
    parsed.unwrap_or_else(|e| panic!("{name}={text}: {e}"))
}

/// **Der Fuzzer der Abnahme.** Erzeugte Programme laufen in beiden
/// Implementierungen und liefern dieselben Outputs und Faults.
#[test]
fn generated_programs_agree() {
    let Some(clang) = common::clang() else { return };
    let (seed, rounds) = (setting("TAKT_FUZZ_SEED", 0x2026_0912), setting("TAKT_FUZZ_ROUNDS", 150));
    eprintln!("TAKT_FUZZ_SEED={seed:#x} TAKT_FUZZ_ROUNDS={rounds}");
    let mut rng = Rng(seed);
    let (mut built, mut rejected, mut faulted) = (0u64, Vec::new(), 0u64);
    let mut errors = Vec::new();

    for round in 0..rounds {
        // Ganzzahl, `f64` und `f32` im Wechsel: Im Fliesskomma sitzt Satz
        // 9.4.4, in der Ganzzahl die Faults aus 4.1.
        let kind = [Kind::Int, Kind::Float, Kind::F32][(round % 3) as usize];
        let depth = 2 + (round % 4) as u32;
        let expr = match kind {
            Kind::Int => int_expr(&mut rng, depth),
            Kind::Float => float_expr(&mut rng, depth, false),
            Kind::F32 => float_expr(&mut rng, depth, true),
        };
        let src = program(kind, &expr);
        let Some(p) = compile(&src) else {
            rejected.push(format!("{kind:?} `{expr}`"));
            continue;
        };
        let native = match crate::common::run_native_all(&clang, &p, &format!("fuzz{round}"), TICKS) {
            Ok(t) => t,
            Err(e) => {
                errors.push(format!("Runde {round}, `{expr}`: kein nativer Lauf:\n{e}"));
                continue;
            }
        };
        let options = takt_interp::RunOptions { ticks: TICKS, profile: None, order_seed: None, ..Default::default() };
        let interpreted = match takt_interp::run(&p, &takt_interp::Trace::default(), &options) {
            Ok(r) => r.trace.render(),
            Err(e) => {
                errors.push(format!("Runde {round}, `{expr}`: der Interpreter bricht ab: {e:?}"));
                continue;
            }
        };
        built += 1;
        faulted += u64::from(interpreted.contains(" fault "));
        let widened = takt_conformance::run::widen_f32(&interpreted, &takt_conformance::run::f32_outputs(&p));
        let diffs = compare(&widened, &native);
        if !diffs.is_empty() {
            errors.push(format!(
                "Runde {round}, {kind:?} `{expr}`: {}\n--- Interpreter ---\n{interpreted}--- nativ ---\n{native}",
                diffs.iter().take(3).map(|d| format!("  {d}")).collect::<Vec<_>>().join("\n")
            ));
        }
    }

    let tag = format!("TAKT_FUZZ_SEED={seed:#x} TAKT_FUZZ_ROUNDS={rounds}");
    eprintln!("{tag}: {built} verglichen, davon {faulted} mit Fault, {} abgelehnt", rejected.len());
    assert!(errors.is_empty(), "{tag}: {} Abweichungen:\n{}", errors.len(), errors.join("\n\n"));
    // Eine Ablehnung ist die Ausnahme: Abgelehnt werden Ausdruecke mit einem
    // feststehenden Fault in einem konstanten Teil und Vergleiche, die eine
    // Konstante in `float` gegen `f32` halten (4.2). Laege die Quote hoeher,
    // wuerfelte der Generator am Sema vorbei, wie die Haelfte in
    // Fliesskomma, die bis KON1-017 an der Einrueckung ihres Rahmens
    // scheiterte.
    assert!(
        built * 5 >= rounds * 3,
        "{tag}: nur {built} von {rounds} Programmen kamen zum Vergleich, abgelehnt etwa {:?}",
        &rejected[..rejected.len().min(5)]
    );
    // Faults gehoeren dazu (13.1 c); ohne sie pruefte der Vergleich ihre
    // Pfade nicht.
    assert!(faulted * 10 >= built, "{tag}: nur {faulted} von {built} Programmen faulten");
}

/// **Der Fuzzer mit Eingaben** (M11 Schritt 29a): Erzeugte Ausdruecke ueber
/// einem Input laufen mit einem der erzeugten Laeufe in beiden
/// Implementierungen und liefern dieselben Outputs und Faults. Der Startwert
/// kommt aus `TAKT_FUZZ_INPUT_SEED`, die Rundenzahl aus
/// `TAKT_FUZZ_INPUT_ROUNDS`.
#[test]
fn generated_programs_with_inputs_agree() {
    let Some(clang) = common::clang() else { return };
    let (seed, rounds) = (setting("TAKT_FUZZ_INPUT_SEED", 0x2026_1010), setting("TAKT_FUZZ_INPUT_ROUNDS", 60));
    let tag = format!("TAKT_FUZZ_INPUT_SEED={seed:#x} TAKT_FUZZ_INPUT_ROUNDS={rounds}");
    eprintln!("{tag}");
    let mut rng = Rng(seed);
    let (mut built, mut rejected, mut faulted) = (0u64, 0u64, 0u64);
    let mut errors = Vec::new();
    for round in 0..rounds {
        let kind = [Kind::Int, Kind::Float, Kind::F32][(round % 3) as usize];
        let depth = 2 + (round % 3) as u32;
        let expr = match kind {
            Kind::Int => int_expr(&mut rng, depth),
            Kind::Float => float_expr(&mut rng, depth, false),
            Kind::F32 => float_expr(&mut rng, depth, true),
        };
        let Some(p) = compile(&program_with_input(kind, &expr)) else {
            rejected += 1;
            continue;
        };
        // Je Runde einer der erzeugten Laeufe, im Wechsel.
        let cases = takt_conformance::generated::cases(&p);
        let case = &cases[(round / 3) as usize % cases.len()];
        let stimulus = takt_interp::Trace::parse(&case.stimulus).expect("Stimulus");
        let inputs = Stimulus::from_trace(&stimulus).expect("Eingaben");
        let label = format!("Runde {round}, {kind:?} `{expr}` ({})", case.label);
        let native = match common::run_native_all_with(&clang, &p, &format!("fuzzi{round}"), case.ticks, &inputs) {
            Ok(t) => t,
            Err(e) => {
                errors.push(format!("{label}: kein nativer Lauf:\n{e}"));
                continue;
            }
        };
        let options = takt_interp::RunOptions { ticks: case.ticks, ..Default::default() };
        let interpreted = match takt_interp::run(&p, &stimulus, &options) {
            Ok(r) => r.trace.render(),
            Err(e) => {
                errors.push(format!("{label}: der Interpreter bricht ab: {e:?}"));
                continue;
            }
        };
        built += 1;
        faulted += u64::from(interpreted.contains(" fault "));
        let widened = takt_conformance::run::widen_f32(&interpreted, &takt_conformance::run::f32_outputs(&p));
        let diffs = compare(&widened, &native);
        if !diffs.is_empty() {
            let list: Vec<String> = diffs.iter().take(3).map(|d| format!("  {d}")).collect();
            errors.push(format!("{label}:\n{}", list.join("\n")));
        }
    }
    eprintln!("{tag}: {built} verglichen, davon {faulted} mit Fault, {rejected} abgelehnt");
    assert!(errors.is_empty(), "{tag}: {} Abweichungen:\n{}", errors.len(), errors.join("\n\n"));
    assert!(built * 5 >= rounds * 3, "{tag}: nur {built} von {rounds} Programmen kamen zum Vergleich");
    // Ungueltige Eingaben faulten beim Lesen; ohne Fault pruefte der Lauf
    // ihren Pfad nicht.
    assert!(faulted * 2 >= built, "{tag}: nur {faulted} von {built} Programmen faulten");
}

/// **Ein Schieben um mehr als 31 Bit bleibt 64 Bit breit** (4.1, Lemma
/// 3.4; gefunden vom Fuzzer, Runde 117 mit dem festen Startwert). Passen
/// Ergebnis und Operanden in `i32`, rechnet der Codegen schmal; ein
/// Schiebebetrag ab 32 ist in `i32` aber Gift, waehrend `int` ihn kennt:
/// `m >> 50` ist fuer `m` in `0..999` null und `3 >> 0` drei, und `n << 40
/// >> 40` gibt `n` zurueck.
#[test]
fn a_shift_by_more_than_31_bits_is_not_narrowed() {
    let Some(clang) = common::clang() else { return };
    // Der Schritt rechnet `n` vor `r`: In Tick 0 ist `n` schon 40, `m` 503.
    for (expr, want) in [("(3 >> (m >> 50))", "3"), ("((n << 40) >> 40)", "40"), ("((m >> 40) + n)", "40")] {
        let src = program(Kind::Int, expr);
        let p = compile(&src).unwrap_or_else(|| panic!("`{expr}` uebersetzt nicht"));
        let interpreted = takt_interp::run(
            &p,
            &takt_interp::Trace::default(),
            &takt_interp::RunOptions { ticks: 1, ..Default::default() },
        )
        .expect("Lauf")
        .trace
        .render();
        assert!(
            interpreted.contains(&format!(
                "t=0 out r {want}
"
            )),
            "`{expr}`:
{interpreted}"
        );
        let native = common::run_native_all(&clang, &p, "shift_narrow", 1).unwrap_or_else(|e| panic!("{e}"));
        let diffs = compare(&interpreted, &native);
        assert!(
            diffs.is_empty(),
            "`{expr}`: {diffs:?}
--- nativ ---
{native}"
        );
    }
}

/// **`abs` des kleinsten Werts laeuft ueber** (4.1, FB-465): `abs` ist
/// `-x` fuer negative `x`, und `-MIN` passt in keine Breite. Der Codegen
/// rief `llvm.abs` ohne Pruefung und lieferte `MIN`, der Interpreter
/// faultete. In Tick 0 ist `n` 40.
#[test]
fn abs_of_the_smallest_value_overflows() {
    let Some(clang) = common::clang() else { return };
    for expr in ["abs(n - 40 - 9223372036854775807 - 1)", "(abs((n - 168) as i8) as int)"] {
        let p = compile(&program(Kind::Int, expr)).unwrap_or_else(|| panic!("`{expr}` uebersetzt nicht"));
        let interpreted = takt_interp::run(
            &p,
            &takt_interp::Trace::default(),
            &takt_interp::RunOptions { ticks: 1, ..Default::default() },
        )
        .expect("Lauf")
        .trace
        .render();
        assert!(interpreted.contains("Overflow"), "`{expr}`:\n{interpreted}");
        let native = common::run_native_all(&clang, &p, "abs_min", 1).unwrap_or_else(|e| panic!("{e}"));
        let diffs = compare(&interpreted, &native);
        assert!(diffs.is_empty(), "`{expr}`: {diffs:?}\n--- nativ ---\n{native}");
    }
}

mod common;
