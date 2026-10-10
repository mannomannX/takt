//! Jede Trace-Zeile liest sich wieder ein (`grammar/trace.md`).
//!
//! Der Trace ist das Vergleichsformat der Abnahme (13.8) und die
//! Stimulusquelle von `takt replay` (12.5). Eine Zeile, die sich
//! schreiben, aber nicht lesen laesst, faellt erst auf, wenn ein
//! aufgezeichneter Lauf nicht mehr startet.

use takt_interp::trace::LineKind;
use takt_interp::{Trace, TraceLine};

fn roundtrip(kind: LineKind) {
    let original = Trace { lines: vec![TraceLine { tick: 7, kind }], ..Trace::default() };
    let text = original.render();
    let wieder = Trace::parse(&text).unwrap_or_else(|e| panic!("{text:?}: {e}"));
    assert_eq!(wieder.render(), text, "{text:?} liest sich anders wieder ein");
}

#[test]
fn an_end_line_reads_back() {
    for reason in ["now", "after", "on_wake", "on_start", "scenario"] {
        roundtrip(LineKind::End { reason: reason.into() });
    }
    roundtrip(LineKind::Persist { hex: "0a0bff".into() });
    roundtrip(LineKind::Property { assumption: false, name: "no_chatter".into(), at: 12 });
    roundtrip(LineKind::Property { assumption: true, name: "slew".into(), at: 0 });
}

/// 12.6: was der Treiberrand meldet, und eine Lieferung mit Zeitstempel
/// und Folgenummer.
#[test]
fn driver_lines_and_timestamped_deliveries_read_back() {
    roundtrip(LineKind::Driver { name: "adc".into(), event: "degraded seq".into() });
    roundtrip(LineKind::Driver { name: "adc".into(), event: "recovered".into() });
    roundtrip(LineKind::Driver { name: "adc".into(), event: "warped p".into() });
    let text = "t=3 in rx \"go t=5 seq=9\" t=25000000 seq=12\nt=3 in p 5 t=-1\n";
    let trace = Trace::parse(text).expect("liest sich");
    assert_eq!(trace.render(), text);
    let LineKind::Input { sample, .. } = &trace.lines[0].kind else { panic!("`in`") };
    assert_eq!(sample.value.as_deref(), Some("\"go t=5 seq=9\""), "im Text ist `t=` kein Schluessel");
    assert_eq!((sample.t, sample.seq), (Some(25_000_000), Some(12)));
}

#[test]
fn an_output_line_reads_back() {
    roundtrip(LineKind::Output { channel: "led".into(), value: "true".into() });
}

#[test]
fn a_state_line_reads_back() {
    roundtrip(LineKind::State { machine: "m".into(), path: "RUN.INNER".into() });
}

#[test]
fn a_final_verdict_reads_back() {
    roundtrip(LineKind::Final { verdict: "PASS".into() });
}

#[test]
fn a_time_line_reads_back() {
    roundtrip(LineKind::Time { took: 123_456, drift: -2_000, slept: 0 });
    roundtrip(LineKind::Time { took: 0, drift: 40_000_000, slept: 49 });
}

/// 8.2, 12.5: ein Input, den das Programm nicht liest, in der Form einer
/// `in`-Zeile — wie ein Board ihn schreibt, mit Leerzeichen am Ende.
#[test]
fn a_record_line_reads_back() {
    let text =
        "t=0 rec daq1_ai3 21.5 degC \nt=2 rec daq1_ai3 22.0 degC t=15000000 \nt=4 rec daq1_di0 bad reason=Driver\n";
    let trace = Trace::parse(text).expect("liest sich");
    assert_eq!(
        trace.render(),
        "t=0 rec daq1_ai3 21.5 degC\nt=2 rec daq1_ai3 22.0 degC t=15000000\nt=4 rec daq1_di0 bad reason=Driver\n"
    );
    let LineKind::Record { channel, sample } = &trace.lines[1].kind else { panic!("`rec`") };
    assert_eq!(
        (channel.as_str(), sample.value.as_deref(), sample.t),
        ("daq1_ai3", Some("22.0 degC"), Some(15_000_000))
    );
    assert!(trace.lines.iter().all(|l| l.kind.is_meta()), "Aufzeichnung, nicht Semantik");
}

