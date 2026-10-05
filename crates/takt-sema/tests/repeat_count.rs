//! `repeat n:` (6.2) mit einer Zahl aus einem `param`: Der Koerper laeuft
//! genau n-mal, bei n = 0 gar nicht (SYN-032).

use takt_diag::Policy;
use takt_interp::{RunOptions, Trace, run};
use takt_sema::{Build, Options};

/// Wie oft der Koerper von `repeat P` mit `P = p` laeuft, abgelesen an
/// einem Zaehler-Output, und ob die Sequenz danach weiterlaeuft.
fn passes(p: i64) -> (i64, bool) {
    let src = format!(
        "system:
    language = 1
    tick     = 10 ms

param P : int in 0..5 = {p}

output n    : int in 0..9 @ hw(\"o/n\")    with safe = 0
output done : bool        @ hw(\"o/done\") with safe = false

machine m:
    var k : int in 0..9 = 0
    initial RUN
    state RUN:
        sequence:
            repeat P:
                k = min(k + 1, 9)
                n = k
                wait 10 ms
            done = true
            -> END
    state END:
        when false: -> RUN
"
    );
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{errors:?}");
    let program = out.program.expect("Programm");
    let t =
        run(&program, &Trace::default(), &RunOptions { ticks: 20, ..Default::default() }).expect("Lauf").trace.render();
    let last = t.lines().rev().find_map(|l| l.split_once(" out n ").and_then(|(_, v)| v.parse().ok())).unwrap_or(0);
    (last, t.contains("out done true"))
}

/// 6.2: Die Zahl der Durchlaeufe ist die Zahl, auch aus einem `param`.
#[test]
fn a_param_counts_the_passes_of_a_repeat() {
    assert_eq!(passes(3), (3, true));
    assert_eq!(passes(1), (1, true));
}

/// 6.2, 4.1: Eine Schranke 0 heisst kein Durchlauf — wie `range(0)`.
#[test]
fn a_repeat_of_zero_does_not_run_its_body() {
    assert_eq!(passes(0), (0, true));
}
