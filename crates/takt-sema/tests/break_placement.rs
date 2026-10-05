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
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
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

#[test]
fn break_outside_a_for_loop_is_an_error_everywhere() {
    // 4.4: `break` beendet eine `for`-Schleife; jeder andere Ort ist genau
    // ein Fehler SC-8.
    // 2.3 (`state_body`): `enter` vor `loop`, `on` und `exit` dahinter.
    let machine = |block: &str, body: &str| {
        let (before, after) = if block == "enter:" {
            (format!("        {block}\n{body}"), String::new())
        } else {
            (String::new(), format!("        {block}\n{body}"))
        };
        format!("machine m:\n    initial RUN\n    state RUN:\n{before}        loop:\n            n = 1\n{after}")
    };
    for (what, src) in [
        ("loop", "machine m:\n    initial RUN\n    state RUN:\n        loop:\n            n = 1\n            break\n".to_string()),
        ("enter", machine("enter:", "            break\n")),
        ("exit", machine("exit:", "            break\n")),
        (
            "Sequenz",
            "machine m:\n    initial RUN\n    state RUN:\n        sequence:\n            wait 1 ms\n            break\n"
                .to_string(),
        ),
        ("if im Handler", machine("on rx as e:", "            if e.data > 3:\n                break\n")),
        (
            "Funktionsrumpf",
            "fn f(x: int) -> int:\n    break\n    return x\n\n".to_string() + &machine("enter:", "            n = f(1)\n"),
        ),
        (
            "generische Funktion, aufgerufen in einer Schleife",
            "fn g[type T: numeric](x: T) -> T:\n    break\n    return x\n\n".to_string()
                + &machine("enter:", "            for i in range(2):\n                n = g(i) % 256\n"),
        ),
    ] {
        let e = errors(&src);
        assert_eq!(e.len(), 1, "{what}: {e:?}");
        assert!(e[0].contains("[SC-8]") && e[0].contains("`break` nur in `for` (4.4)"), "{what}: {e:?}");
    }
}

#[test]
fn break_ends_a_for_loop_inside_a_handler_and_over_a_constant_range() {
    let e = errors(
        "const K : int = 4

machine m:
    initial RUN
    state RUN:
        loop:
            for i in range(K):
                if i > 2:
                    break
                n = i
        on rx as e:
            for i in range(3):
                if i > (e.data as int):
                    break
                n = i
",
    );
    assert!(e.is_empty(), "{e:?}");
}