/// Die Zeitzeile traegt keine Semantik: Sie steht ausserhalb der
/// Hashkette (T6) und laesst einen Trace unveraendert (12.5).
#[test]
fn a_time_line_does_not_change_the_chain() {
    let ohne = Trace {
        lines: vec![TraceLine { tick: 1, kind: LineKind::Output { channel: "led".into(), value: "true".into() } }],
        ..Trace::default()
    };
    let mut mit = ohne.clone();
    mit.lines.push(TraceLine { tick: 1, kind: LineKind::Time { took: 5, drift: 7, slept: 0 } });
    mit.lines.push(TraceLine { tick: 2, kind: LineKind::Time { took: 5, drift: 7, slept: 0 } });
    let rec = Trace::parse("t=1 rec daq1_di0 true\n").expect("`rec`");
    mit.lines.extend(rec.lines);
    let hash = "abc";
    assert_eq!(takt_interp::record::chain(&mit, hash), takt_interp::record::chain(&ohne, hash));
}

/// Wie viele Arten `LineKind` hat; `index_of` zaehlt sie ab.
const KINDS: usize = 25;

/// Die Nummer einer Art. Das `match` nennt jede Variante: Eine neue Art
/// uebersetzt hier erst, wenn sie eine Nummer und damit eine Zeile in
/// `one_of_each` hat.
fn index_of(kind: &LineKind) -> usize {
    match kind {
        LineKind::Input { .. } => 0,
        LineKind::Command { .. } => 1,
        LineKind::Tune { .. } => 2,
        LineKind::Abort => 3,
        LineKind::Runtime { .. } => 4,
        LineKind::Output { .. } => 5,
        LineKind::State { .. } => 6,
        LineKind::Published { .. } => 7,
        LineKind::Signal { .. } => 8,
        LineKind::Job { .. } => 9,
        LineKind::Fault { .. } => 10,
        LineKind::Log { .. } => 11,
        LineKind::Alert { .. } => 12,
        LineKind::Measure { .. } => 13,
        LineKind::Verify { .. } => 14,
        LineKind::Verdict { .. } => 15,
        LineKind::Property { .. } => 16,
        LineKind::Stream { .. } => 17,
        LineKind::Driver { .. } => 18,
        LineKind::Final { .. } => 19,
        LineKind::End { .. } => 20,
        LineKind::Persist { .. } => 21,
        LineKind::Time { .. } => 22,
        LineKind::Record { .. } => 23,
        LineKind::Tx { .. } => 24,
    }
}

/// Jede Art mindestens einmal, mit einem Text, der `"`, `\` und ein
/// Zeilenende traegt.
fn one_of_each() -> Vec<LineKind> {
    let text = "say \"hi\" \\ a\nb".to_string();
    let all = vec![
        LineKind::Input { channel: "p".into(), sample: sample("45 bar") },
        LineKind::Command { name: "start".into() },
        LineKind::Tune { name: "GAIN".into(), value: "3".into(), accepted: false },
        LineKind::Abort,
        LineKind::Runtime { kind: "Driver".into(), output: Some("valve".into()) },
        LineKind::Output { channel: "led".into(), value: "true".into() },
        LineKind::State { machine: "m".into(), path: "RUN.INNER".into() },
        LineKind::Published { machine: "m".into(), var: "count".into(), value: "3".into() },
        LineKind::Signal { machine: "m".into(), name: "done".into() },
        LineKind::Job { machine: "m".into(), handle: "v".into(), start: 3, late: false },
        LineKind::Job { machine: "m".into(), handle: "v".into(), start: 4, late: true },
        LineKind::Fault { machine: "m".into(), kind: "Timeout".into(), message: text.clone(), target: "SAFE".into() },
        LineKind::Log { machine: "m".into(), text: text.clone() },
        LineKind::Alert { machine: "m".into(), on: true, text: text.clone() },
        LineKind::Measure { machine: "m".into(), name: "boot".into(), value: "1500 ms".into() },
        LineKind::Verify { machine: "m".into(), ok: false, text: text.clone() },
        LineKind::Verdict { machine: "m".into(), pass: true, text: Some(text.clone()) },
        LineKind::Verdict { machine: "m".into(), pass: false, text: None },
        LineKind::Property { assumption: true, name: "slew".into(), at: 4 },
        LineKind::Stream { name: "rx".into(), dropped: 2, overflowed: 0, malformed: 1 },
        LineKind::Driver { name: "adc".into(), event: "degraded seq".into() },
        LineKind::Final { verdict: "PASS".into() },
        LineKind::End { reason: "now".into() },
        LineKind::Persist { hex: "0aff".into() },
        LineKind::Time { took: 5, drift: -7, slept: 1 },
        LineKind::Record { channel: "daq1_ai3".into(), sample: sample("21.5 degC") },
        LineKind::Tx { stream: "uart_tx".into(), free: 64, idle: false },
    ];
    let mut seen = [false; KINDS];
    for kind in &all {
        seen[index_of(kind)] = true;
    }
    let missing: Vec<usize> = (0..KINDS).filter(|i| !seen[*i]).collect();
    assert!(missing.is_empty(), "Arten ohne Zeile: {missing:?}");
    all
}

