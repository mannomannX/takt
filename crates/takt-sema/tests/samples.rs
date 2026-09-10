//! Oversampelte Kanaele (Referenz 8.9): das Tick-Array, die Reduktionen
//! `min`/`max`/`mean`/`rms`/`count`/`last`, die Schleife ueber die Samples
//! und die Qualitaetsregeln bei fehlenden und ausserhalb der Range liegenden
//! Samples.

use takt_diag::Policy;
use takt_interp::{RunOptions, Trace, run};
use takt_sema::{Build, Options};

const HEAD: &str = "system:\n    language = 1\n    tick = 1 ms\n\n";

fn simulate(body: &str, stim: &str, ticks: u64) -> String {
    let src = format!("{HEAD}{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "unerwartete Fehler:\n{}", errors.join("\n"));
    let program = out.program.expect("Programm");
    let stimulus = Trace::parse(stim).expect("Stimulus lesbar");
    run(&program, &stimulus, &RunOptions { ticks, ..Default::default() }).expect("Lauf").trace.render()
}

/// Ein Kanal mit vier Samples je Tick und einer deklarierten Range.
const OVERSAMPLED: &str = "\
input i_dut : samples<float[A] in 0.0 A .. 3.0 A, 4> @ hw(\"daq1/ai2\") with rate = 4 kHz

output peak : float[A] @ hw(\"o/peak\") with safe = 0.0 A
output n : int in 0..99 @ hw(\"o/n\") with safe = 0
output ok : bool @ hw(\"o/ok\") with safe = false

machine watch:
    initial RUN
    state RUN:
        loop:
            ok = i_dut.valid
            peak = i_dut.max()
            n = i_dut.count
";

#[test]
fn reductions_read_the_tick_array() {
    // 8.9: „`samples<T, N>` liefert pro Basis-Tick ein beschraenktes Array;
    // Reduktionen `.min() .max() .mean() .rms() .count .last`".
    let trace = simulate(OVERSAMPLED, "t=0 in i_dut [0.5 A, 1.0 A, 2.0 A, 1.2 A]\n", 2);
    assert!(trace.contains("t=0 out peak 2.0 A\n"), "das Maximum: {trace}");
    assert!(trace.contains("t=0 out n 4\n"), "die Anzahl: {trace}");
}

#[test]
fn a_sample_outside_the_range_spoils_the_whole_array() {
    // 8.9: „ein Sample ausserhalb der deklarierten Range macht das ganze
    // Tick-Array `Bad` (Grund `OutOfRange`, konservativ)". Die Range steht am
    // Element, der Kanal traegt sie also nicht selbst.
    let trace = simulate(
        OVERSAMPLED,
        "t=0 in i_dut [0.5 A, 1.0 A, 2.0 A, 1.2 A]\nt=1 in i_dut [0.5 A, 9.9 A, 2.0 A, 1.2 A]\n",
        3,
    );
    assert!(trace.contains("t=0 out ok true\n"), "erst gueltig: {trace}");
    assert!(trace.contains("t=1 out ok false\n"), "ein Ausreisser verdirbt das Array: {trace}");
}

#[test]
fn a_short_array_is_read_without_error() {
    // 8.9: „fehlende Samples ergeben Qualitaet `Stale`" — ein kuerzeres Array
    // ist der Normalfall, kein Lesefehler des Stimulus.
    let trace = simulate(OVERSAMPLED, "t=0 in i_dut [0.5 A, 1.0 A]\n", 2);
    assert!(trace.contains("t=0 out n 2\n"), "zwei Samples: {trace}");
}

#[test]
fn a_loop_walks_the_samples() {
    // 8.9: „`for x in i_dut:` — beschraenkt durch N".
    let trace = simulate(
        "\
input i_dut : samples<float[A] in 0.0 A .. 3.0 A, 4> @ hw(\"daq1/ai2\") with rate = 4 kHz

machine watch:
    initial RUN
    state RUN:
        loop:
            for x in i_dut:
                alert x > 1.5 A, \"current spike\"
",
        "t=0 in i_dut [0.5 A, 1.0 A, 2.0 A, 1.2 A]\nt=1 in i_dut [0.1 A, 0.2 A, 0.3 A, 0.4 A]\n",
        3,
    );
    assert!(trace.contains("t=0 alert watch on \"current spike\""), "der Ausreisser meldet sich: {trace}");
    assert!(trace.contains("t=1 alert watch off \"current spike\""), "und verstummt wieder: {trace}");
}
