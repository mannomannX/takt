//! `break` beendet eine `for`-Schleife (4.4) — sonst nichts: in einem
//! Handler hatte es keinen Sinn und liess den Interpreter mit einem
//! Sprung ohne Schleife zurueck (FB-187).

use takt_diag::Policy;
use takt_sema::{Build, Options};

const HEAD: &str = "system:
    language = 1
    tick = 1 ms

input  rx  : stream<u8> @ hw(\"u/rx\") with max_rate = 1 kHz, capacity = 8
output n   : int in 0..255 @ hw(\"o/n\") with safe = 0
output rx_sim : stream<u8> @ sim(\"u/rx\")

";

fn errors(body: &str) -> Vec<String> {
    let src = format!("{HEAD}{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect()
}

#[test]
fn break_ends_a_for_loop_over_a_stream() {
    let e = errors(
        "machine m:
    initial RUN
    state RUN:
        loop:
            for e in rx:
                n = e.data as int
                break
",
    );
    assert!(e.is_empty(), "{e:?}");
}

#[test]
fn break_in_a_handler_is_an_error() {
    let e = errors(
        "machine m:
    initial RUN
    state RUN:
        on rx as e:
            n = e.data as int
            break
",
    );
    assert!(e.iter().any(|d| d.contains("`break` nur in `for`")), "{e:?}");
}
