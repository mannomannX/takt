//! Dominanz durch Kontrollfluss (3.5, 3.8): ein Zweig, der unter
//! `not x.valid` den Block immer verlaesst, bewacht den Rest wie
//! `check x.valid`.

use takt_diag::Policy;
use takt_sema::{Build, Options};

const HEAD: &str = "system:
    language = 1
    tick = 1 ms

record Head layout little:
    n : u8

";

fn diagnostics(body: &str) -> Vec<String> {
    let src = format!("{HEAD}{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
    takt_sema::compile(&src, &options).diagnostics.iter().map(|d| format!("{d}")).collect()
}

fn unguarded(d: &[String]) -> bool {
    d.iter().any(|d| d.contains("ohne `.valid`"))
}

#[test]
fn an_early_return_dominates_the_rest_of_the_block() {
    let d = diagnostics(
        "fn first(b: bytes<4>) -> u8:
    var h : Head? = Head.decode(b)
    if not h.valid:
        return 0
    return h.n
",
    );
    assert!(!unguarded(&d), "{d:?}");
}

#[test]
fn an_early_break_dominates_the_rest_of_the_loop_body() {
    let d = diagnostics(
        "fn sum(b: bytes<4>) -> int:
    var s : int in 0..1024 = 0
    for i in range(4):
        var h : Head? = Head.decode(b[i..i + 1])
        if not h.valid:
            break
        s = s + (h.n as int)
    return s
",
    );
    assert!(!unguarded(&d), "{d:?}");
}

#[test]
fn a_conditional_exit_does_not_dominate() {
    let d = diagnostics(
        "fn first(b: bytes<4>, flag: bool) -> u8:
    var h : Head? = Head.decode(b)
    if not h.valid:
        if flag:
            return 0
    return h.n
",
    );
    assert!(unguarded(&d), "{d:?}");
}

#[test]
fn an_assignment_after_the_guard_ends_the_dominance() {
    // 3.8: Die Dominanz gilt dem Wert, den der Guard sah. Ein neuer Wert
    // kann wieder fehlen; die Warnung kehrt zurueck.
    let d = diagnostics(
        "fn first(b: bytes<4>, b2: bytes<4>) -> u8:
    var h : Head? = Head.decode(b)
    if not h.valid:
        return 0
    h = Head.decode(b2)
    return h.n
",
    );
    assert!(unguarded(&d), "{d:?}");
}

#[test]
fn an_assignment_in_one_branch_ends_the_dominance_after_the_join() {
    let d = diagnostics(
        "fn first(b: bytes<4>, b2: bytes<4>, flag: bool) -> u8:
    var h : Head? = Head.decode(b)
    if not h.valid:
        return 0
    if flag:
        h = Head.decode(b2)
    return h.n
",
    );
    assert!(unguarded(&d), "{d:?}");
}

#[test]
fn a_new_guard_after_the_assignment_dominates_again() {
    let d = diagnostics(
        "fn first(b: bytes<4>, b2: bytes<4>) -> u8:
    var h : Head? = Head.decode(b)
    if not h.valid:
        return 0
    h = Head.decode(b2)
    if not h.valid:
        return 1
    return h.n
",
    );
    assert!(!unguarded(&d), "{d:?}");
}

#[test]
fn a_break_does_not_dominate_beyond_its_loop() {
    // Hinter der Schleife kann sie ohne `break` geendet haben.
    let d = diagnostics(
        "fn last(b: bytes<4>) -> u8:
    var h : Head? = Head.decode(b)
    for i in range(4):
        if not h.valid:
            break
    return h.n
",
    );
    assert!(unguarded(&d), "{d:?}");
}

#[test]
fn an_else_branch_exit_is_not_taken_as_dominance() {
    // Konservativ: nur `if not x.valid: <verlassen>` traegt die Dominanz.
    let d = diagnostics(
        "fn first(b: bytes<4>) -> u8:
    var h : Head? = Head.decode(b)
    if h.valid:
        pass
    else:
        return 0
    return h.n
",
    );
    assert!(unguarded(&d), "{d:?}");
}
