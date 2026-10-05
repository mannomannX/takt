//! Standardbibliothek (Referenz 11.4): die Bloecke laufen im Golden-Trace
//! `corpus-try/sim/11_4`; hier steht, was der Trace nicht zeigt — der
//! Sichtbereich der Vorlagen (2.5) und ein `writer` nach `reset()` (5.7).

use takt_diag::Policy;
use takt_interp::{RunOptions, Trace, run};
use takt_mir::Program;
use takt_sema::{Build, Options};

const HEAD: &str = "system:\n    language = 1\n    tick = 1 ms\n\n";

fn compile(body: &str) -> Program {
    let src = format!("{HEAD}{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "unerwartete Fehler:\n{}", errors.join("\n"));
    out.program.expect("Programm")
}

fn trace(p: &Program, stimulus: &str, ticks: u64) -> String {
    let stimulus = Trace::parse(stimulus).expect("Stimulus");
    run(p, &stimulus, &RunOptions { ticks, ..Default::default() }).expect("Lauf").trace.render()
}

/// Eingebaute Operationen aus 11.4: keine Deklaration im Prelude, sondern
/// Intrinsics (4.2, 3.9) und Matrixfunktionen (3.11) der Sema.
const BUILTIN: [&str; 7] = ["fma", "interp", "solve", "inv", "det", "cholesky", "transpose"];

/// Was 11.4 nennt und noch fehlt, mit dem Schritt, der es nachzieht; seit
/// M10 Schritt 21 nichts.
const MISSING: [(&str, &str); 0] = [];

/// Die Eintraege des Codeblocks in 11.4 als (Art, Name). Eine Zeile traegt
/// Segmente, getrennt durch zwei Leerzeichen; ein Segment, das mit einer
/// Art beginnt, nennt Namen, getrennt durch ` / ` — die Methoden eines
/// Blocks stehen in einem spaeteren Segment ohne Art.
fn entries_of_11_4() -> Vec<(String, String)> {
    let reference = include_str!("../../../plan/definition.md");
    let start = reference.find("### 11.4 ").expect("11.4");
    let block = &reference[start..];
    let block = &block[block.find("```\n").expect("Codeblock") + 4..];
    let block = &block[..block.find("```").expect("Ende des Codeblocks")];
    let mut out = Vec::new();
    for line in block.lines().filter(|l| !l.starts_with('#')) {
        for segment in line.split("  ").map(str::trim).filter(|s| !s.is_empty()) {
            let Some((kind, rest)) = ["native fn ", "native job ", "fn ", "block ", "machine "]
                .iter()
                .find_map(|k| segment.strip_prefix(k).map(|r| (k.trim(), r)))
            else {
                continue;
            };
            for part in rest.split(" / ") {
                let name: String = part.trim().chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
                if !name.is_empty() {
                    out.push((kind.to_string(), name));
                }
            }
        }
    }
    out
}

/// **Jeder Eintrag aus 11.4 hat eine Umsetzung** (M10 Schritt 20): eine
/// Deklaration im Prelude, eine Native der kuratierten Menge (4.5, 13.8)
/// oder eine eingebaute Operation. Was fehlt, steht in `MISSING` mit dem
/// Schritt, der es nachzieht — und verschwindet dort, sobald es kommt.
#[test]
fn every_entry_of_11_4_exists() {
    let prelude = include_str!("../src/prelude.takt");
    let declared = |kind: &str, name: &str| {
        let kind = if kind.starts_with("native") { "fn" } else { kind };
        prelude.lines().any(|l| {
            l.strip_prefix(kind)
                .and_then(|r| r.strip_prefix(' '))
                .and_then(|r| r.strip_prefix(name))
                .is_some_and(|r| r.starts_with('[') || r.starts_with('('))
        })
    };
    let entries = entries_of_11_4();
    assert!(entries.len() > 40, "der Codeblock wird nicht gelesen: {entries:?}");
    for (kind, name) in &entries {
        let found =
            declared(kind, name) || takt_native::Native::by_name(name).is_some() || BUILTIN.contains(&name.as_str());
        let missing = MISSING.iter().find(|(m, _)| m == name);
        match (found, missing) {
            (true, None) | (false, Some(_)) => {}
            (true, Some((_, step))) => panic!("`{name}` gibt es jetzt; aus MISSING nehmen ({step})"),
            (false, None) => panic!("11.4 nennt `{kind} {name}`, die Bibliothek hat es nicht"),
        }
    }
}

#[test]
fn a_user_name_does_not_collide_with_library_parameters() {
    // `hysteresis.step(x: …)` nennt seinen Parameter `x`; der Input `x`
    // des Nutzers liegt in einem anderen Bereich, die Vorlage sieht nur
    // das Prelude.
    let p = compile(
        "input x : float[V] in 0..10 V @ hw(\"a/x\") with max_age = 10 ms
output above : bool @ hw(\"o/above\") with safe = false
machine m:
    var h = hysteresis[V](lo = 2 V, hi = 4 V)
    initial RUN
    state RUN:
        loop:
            above = h.step(x)
",
    );
    let t = trace(&p, "t=0 in x 5.0 V\n", 2);
    assert!(t.contains("t=0 out above true"), "{t}");
}

#[test]
fn the_library_uses_its_own_helpers() {
    // Ein `clamp` des Nutzers verdeckt das der Bibliothek (Warnung 2.5);
    // `pid` rechnet trotzdem mit seinem eigenen.
    let p = compile(
        "fn clamp[U](x: float[U], lo: float[U], hi: float[U]) -> float[U]:
    return lo

output u : float[pct] @ hw(\"o/u\") with safe = 0 pct
machine m:
    var ctrl = pid[pct, K](kp = 2 pct/K, ki = 0 pct/K/s, kd = 0 pct*s/K, lo = 0 pct, hi = 100 pct)
    initial RUN
    state RUN:
        loop:
            u = ctrl.step(1 K, 1 ms)
",
    );
    let t = trace(&p, "", 1);
    assert!(t.contains("t=0 out u 2.0 pct"), "{t}");
}

#[test]
fn a_writer_starts_over_after_reset() {
    let p = compile(
        "output n : int in 0..8 @ hw(\"o/n\") with safe = 0
machine m:
    var w     = writer[8]()
    var ok    : bool = false
    var frame : bytes<8> = default
    initial RUN
    state RUN:
        enter:
            ok = w.u16_le(0xBEEF)
            w.reset()
            ok = w.u8(7)
            frame = w.data()
            n = frame.len
",
    );
    let t = trace(&p, "", 1);
    assert!(t.contains("t=0 out n 1"), "{t}");
}

#[test]
fn an_integer_lowpass_settles_exactly() {
    // 11.4: 16 Nachkommabits im Zustand — der Sprung kommt ganz an, der
    // erste Schritt rundet wie die Fliesskommaform (1000 / 11 = 90.9).
    let p = compile(
        "input raw : int[mV] in -32768..32767 mV @ hw(\"a/raw\") with max_age = 3 s
output flt : int[mV] in -32768..32767 mV @ hw(\"o/flt\") with safe = 0 mV
machine m every 10 ms:
    var f = lowpass_i[mV](tau = 100 ms)
    initial RUN
    state RUN:
        loop:
            flt = f.step(raw, 10 ms)
",
    );
    let t = trace(&p, "t=0 in raw 0 mV\nt=10 in raw -1000 mV\n", 2000);
    assert!(t.contains("t=10 out flt -91 mV"), "{t}");
    let last = t.lines().rev().find(|l| l.contains("out flt")).expect("Ausgabe");
    assert!(last.ends_with("out flt -1000 mV"), "{last}");
}

#[test]
fn a_decaying_lowpass_stops_at_zero_before_the_subnormals() {
    // 4.2: Mit tau = dt halbiert jeder Schritt den Zustand; nach 100
    // Schritten laege er unter 1e-30 und ohne Totband bald in den
    // Subnormalen. Das Totband setzt ihn dort auf null.
    let p = compile(
        "input x : float[bar] in -10 bar..10 bar @ hw(\"a/x\") with max_age = 3 s
output y : float[bar] @ hw(\"o/y\") with safe = 0 bar
machine m every 10 ms:
    var f = lowpass[bar](tau = 10 ms)
    initial RUN
    state RUN:
        loop:
            y = f.step(x, 10 ms)
",
    );
    let t = trace(
        &p,
        "t=0 in x 1 bar
t=10 in x 0 bar
",
        2000,
    );
    let values: Vec<f64> =
        t.lines().filter_map(|l| l.split_once(" out y ")?.1.strip_suffix(" bar")?.parse().ok()).collect();
    assert!(values.iter().all(|v| *v == 0.0 || v.abs() >= 1e-30), "unter dem Totband: {t}");
    assert_eq!(values.last(), Some(&0.0), "{t}");
    assert!(values.len() <= 102, "{} Werte bis null: {t}", values.len());
}

#[test]
fn an_integer_pid_clamps_at_its_limits() {
    // Verstaerkungen je Schritt in int[O/E]: 2 mpct/mK * 1000 mK plus das
    // Integral 1000 mpct je Schritt, gedeckelt bei hi.
    let p = compile(
        "input err : int[mK] in -100000..100000 mK @ hw(\"a/err\") with max_age = 3 s
output u : int[mpct] in 0..10000 mpct @ hw(\"o/u\") with safe = 0 mpct
machine m every 10 ms:
    var c = pid_i[mpct, mK](kp = 2 mpct/mK, ki = 1 mpct/mK, kd = 0 mpct/mK, lo = 0 mpct, hi = 10000 mpct)
    initial RUN
    state RUN:
        loop:
            u = c.step(err)
",
    );
    let t = trace(&p, "t=0 in err 1000 mK\n", 200);
    assert!(t.contains("t=0 out u 3000 mpct"), "{t}");
    assert!(t.contains("t=10 out u 4000 mpct"), "{t}");
    let last = t.lines().rev().find(|l| l.contains("out u ")).expect("Ausgabe");
    assert!(last.ends_with("out u 10000 mpct"), "{last}");
}

/// Die Bloecke des Codeblocks in 11.4 mit den Methoden, die er nennt:
/// das Segment nach dem Kopf, Namen vor `(`, getrennt durch ` / `.
/// Klammerbemerkungen (`(3.9; take(n) … offen …)`) und Kommentare nennen
/// keine Methode.
fn block_methods_of_11_4() -> Vec<(String, Vec<String>)> {
    let reference = include_str!("../../../plan/definition.md");
    let start = reference.find("### 11.4 ").expect("11.4");
    let block = &reference[start..];
    let block = &block[block.find("```\n").expect("Codeblock") + 4..];
    let block = &block[..block.find("```").expect("Ende des Codeblocks")];
    let ident = |s: &str| -> String { s.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect() };
    let mut out = Vec::new();
    for line in block.lines() {
        let Some(rest) = line.strip_prefix("block ") else { continue };
        let line = rest.split('#').next().unwrap_or_default();
        let mut segments = line.split("  ").map(str::trim).filter(|s| !s.is_empty() && !s.starts_with('('));
        let head = segments.next().unwrap_or_default();
        let methods: Vec<String> = segments
            .flat_map(|seg| seg.split(" / "))
            .filter(|part| {
                ident(part.trim()).len() < part.trim().len() && part.trim()[ident(part.trim()).len()..].starts_with('(')
            })
            .map(|part| ident(part.trim()))
            .collect();
        for name in head.split(" / ").map(|b| ident(b.trim())) {
            out.push((name, methods.clone()));
        }
    }
    out
}

/// Die Methoden eines Blocks im Prelude: Zeilen `    name(` in seinem Rumpf.
fn prelude_methods(block: &str) -> Vec<String> {
    let prelude = include_str!("../src/prelude.takt");
    let mut lines = prelude.lines().skip_while(|l| {
        !l.strip_prefix("block ")
            .and_then(|r| r.strip_prefix(block))
            .is_some_and(|r| r.starts_with('[') || r.starts_with('('))
    });
    assert!(lines.next().is_some(), "`block {block}` fehlt im Prelude");
    lines
        .take_while(|l| l.is_empty() || l.starts_with(' '))
        .filter_map(|l| l.strip_prefix("    ").filter(|r| !r.starts_with(' ')))
        .filter_map(|r| r.split_once('(').map(|(name, _)| name.to_string()))
        .filter(|name| name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
        .collect()
}

/// **Jede Methode, die 11.4 einem Block gibt, hat er im Prelude.**
#[test]
fn every_block_method_of_11_4_exists() {
    let blocks = block_methods_of_11_4();
    assert!(blocks.len() > 20, "der Codeblock wird nicht gelesen: {blocks:?}");
    assert!(blocks.iter().any(|(b, m)| b == "writer" && m.contains(&"patch_u16".to_string())), "{blocks:?}");
    let mut missing = Vec::new();
    for (block, methods) in &blocks {
        let have = prelude_methods(block);
        for m in methods {
            if !have.contains(m) {
                missing.push(format!("{block}.{m} (im Prelude: {have:?})"));
            }
        }
    }
    assert!(missing.is_empty(), "11.4 nennt, der Prelude hat nicht:\n{}", missing.join("\n"));
}

/// Das `dt` der Bloecke liegt in `tick..1 h` (Prelude): ein Literal
/// daneben ist ein Fehler, ein Wert daneben zur Laufzeit ein `RangeFault`.
#[test]
fn dt_outside_tick_to_one_hour_is_refused() {
    for dt in ["0 ms", "2 h"] {
        let src = format!(
            "{HEAD}output y : float[V] @ hw(\"o/y\") with safe = 0 V
machine m:
    var f = lowpass[V](tau = 10 ms)
    initial RUN
    state RUN:
        loop:
            y = f.step(1 V, {dt})
"
        );
        let out = takt_sema::compile(&src, &Options::default());
        let e: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
        assert!(e.len() == 1 && e[0].contains("SC-3") && e[0].contains("Dauer ausserhalb der Range"), "{dt}: {e:?}");
    }
    let p = compile(
        "output y : float[V] @ hw(\"o/y\") with safe = 0 V
machine m:
    var f = lowpass[V](tau = 10 ms)
    var d : Duration = 0 ms
    initial RUN
    state RUN:
        loop:
            y = f.step(1 V, d)
",
    );
    let t = trace(&p, "", 1);
    assert!(t.contains("t=0 fault m RangeFault \"0 ns ausserhalb der Range\""), "{t}");
}
