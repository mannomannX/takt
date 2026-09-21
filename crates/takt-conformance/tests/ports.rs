//! Registerports in der Simulation (Referenz 12.10, v1.2).
//!
//! **Was hier belegt wird.** Ein Port ist im Sim-Build ein Channel-Paar:
//! Gelesen wird der Modellwert des Ticks, geschrieben wird in einen
//! Strom, der *jeden* Vorgang einzeln und in Reihenfolge fuehrt. Das ist
//! der Unterschied zu einem Output, der am Tickende einmal committet —
//! und er ist der Grund, warum ein Treiber ueberhaupt einen Port braucht.
//!
//! Dazu die Zugriffsarten am Bitfeld (3.7): Ein `w1c`-Feld senkt auf
//! einen einzelnen Schreibvorgang mit Einzelbitmaske, nicht auf
//! Lese-Modifiziere-Schreibe. Nur so kann ein ungesehenes Ereignis nicht
//! verlorengehen.

use takt_diag::Policy;
use takt_interp::{RunOptions, Trace};

fn run(src: &str, ticks: u64) -> String {
    let options = takt_sema::Options { policy: Policy::default(), build: takt_sema::Build::Sim, profile: None };
    let out = takt_sema::compile(src, &options);
    let fehler: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(fehler.is_empty(), "{}", fehler.join("\n"));
    let program = out.program.expect("Programm");
    let options = RunOptions { ticks, ..Default::default() };
    takt_interp::run(&program, &Trace::default(), &options).expect("Lauf").trace.render()
}

const KOPF: &str = "\
system:
    language = 1
    tick     = 10 ms

record Ctrl layout little:
    flags : u8 with bits:
        a : bool at 0
        b : bool at 1 w1c

port reg : Ctrl @ mmio(0x50000000)

input w : stream<Ctrl> @ sim(\"mmio/0x50000000/w\") with capacity = 16, overflow = fault, max_rate = 400 Hz

output seen : int in 0..9 @ hw(\"o/seen\") with safe = 0
";

/// **Drei Schreibvorgaenge in einem Tick kommen als drei Elemente an, in
/// Reihenfolge.** Ein Output haette am Tickende einen Wert; ein Port
/// fuehrt jeden Vorgang.
#[test]
fn three_writes_in_one_tick_arrive_in_order() {
    let src = format!(
        "{KOPF}\n\
         output last : int in 0..9 @ hw(\"o/last\") with safe = 0\n\n\
         driver machine d:\n    var n : int in 0..9 = 0\n\n    initial RUN\n\n    \
         state RUN:\n        loop:\n            if now == 0 s:\n                \
         reg = Ctrl(flags = 1)\n                reg = Ctrl(flags = 2)\n                \
         reg = Ctrl(flags = 3)\n\n        on w as e:\n            n = n + 1\n            \
         seen = n\n            last = e.data.flags as int\n\n        after 1 s: -> RUN\n"
    );
    let trace = run(&src, 4);
    // Drei Elemente, und das zuletzt gesehene traegt den dritten Wert:
    // Haette der Strom sie zusammengefasst, stuende hier eine Eins.
    assert!(trace.contains("out seen 3"), "{trace}");
    assert!(trace.contains("out last 3"), "{trace}");
}

/// **Ein `w1c`-Feld schreibt nur die eigene Eins.** Die Senkung ist
/// `reg.0 = 0.with_bit(1, true)`: Kein Lesen, kein Zurueckschreiben
/// fremder Bits — sonst loeschte ein Treiber Ereignisse, die er nie
/// gesehen hat (FB-11).
#[test]
fn a_w1c_write_sends_only_its_own_bit() {
    let src = format!(
        "{KOPF}\n\
         driver machine d:\n    initial RUN\n\n    \
         state RUN:\n        loop:\n            if now == 0 s:\n                \
         reg.flags.b = true\n\n        on w as e:\n            seen = e.data.flags as int\n\n        \
         after 1 s: -> RUN\n"
    );
    let trace = run(&src, 4);
    // Bit 1 allein ist die Zwei. Waere es ein Lese-Modifiziere-Schreibe
    // ueber den ganzen Traeger, stuende hier der gelesene Wert mit
    // gesetztem Bit 1.
    assert!(trace.contains("out seen 2"), "{trace}");
}

/// **Ein `rw`-Feld unter einem Port liest und schreibt den ganzen
/// Record.** Es hat keinen eigenen Speicher; der Wert kommt vom Modell.
#[test]
fn a_plain_field_under_a_port_goes_through_the_whole_record() {
    let src = format!(
        "{KOPF}\n\
         output model : Ctrl @ sim(\"mmio/0x50000000/r\")\n\
         output got   : int in 0..255 @ hw(\"o/got\") with safe = 0\n\n\
         machine m:\n    initial RUN\n\n    state RUN:\n        loop:\n            \
         model = Ctrl(flags = 0x83)\n\n        after 1 s: -> RUN\n\n\
         driver machine d:\n    initial RUN\n\n    \
         state RUN:\n        loop:\n            if now == 20 ms:\n                \
         reg.flags.a = false\n\n        on w as e:\n            got = e.data.flags as int\n\n        \
         after 1 s: -> RUN\n"
    );
    let trace = run(&src, 5);
    // Gelesen wurde 0x83, Bit 0 geloescht, Bit 1 ist `w1c` und bleibt beim
    // Schreiben des ganzen Traegers stehen: 0x82 = 130. Ohne das Lesen
    // stuende hier 0 — das Feld hat keinen eigenen Speicher.
    assert!(trace.contains("out got 130"), "{trace}");
}
