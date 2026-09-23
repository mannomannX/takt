//! Pruefung 43 und Szenarien (8.6, 13.6): Zwei Szenarien duerfen denselben
//! internen Strom senden, weil sie nie zusammen laufen — dieselbe Ausnahme,
//! die Pruefung 26 fuer Outputs macht (FB-213). Ein Szenario neben einer
//! Maschine bleibt ein Fehler.

use takt_diag::Policy;

fn codes(src: &str) -> Vec<String> {
    let options = takt_sema::Options { policy: Policy::default(), build: takt_sema::Build::Sim, profile: None };
    takt_sema::compile(src, &options).diagnostics.iter().filter(|d| d.is_error()).map(|d| d.code.to_string()).collect()
}

const HEAD: &str = "\
system:
    language = 1
    tick     = 10 ms

stream<u8> line with capacity = 8, overflow = fault
output n : int in 0..99 @ hw(\"o/n\") with safe = 0

machine reader:
    var c : int in 0..99 = 0
    initial RUN
    state RUN:
        on line as e:
            c = (c + 1) % 100
            n = c
";

#[test]
fn two_scenarios_may_send_to_the_same_stream() {
    let src = format!(
        "{HEAD}
scenario \"one\":
    initial RUN
    state RUN:
        sequence:
            send line, 1
            verdict pass \"one\"

scenario \"two\":
    initial RUN
    state RUN:
        sequence:
            send line, 2
            verdict pass \"two\"
"
    );
    assert!(codes(&src).is_empty(), "{:?}", codes(&src));
}

#[test]
fn a_scenario_beside_a_machine_is_still_two_writers() {
    let src = format!(
        "{HEAD}
machine writer:
    initial RUN
    state RUN:
        loop:
            send line, 3

scenario \"one\":
    initial RUN
    state RUN:
        sequence:
            send line, 1
            verdict pass \"one\"
"
    );
    assert_eq!(codes(&src), ["SC-43"]);
}
