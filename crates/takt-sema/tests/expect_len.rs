//! `expect_len` (8.6, Pruefung 42): Ohne `capacity_bytes` bemisst die
//! Annahme den Puffer, und der Lint sagt es — fuer Kanal- und interne
//! Stroeme gleich, und nur, wenn sie wirklich unter der Hoechstlaenge liegt
//! (Tabelle 10: `expect_len < N`).

use takt_diag::Policy;
use takt_sema::{Build, Options};

const HEAD: &str = "system:\n    language = 1\n    tick = 10 ms\n\n";

/// Die Meldungen der Pruefung 42 und alle Fehler.
fn lint(body: &str) -> (Vec<String>, Vec<String>) {
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
    let out = takt_sema::compile(&format!("{HEAD}{body}"), &options);
    let of = |keep: &dyn Fn(&takt_diag::Diagnostic) -> bool| {
        out.diagnostics.iter().filter(|d| keep(d)).map(|d| format!("{d}")).collect::<Vec<_>>()
    };
    (of(&|d| d.code == "SC-42"), of(&|d| d.is_error()))
}

/// Ein interner Strom, je Tick ein `send`, gelesen von einem Handler.
fn internal(attrs: &str) -> String {
    format!(
        "stream<bytes<100>> q with capacity = 16{attrs}
output n : int in 0..999 @ hw(\"o/n\") with safe = 0

machine writer:
    var b : bytes<100> = default
    initial RUN
    state RUN:
        loop:
            send q, b

machine reader:
    var k : int in 0..999 = 0
    initial RUN
    state RUN:
        on q as e:
            k = min(k + 1, 999)
            n = k
"
    )
}

/// 8.6: interne Streams bemessen Budget und Speicher wie Kanalstroeme; die
/// Annahme `expect_len = 50` bei `bytes<100>` schwaecht Lemma 9.6.1 auch
/// hier, also warnt Pruefung 42.
#[test]
fn an_internal_stream_with_expect_len_warns_too() {
    let (warned, errors) = lint(&internal(", expect_len = 50"));
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(warned.len(), 1, "{warned:?}");
    assert!(warned[0].contains("`expect_len = 50`"), "{warned:?}");
    let (warned, _) = lint(&internal(""));
    assert!(warned.is_empty(), "{warned:?}");
}

/// Tabelle 10, Zeile 42: Nur `expect_len < N` schwaecht das Lemma. Eine
/// Annahme in Hoechstlaenge aendert nichts und warnt nicht.
#[test]
fn expect_len_at_the_maximum_does_not_warn() {
    let channel = |expect: u32| {
        format!(
            "input  rx    : stream<line<256>> @ hw(\"uart0/rx\") with max_rate = 100 Hz, framing = lines, capacity = 8, expect_len = {expect}
output rx_in : stream<line<256>> @ sim(\"uart0/rx\")
output lines : int in 0..999     @ hw(\"o/lines\")  with safe = 0

machine m:
    var n : int in 0..999 = 0
    initial RUN
    state RUN:
        on rx as l:
            n = min(n + 1, 999)
            lines = n
"
        )
    };
    let (warned, errors) = lint(&channel(80));
    assert!(errors.is_empty() && warned.len() == 1, "{warned:?} {errors:?}");
    let (warned, errors) = lint(&channel(256));
    assert!(errors.is_empty(), "{errors:?}");
    assert!(warned.is_empty(), "expect_len = N warnt: {warned:?}");
}
