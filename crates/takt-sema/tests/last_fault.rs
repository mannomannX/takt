//! `last_fault` (5.3, FB-340): Art, Nachricht, Zeile und Tick des letzten
//! Faults. Korpus 98 faultet je Runde anders; der Differentialtest haelt den
//! erzeugten Code dagegen, hier stehen die Werte selbst.

use takt_diag::Policy;
use takt_interp::{RunOptions, Trace, run};
use takt_sema::{Build, Options};

fn source() -> String {
    std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try/98_last_fault.takt"))
        .expect("Quelle")
}

/// Die Zeile (ab 1), in der `needle` zum ersten Mal steht.
fn line_of(src: &str, needle: &str) -> i64 {
    src.lines().position(|l| l.contains(needle)).unwrap_or_else(|| panic!("`{needle}` fehlt")) as i64 + 1
}

/// Die Zeile (ab 1), die ohne Einrueckung genau `text` ist.
fn line_exact(src: &str, text: &str) -> i64 {
    src.lines().position(|l| l.trim() == text).unwrap_or_else(|| panic!("`{text}` fehlt")) as i64 + 1
}

/// Der Wert von `name` im Tick `t`, wie der Trace ihn schreibt.
fn value(trace: &str, t: u64, name: &str) -> Option<i64> {
    let prefix = format!("t={t} out {name} ");
    trace.lines().find_map(|l| l.strip_prefix(&prefix)).and_then(|v| v.trim().parse().ok())
}

#[test]
fn last_fault_names_kind_message_line_and_tick() {
    let src = source();
    let options = Options { policy: Policy::default(), build: Build::Sim, ..Default::default() };
    let p = takt_sema::compile(&src, &options).program.expect("Programm");
    let trace =
        run(&p, &Trace::default(), &RunOptions { ticks: 16, ..Default::default() }).expect("Lauf").trace.render();
    // (Tick, Art, Zeile, Laenge der Nachricht). Die Nachricht ist die der
    // Anweisung, ohne Meldung `check verletzt`, sonst der Name der Art; die
    // ueber 128 Byte endet vor dem Umlaut, den die Grenze teilen wuerde.
    let want = [
        (1, 1, line_of(&src, "check n < 2, \"Runde"), "Runde 0 bei 2".len()),
        (3, 1, line_exact(&src, "check n < 2"), "check verletzt".len()),
        (5, 21, line_of(&src, "small = 10 / (turn - 2)"), "ARITHMETIC".len()),
        (7, 7, line_of(&src, "small = n * 3"), "RANGE".len()),
        (9, 21, line_of(&src, "return total / parts"), "ARITHMETIC".len()),
        (11, 1, line_of(&src, "check n < 2, \"xxx"), 127),
        (13, 11, line_of(&src, "abort \"Halt"), "Halt 6".len()),
    ];
    for (t, kind, line, size) in want {
        let got =
            (value(&trace, t, "kind"), value(&trace, t, "line"), value(&trace, t, "stamp"), value(&trace, t, "size"));
        // Ein Wert, der sich gegenueber dem Tick davor nicht aendert, steht
        // nicht im Trace; `kind` bleibt zwischen zwei gleichen Arten stehen.
        let kind_now = got.0.or_else(|| (0..t).rev().find_map(|u| value(&trace, u, "kind")));
        assert_eq!(
            (kind_now, got.1, got.2, got.3),
            (Some(kind), Some(line), Some(t as i64), Some(size as i64)),
            "Tick {t}:\n{trace}"
        );
    }
}

/// Die Positionen des Prelude zeigen in seine eigene Datei: Die Zeile eines
/// Faults darin zaehlt dort, nicht im Programm.
#[test]
fn prelude_positions_are_in_their_own_file() {
    let src = source();
    let p = takt_sema::compile(&src, &Options::default()).program.expect("Programm");
    let prelude_lines = takt_sema::PRELUDE.lines().count() as u32;
    let program_fn = p.fns.iter().find(|f| f.name == "share").expect("fn share");
    assert_eq!(program_fn.span.file, takt_diag::FileId(0));
    assert_eq!(p.line_of(program_fn.span) as i64, line_of(&src, "fn share"));
    let prelude_fn = p.fns.iter().find(|f| f.span.file == takt_sema::PRELUDE_FILE).expect("eine Funktion des Prelude");
    let line = p.line_of(prelude_fn.span);
    assert!((1..=prelude_lines).contains(&line), "Zeile {line} von {prelude_lines}");
}