fn sample(value: &str) -> takt_interp::trace::SampleText {
    takt_interp::trace::SampleText {
        value: Some(value.into()),
        quality: None,
        reason: None,
        age: None,
        t: None,
        seq: None,
    }
}

/// INT-012: `parse(render(x)) == x` fuer jede Art, als Struktur und nicht
/// nur als Text — ein Text mit `"` las sich sonst als `say ` zurueck, und
/// ein Zeilenende zerriss die Zeile samt Hashkette (T6).
#[test]
fn every_line_kind_reads_back_as_itself() {
    for kind in one_of_each() {
        let original = Trace { lines: vec![TraceLine { tick: 7, kind }], ..Trace::default() };
        let text = original.render();
        assert_eq!(text.lines().count(), 1, "eine Zeile: {text:?}");
        let back = Trace::parse(&text).unwrap_or_else(|e| panic!("{text:?}: {e}"));
        assert_eq!(back, original, "{text:?}");
    }
}

/// SEM2-056: Zeilen, die kein Trace sind, lehnt der Leser ab, statt sie
/// still umzudeuten.
#[test]
fn a_malformed_line_is_refused() {
    let refused = [
        ("in p 5", "ohne Tick"),
        ("t=-1 in p 5", "negativer Tick"),
        ("t=x in p 5", "nichtnumerischer Tick"),
        ("t=1 frobnicate x", "unbekannte Art"),
        ("t=1 in", "Input ohne Kanal"),
        ("t=1 in p", "Input ohne Wert"),
        ("t=1 log m say", "Text ohne Anfuehrungszeichen"),
        ("t=1 log m \"say", "Text ohne Ende"),
        ("t=1 log m \"a\\q\"", "unbekanntes Escape"),
        ("t=1 job m v", "Job ohne done"),
        ("t=1 stream rx dropped=1 overflowed=0", "Zaehler fehlt"),
        ("t=1 persist abc", "ungerade viele Hexziffern"),
        ("t=1 property p violated x", "Position keine Zahl"),
        ("t=1 time took=1 drift=2", "Feld fehlt"),
    ];
    let mut accepted = Vec::new();
    for (line, why) in refused {
        if let Ok(t) = Trace::parse(line) {
            accepted.push(format!("  `{line}` ({why}) las sich als {:?}", t.lines));
        }
    }
    assert!(accepted.is_empty(), "angenommen statt abgelehnt:\n{}", accepted.join("\n"));
}

/// GEN-007 (T2): Eine Qualitaet darf hinter dem Wert stehen, wie `in` sie
/// schreibt (`90 K stale age=120 ms`); sie ist dann Qualitaet, nicht Teil
/// des Werts. Ein Text in Anfuehrungszeichen bleibt Wert.
#[test]
fn a_quality_after_the_value_is_a_quality() {
    let text = "t=2 in lox_temp 90 K stale age=120 ms\nt=3 in p 12.0 bar bad reason=Driver\nt=4 in s \"stale\"\n";
    let trace = Trace::parse(text).expect("liest sich");
    assert_eq!(trace.render(), text);
    let samples: Vec<(Option<&str>, Option<&str>)> = trace
        .lines
        .iter()
        .filter_map(|l| match &l.kind {
            LineKind::Input { sample, .. } => Some((sample.value.as_deref(), sample.quality.as_deref())),
            _ => None,
        })
        .collect();
    assert_eq!(samples, [(Some("90 K"), Some("stale")), (Some("12.0 bar"), Some("bad")), (Some("\"stale\""), None)]);
}
