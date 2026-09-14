//! Der Fuzzer der Maschinenstruktur (13.8, Satz 9.4.4).
//!
//! **Warum ein zweiter Fuzzer.** `fuzz.rs` wuerfelt Ausdruecke in einem
//! festen Rahmen: eine Maschine, ein Zustand, eine Zuweisung. Damit
//! findet er Fehler in der Arithmetik — und nur dort. Die Konstrukte,
//! die M4 zuletzt dazubekam (`every`, `at`, `->` als Anweisung,
//! Reduktionen, Muster-Guards), liegen *ausserhalb* dieses Rahmens: Sie
//! sind Struktur, nicht Ausdruck.
//!
//! Dieser Fuzzer wuerfelt darum die Struktur — wie viele Zustaende, was
//! in ihren `loop:`-Bloecken steht, wohin die Uebergaenge zeigen. Was
//! dabei entsteht, ist selten ein sinnvolles Programm; es muss nur
//! *gueltig* sein, und beide Implementierungen muessen dasselbe daraus
//! machen.
//!
//! **Warum das Zusammenspiel der Punkt ist.** Jedes Konstrukt fuer sich
//! hat seinen Korpustest (`26_samples`, `27_every`, `28_scheduled`). Was
//! keiner davon prueft, ist ihr Zusammentreffen: ein `every` im
//! Zustand, den ein `->` gerade verlaesst; ein `at` in einem
//! Entry-Tick; ein Zaehler, der zurueckgesetzt wird, waehrend er feuert.
//! Genau dort sitzen die Fehler, die ein geschriebener Test nicht trifft.
//!
//! Der Generator ist ein xorshift mit festem Startwert: Ein Fehlschlag
//! ist reproduzierbar, und der Testlauf ist es auch.

use std::fmt::Write as _;

use takt_conformance::compare;
use takt_llvm::toolchain::{Clang, find};

