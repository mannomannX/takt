//! Pruefung 16 (8.8): Die Hoechstlaenge eines Formatstrings deckt jeden
//! Wert seiner Platzhalter, auch `<invalid>` (3.5); gekuerzt wird nie.

use takt_diag::Policy;
use takt_interp::{RunOptions, Trace, run};
use takt_sema::{Build, Options};

/// Der Trace eines Programms, das je Tick `log "<text>"` schreibt.
fn logged(decls: &str, text: &str, stimulus: &str) -> String {
    let src = format!(
        "system:
    language = 1
    tick     = 10 ms

{decls}
output x : int in 0..9 @ hw(\"o/x\") with safe = 0

machine m:
    initial RUN
    state RUN:
        loop:
            log \"{text}\"
            x = 1
"
    );
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{errors:?}");
    let stimulus = Trace::parse(stimulus).expect("Stimulus");
    let p = out.program.expect("Programm");
    run(&p, &stimulus, &RunOptions { ticks: 1, ..Default::default() }).expect("Lauf").trace.render()
}

/// Ein ungueltiger `bool` steht ganz als `<invalid>` da, nicht als `<inva`.
#[test]
fn an_invalid_bool_is_not_cut() {
    let decls = "input  b     : bool @ hw(\"d/b\")\noutput b_sim : bool @ sim(\"d/b\")";
    let t = logged(decls, "b={b}", "t=0 in b true\nt=1 in b bad reason=Driver\n");
    assert!(t.contains("t=0 log m \"b=true\"\n"), "{t}");
    assert!(t.contains("t=1 log m \"b=<invalid>\"\n"), "{t}");
}

/// Ebenso ein Enum mit kurzen Variantennamen.
#[test]
fn an_invalid_enum_is_not_cut() {
    let decls = "enum Mode: A, B\ninput  mode     : Mode @ hw(\"d/mode\")\noutput mode_sim : Mode @ sim(\"d/mode\")";
    let t = logged(decls, "{mode}", "t=0 in mode A\nt=1 in mode bad reason=Driver\n");
    assert!(t.contains("t=0 log m \"A\"\n"), "{t}");
    assert!(t.contains("t=1 log m \"<invalid>\"\n"), "{t}");
}

/// Die Breite eines Records und eines Arrays folgt ihren Feldern und
/// Elementen: Ein Text von 30 Zeichen bleibt ganz, auch in `Rec(...)` und in
/// `[..., ...]`.
#[test]
fn records_and_arrays_are_as_wide_as_their_parts() {
    let decls = "record Rec:\n    name : str<40>";
    let text = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let src = format!(
        "system:
    language = 1
    tick     = 10 ms

{decls}
output x : int in 0..9 @ hw(\"o/x\") with safe = 0

machine m:
    var r  : Rec = default
    var xs : [2] str<40> = default
    initial RUN
    state RUN:
        loop:
            r.name = \"{text}\"
            xs[0] = \"{text}\"
            xs[1] = \"{text}\"
            log \"{{r}}\"
            log \"{{xs}}\"
            x = 1
"
    );
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{errors:?}");
    let p = out.program.expect("Programm");
    let t = run(&p, &Trace::default(), &RunOptions { ticks: 0, ..Default::default() }).expect("Lauf").trace.render();
    assert!(t.contains(&format!("t=0 log m \"Rec({text})\"\n")), "{t}");
    assert!(t.contains(&format!("t=0 log m \"[{text}, {text}]\"\n")), "{t}");
}

/// Die SC-16-Fehler eines Programms, das `text` an einen Strom aus
/// `line<cap>` sendet; `float = f32`, wenn `narrow`.
fn send_errors(narrow: bool, cap: u32, text: &str) -> Vec<String> {
    let width = if narrow { "    float    = f32\n" } else { "" };
    let src = format!(
        "system:
    language = 1
    tick     = 10 ms
{width}
stream<line<{cap}>> s with capacity = 4
output n : int in 0..9 @ hw(\"o/n\") with safe = 0

machine w:
    var x : float = 0.0
    initial RUN
    state RUN:
        loop:
            send s, \"{text}\"

machine r:
    initial RUN
    state RUN:
        on s as m:
            n = 1
"
    );
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
    takt_sema::compile(&src, &options).diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect()
}

/// 3.9 (fuenfte Runde): Kuerzeste Ziffern, Exponent unter 1e-5 und ab 1e16
/// (`f64`) bzw. 1e7 (`f32`). Ein `f64` braucht hoechstens 24 Zeichen
/// (`-1.2345678901234567e-308`), ein `f32` hoechstens 16
/// (`-0.0000123456789`).
#[test]
fn a_float_placeholder_is_as_wide_as_its_longest_text() {
    for (narrow, most) in [(false, 24), (true, 16)] {
        let fits = send_errors(narrow, most, "{x}");
        assert!(fits.is_empty(), "{narrow} {most}: {fits:?}");
        let short = send_errors(narrow, most - 1, "{x}");
        assert!(short.len() == 1 && short[0].contains("SC-16"), "{narrow} {most}: {short:?}");
    }
}

/// `{x:.N}` in derselben Form mit N Nachkommastellen: Vorzeichen, die
/// Stellen vor dem Punkt (16 fuer `f64`, 7 fuer `f32`), Punkt, N — also
/// 18 + N bzw. 9 + N; die Exponentform (`-1.234e-308`) ist kuerzer.
#[test]
fn a_precision_adds_its_digits_to_the_widest_form() {
    for (narrow, most) in [(false, 21), (true, 12)] {
        assert!(send_errors(narrow, most, "{x:.3}").is_empty(), "{narrow}");
        let short = send_errors(narrow, most - 1, "{x:.3}");
        assert!(short.len() == 1 && short[0].contains("SC-16"), "{narrow}: {short:?}");
    }
}
