//! Pruefung 43 und Szenarien (8.6, 13.6): Zwei Szenarien duerfen denselben
//! internen Strom senden, weil sie nie zusammen laufen — dieselbe Ausnahme,
//! die Pruefung 26 fuer Outputs macht (FB-213). Ein Szenario neben einer
//! Maschine bleibt ein Fehler.

use takt_diag::Policy;

fn codes(src: &str) -> Vec<String> {
    let options = takt_sema::Options {
        policy: Policy::default(),
        build: takt_sema::Build::Sim,
        profile: None,
        ..Default::default()
    };
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

/// 13.6: Nur das gewaehlte Szenario laeuft. Mit `one` kommt genau das
/// Element 1 beim Leser an, mit `two` genau die 2; das andere Szenario
/// sendet nichts.
#[test]
fn only_the_chosen_scenario_sends() {
    let src = format!(
        "{HEAD}output last : int in 0..9 @ hw(\"o/last\") with safe = 0

machine spy:
    initial RUN
    state RUN:
        on line as e:
            last = e.data as int

scenario \"one\":
    initial RUN
    state RUN:
        sequence:
            send line, 1
            wait 20 ms
            verdict pass \"one\"

scenario \"two\":
    initial RUN
    state RUN:
        sequence:
            send line, 2
            wait 20 ms
            verdict pass \"two\"
"
    );
    let options = takt_sema::Options {
        policy: Policy::default(),
        build: takt_sema::Build::Sim,
        profile: None,
        ..Default::default()
    };
    let p = takt_sema::compile(&src, &options).program.expect("Programm");
    for (name, value) in [("one", 1), ("two", 2)] {
        let r = takt_interp::run(
            &p,
            &takt_interp::Trace::default(),
            &takt_interp::RunOptions { ticks: 10, scenario: Some(name.into()), ..Default::default() },
        )
        .expect("Lauf");
        let t = r.trace.render();
        assert_eq!(r.verdict, takt_interp::Verdict::Pass, "{name}:\n{t}");
        let seen: Vec<&str> = t.lines().filter(|l| l.contains(" out n ") || l.contains(" out last ")).collect();
        assert_eq!(
            seen,
            ["t=0 out n 0", "t=0 out last 0", "t=1 out n 1", &format!("t=1 out last {value}")],
            "{name}:\n{t}"
        );
    }
}