mod common;

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

    fn upto(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

/// Eine Anweisung fuer einen `loop:`-Block.
///
/// Die Auswahl deckt die Konstrukte ab, die M4 zuletzt bekam, und
/// mischt sie mit gewoehnlichen Zuweisungen — ein `every`, das allein
/// steht, trifft das Zusammenspiel nicht.
fn statement(rng: &mut Rng, states: usize, at_ns: &mut u64) -> String {
    match rng.upto(7) {
        // Eine gewoehnliche Zuweisung: der Grundfall, gegen den sich
        // alles andere abhebt.
        0 => format!("            c = c + {}", 1 + rng.upto(3)),
        // `every d:` (5.8) — die Uhr haengt am Ort, und hier steht es im
        // Zustand, also `t_in_state`.
        1 => format!("            every {} ms:\n                c = c + 1", 10 * (1 + rng.upto(4))),
        // Reduktionen ueber ein Feld (8.9).
        2 => {
            let which = ["min()", "max()", "mean()", "rms()"][rng.upto(4) as usize];
            format!("            f = xs.{which}")
        }
        3 => "            n = xs.count".to_string(),
        // `at T:` (9.8) — jeder Zeitpunkt genau einmal, weil zwei
        // gleiche einander ueberschreiben und der Vergleich dann von der
        // Reihenfolge abhinge.
        4 => {
            *at_ns += 10;
            format!("            at {} ms:\n                d = {}", *at_ns, rng.upto(9))
        }
        // Ein `check`, das immer haelt: Er darf den Lauf nicht faulten,
        // sonst vergleicht der Test zwei Abbrueche.
        5 => "            check c >= 0, \"counter\"".to_string(),
        // `-> ZIEL` als Anweisung (11.2), nur wenn es mehr als einen
        // Zustand gibt.
        _ if states > 1 => {
            let to = rng.upto(states as u64);
            format!("            if c > 5:\n                -> S{to}")
        }
        _ => "            c = c + 1".to_string(),
    }
}

/// Ein Programm mit gewuerfelter Struktur.
///
/// Fest ist, was der Vergleich braucht: Outputs, die jeden Tick
/// geschrieben werden, und Ranges, die die Analyse beweisen kann.
/// Gewuerfelt ist die Zahl der Zustaende, ihr Inhalt und ihre
/// Uebergaenge.
fn machine_program(rng: &mut Rng) -> String {
    let states = 1 + rng.upto(3) as usize;
    let mut s = String::new();
    let _ = writeln!(s, "system:");
    let _ = writeln!(s, "    language = 1");
    let _ = writeln!(s, "    tick     = 10 ms\n");
    let _ = writeln!(s, "output c : int in 0..999 @ sim(\"c\")");
    let _ = writeln!(s, "output d : int in 0..9 @ sim(\"d\")");
    let _ = writeln!(s, "output n : int in 0..9 @ sim(\"n\")");
    let _ = writeln!(s, "output f : float @ sim(\"f\")\n");
    let _ = writeln!(s, "machine m:");
    // Ein Feld mit festen Werten: Die Reduktionen brauchen eines, und
    // ein gewuerfeltes waere eine zweite Quelle von Unterschieden.
    let _ = writeln!(s, "    var xs : [4] float = [3.0, -1.0, 4.0, 2.0]");
    let _ = writeln!(s, "    var k : int in 0..999 = 0\n");
    let _ = writeln!(s, "    initial S0\n");
    let mut at_ns = 0u64;
    for i in 0..states {
        let _ = writeln!(s, "    state S{i}:");
        let _ = writeln!(s, "        loop:");
        // Ein bis drei Anweisungen je Zustand.
        for _ in 0..=rng.upto(3) {
            let _ = writeln!(s, "{}", statement(rng, states, &mut at_ns));
        }
        // Ein Uebergang, damit der Zustand nicht endgueltig ist (SC-10);
        // das Ziel ist gewuerfelt, auch auf sich selbst.
        let to = rng.upto(states as u64);
        let _ = writeln!(s, "        after {} ms: -> S{to}\n", 10 * (1 + rng.upto(5)));
    }
    s
}

/// Uebersetzt ein Programm; `None`, wenn das Sema es ablehnt.
///
/// Ablehnungen sind der Normalfall: Der Generator wuerfelt auch
/// Programme, deren Ranges die Analyse nicht beweisen kann (3.4), und
/// eine Ablehnung ist dann die richtige Antwort.
fn compile(src: &str) -> Option<takt_mir::Program> {
    let o = takt_sema::Options { policy: takt_diag::Policy::default(), build: takt_sema::Build::Sim, profile: None };
    let out = takt_sema::compile(src, &o);
    if out.diagnostics.iter().any(|d| d.is_error()) {
        return None;
    }
    out.program
}

const TICKS: u64 = 12;

/// **Der Strukturfuzzer.** Erzeugte Maschinen laufen in beiden
/// Implementierungen und liefern dieselben Outputs (Satz 9.4.4).
#[test]
fn generated_machines_agree() {
    let Clang::At(path) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return;
    };
    let clang = Clang::At(path);
    // Zwei Startwerte sind gelaufen: 0x2026_0914 fand FB-122 in Runde
    // 106, 0xDEAD_BEEF ueber 300 Runden nichts weiter. Der erste bleibt,
    // weil ein Fuzzer, der seinen Fund nicht mehr trifft, ihn nicht mehr
    // bewacht.
    let mut rng = Rng(0x2026_0914);
    let (mut built, mut rejected, mut not_lowered) = (0, 0, 0);
    let mut errors = Vec::new();

    for round in 0..120 {
        let src = machine_program(&mut rng);
        let Some(p) = compile(&src) else {
            rejected += 1;
            continue;
        };
        let machine = match p.machines.first().map(|m| m.name.clone()) {
            Some(m) => m,
            None => continue,
        };
        let name = format!("fuzzm{round}");
        let native = match common::run_native(&clang, &p, &name, &machine, TICKS) {
            Ok(t) => t,
            Err(e) => {
                // Ein Konstrukt, das der Codegen nicht senkt, ist kein
                // Fehlschlag — er meldet es, und `NotYet` ist eine
                // ehrliche Auskunft. Gezaehlt wird es trotzdem: Waere
                // die Zahl hoch, pruefte der Fuzzer wenig.
                if e.contains("undefined symbol") || e.contains("nicht aufgeloest") {
                    not_lowered += 1;
                    continue;
                }
                errors.push(format!("Runde {round}: baut nicht:\n{e}\n--- Quelle ---\n{src}"));
                continue;
            }
        };
        let options = takt_interp::RunOptions { ticks: TICKS, profile: None, order_seed: None };
        let interpreted = match takt_interp::run(&p, &takt_interp::Trace::default(), &options) {
            Ok(r) => r.trace.render(),
            Err(e) => {
                errors.push(format!("Runde {round}: Interpreter scheitert: {e:?}\n--- Quelle ---\n{src}"));
                continue;
            }
        };
        built += 1;
        let diffs = compare(&interpreted, &native);
        if !diffs.is_empty() {
            let list: Vec<String> = diffs.iter().take(6).map(|d| format!("  {d}")).collect();
            errors.push(format!(
                "Runde {round}: {} Abweichungen\n{}\n--- Quelle ---\n{src}--- Interpreter ---\n{}\n--- nativ ---\n{}",
                diffs.len(),
                list.join("\n"),
                interpreted.lines().take(14).collect::<Vec<_>>().join("\n"),
                native.lines().take(14).collect::<Vec<_>>().join("\n")
            ));
        }
    }

    eprintln!("{built} verglichen, {rejected} abgelehnt, {not_lowered} nicht gesenkt");
    assert!(errors.is_empty(), "{}", errors.join("\n\n"));
    // Ein Fuzzer, der nichts baut, prueft nichts. Die Schranke ist
    // grosszuegig — sie faengt den Fall, dass der Generator nur noch
    // Ablehnungen erzeugt, nicht eine Schwankung um ein paar Programme.
    assert!(built >= 40, "nur {built} von 120 Programmen kamen zum Vergleich; der Generator trifft zu selten");
}
