//! Das Drahtformat der Records in beiden Implementierungen (3.7, Satz
//! 9.4.4): `decode` und `encode`, hier mit einem Feld, dessen Laenge ein
//! vorangehendes Feld traegt (FB-413).

mod common;

use takt_interp::{RunOptions, Trace};
use takt_llvm::toolchain::Clang;

const HEAD: &str = "system:
    language = 1
    tick     = 1 ms

";

/// Beide Implementierungen liefern dieselben Outputs; zurueck kommt der
/// Trace des Interpreters, `None` ohne clang.
fn agree(body: &str, name: &str, ticks: u64) -> Option<String> {
    let path = common::clang_path()?;
    let src = format!("{HEAD}{body}");
    let options = takt_sema::Options {
        policy: takt_diag::Policy::default(),
        build: takt_sema::Build::Sim,
        profile: None,
        ..Default::default()
    };
    let out = takt_sema::compile(&src, &options);
    assert!(!out.has_errors(), "{name}: {:?}", out.diagnostics);
    let p = out.program.expect("Programm");
    let interpreted = takt_interp::run(&p, &Trace::default(), &RunOptions { ticks, ..Default::default() })
        .expect("Lauf")
        .trace
        .render();
    let native = common::run_native_all(&Clang::At(path), &p, name, ticks).expect("nativ");
    let diffs = takt_conformance::compare(&interpreted, &native);
    let list: Vec<String> = diffs.iter().take(6).map(|d| format!("  {d}")).collect();
    assert!(
        diffs.is_empty(),
        "{name}:
{}
--- Interpreter ---
{interpreted}
--- nativ ---
{native}",
        list.join(
            "
"
        )
    );
    Some(interpreted)
}

#[test]
fn a_length_prefixed_field_decodes_and_encodes_alike() {
    let Some(trace) = agree(
        "record Tlv layout little:
    kind  : u8
    n     : u8
    value : bytes<8> with len = n
    tail  : u8

output len_out  : int in 0..99  @ hw(\"o/len\")  with safe = 0
output tail_out : u8            @ hw(\"o/tail\") with safe = 0
output size_out : int in 0..99  @ hw(\"o/size\") with safe = 0
output n_out    : u8            @ hw(\"o/n\")    with safe = 0
output after_out : u8           @ hw(\"o/after\") with safe = 0
output over     : bool          @ hw(\"o/over\") with safe = true
output short    : bool          @ hw(\"o/short\") with safe = true

machine m:
    var b : bytes<16> = default
    var c : bytes<16> = default
    var d : bytes<4> = default
    var t : Tlv = default
    var ok : bool = false
    initial RUN

    state RUN:
        enter:
            ok = b.push(7)
            ok = b.push(3)
            ok = b.push(97)
            ok = b.push(98)
            ok = b.push(99)
            ok = b.push(85)
            ok = c.push(7)
            ok = c.push(9)
            for i in range(12):
                ok = c.push(0)
            ok = d.push(7)
            ok = d.push(3)
            ok = d.push(97)
            ok = d.push(98)
            t.kind = 1
            t.tail = 9
            ok = t.value.push(120)
            ok = t.value.push(121)
        loop:
            var got = Tlv.decode(b)
            len_out = got.or(default).value.len
            tail_out = got.or(default).tail
            over = Tlv.decode(c).valid
            short = Tlv.decode(d).valid
            var e = t.encode()
            size_out = e.len
            var back = Tlv.decode(e)
            n_out = back.or(default).n
            after_out = back.or(default).tail
",
        "length_prefixed",
        1,
    ) else {
        return;
    };
    assert!(
        trace.contains(
            "out len_out 3
"
        ) && trace.contains(
            "out size_out 5
"
        ),
        "{trace}"
    );
}

#[test]
fn two_length_prefixed_fields_shift_what_follows_and_align_rounds() {
    // Plan: a@0, x@1 (4), b@5, y@7 (4), z@11 — Ende 12. Im Draht sind x
    // und y zwei und ein Byte lang: Die Luecke 5 laesst z an 6 liegen, und
    // `align = 4` rundet die Laenge 7 auf 8 — der Puffer traegt darum ein
    // Fuellbyte, ohne es waere er zu kurz.
    let Some(trace) = agree(
        "\
record Two layout big, align = 4:
    a : u8
    x : bytes<4> with len = a
    b : u16
    y : bytes<4> with len = b
    z : u8

output x_len : int in 0..9   @ hw(\"o/x\") with safe = 0
output y_len : int in 0..9   @ hw(\"o/y\") with safe = 0
output z_out : u8            @ hw(\"o/z\") with safe = 0
output size  : int in 0..99  @ hw(\"o/s\") with safe = 0

machine m:
    var b : bytes<16> = default
    var ok : bool = false
    initial RUN

    state RUN:
        enter:
            ok = b.push(2)
            ok = b.push(112)
            ok = b.push(113)
            ok = b.push(0)
            ok = b.push(1)
            ok = b.push(114)
            ok = b.push(119)
            ok = b.push(0)
        loop:
            var got = Two.decode(b).or(default)
            x_len = got.x.len
            y_len = got.y.len
            z_out = got.z
            size = got.encode().len
",
        "two_length_prefixed",
        1,
    ) else {
        return;
    };
    for want in ["out x_len 2\n", "out y_len 1\n", "out z_out 119\n", "out size 8\n"] {
        assert!(trace.contains(want), "`{}` fehlt:\n{trace}", want.trim());
    }
}
