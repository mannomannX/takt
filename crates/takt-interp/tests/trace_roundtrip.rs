//! Jede Trace-Zeile liest sich wieder ein (`grammar/trace.md`).
//!
//! Der Trace ist das Vergleichsformat der Abnahme (13.8) und die
//! Stimulusquelle von `takt replay` (12.5). Eine Zeile, die sich
//! schreiben, aber nicht lesen laesst, faellt erst auf, wenn ein
//! aufgezeichneter Lauf nicht mehr startet.

use takt_interp::trace::LineKind;
use takt_interp::{Trace, TraceLine};

fn roundtrip(kind: LineKind) {
    let original = Trace { lines: vec![TraceLine { tick: 7, kind }] };
    let text = original.render();
    let wieder = Trace::parse(&text).unwrap_or_else(|e| panic!("{text:?}: {e}"));
    assert_eq!(wieder.render(), text, "{text:?} liest sich anders wieder ein");
}

#[test]
fn an_end_line_reads_back() {
    roundtrip(LineKind::End { reason: "deep_sleep".into() });
    roundtrip(LineKind::End { reason: "boot_jump".into() });
    roundtrip(LineKind::End { reason: "restart".into() });
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

/// Die Zeitzeile traegt keine Semantik: Sie steht ausserhalb der
/// Hashkette (T6) und laesst einen Trace unveraendert (12.5).
#[test]
fn a_time_line_does_not_change_the_chain() {
    let ohne = Trace {
        lines: vec![TraceLine { tick: 1, kind: LineKind::Output { channel: "led".into(), value: "true".into() } }],
    };
    let mut mit = ohne.clone();
    mit.lines.push(TraceLine { tick: 1, kind: LineKind::Time { took: 5, drift: 7, slept: 0 } });
    mit.lines.push(TraceLine { tick: 2, kind: LineKind::Time { took: 5, drift: 7, slept: 0 } });
    let hash = "abc";
    assert_eq!(takt_interp::record::chain(&mit, hash), takt_interp::record::chain(&ohne, hash));
}
