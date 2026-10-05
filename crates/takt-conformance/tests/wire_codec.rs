//! Das Drahtformat der Records in beiden Implementierungen (3.7, Satz
//! 9.4.4): `decode` und `encode`, hier mit einem Feld, dessen Laenge ein
//! vorangehendes Feld traegt (FB-413).

mod common;

use takt_interp::{RunOptions, Trace};
use takt_llvm::toolchain::Clang;

const HEAD: &str = "\
system:
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
        "{name}:\n{}\n--- Interpreter ---\n{interpreted}\n--- nativ ---\n{native}",
        list.join("\n")
    );
    Some(interpreted)
}

#[test]
fn a_length_prefixed_field_decodes_and_encodes_alike() {
    let Some(trace) = agree(
        "\
record Tlv layout little:
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
    assert!(trace.contains("out len_out 3\n") && trace.contains("out size_out 5\n"), "{trace}");
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

#[test]
fn decode_rejects_an_unknown_discriminant_and_a_value_out_of_range() {
    // 3.7: `none` bei einer Range-Verletzung eines Feldes; ein Enum im
    // Draht trifft eine deklarierte Diskriminante (FB-425).
    let Some(trace) = agree(
        "\
enum Kind layout u8: A = 1, B = 5

record Msg layout little:
    kind  : Kind
    level : u8 in 0..9

output bad_kind  : bool @ hw(\"o/k\") with safe = true
output bad_level : bool @ hw(\"o/l\") with safe = true
output good      : bool @ hw(\"o/g\") with safe = false
output is_b      : bool @ hw(\"o/b\") with safe = false

machine m:
    var b1 : bytes<4> = default
    var b2 : bytes<4> = default
    var b3 : bytes<4> = default
    var ok : bool = false
    initial RUN

    state RUN:
        enter:
            ok = b1.push(3)
            ok = b1.push(2)
            ok = b2.push(1)
            ok = b2.push(12)
            ok = b3.push(5)
            ok = b3.push(4)
        loop:
            bad_kind = Msg.decode(b1).valid
            bad_level = Msg.decode(b2).valid
            good = Msg.decode(b3).valid
            is_b = Msg.decode(b3).or(default).kind == B
",
        "decode_checks",
        1,
    ) else {
        return;
    };
    for want in ["out bad_kind false\n", "out bad_level false\n", "out good true\n", "out is_b true\n"] {
        assert!(trace.contains(want), "`{}` fehlt:\n{trace}", want.trim());
    }
}

#[test]
fn decode_rejects_a_buffer_one_byte_short_of_the_record() {
    // 3.7: `none` bei zu kurzem Puffer. Der Record ist sieben Byte lang;
    // sechs sind zu wenig, sieben und acht genug (SEM2-026).
    let Some(trace) = agree(
        "\
record Frame layout little:
    a : u8
    b : u16
    c : u32

output six   : bool @ hw(\"o/six\")   with safe = true
output seven : bool @ hw(\"o/seven\") with safe = false
output eight : bool @ hw(\"o/eight\") with safe = false
output c_out : u32  @ hw(\"o/c\")     with safe = 0

machine m:
    var b6 : bytes<8> = default
    var b7 : bytes<8> = default
    var b8 : bytes<8> = default
    var ok : bool = false
    initial RUN

    state RUN:
        enter:
            for i in range(8):
                if i < 6:
                    ok = b6.push((i + 1) as u8)
                if i < 7:
                    ok = b7.push((i + 1) as u8)
                ok = b8.push((i + 1) as u8)
        loop:
            six = Frame.decode(b6).valid
            seven = Frame.decode(b7).valid
            eight = Frame.decode(b8).valid
            c_out = Frame.decode(b7).or(default).c
",
        "short_by_one",
        1,
    ) else {
        return;
    };
    for want in ["out six false\n", "out seven true\n", "out eight true\n", "out c_out 117835012\n"] {
        assert!(trace.contains(want), "`{}` fehlt:\n{trace}", want.trim());
    }
}

#[test]
fn a_bit_field_across_a_byte_boundary_follows_the_byte_order() {
    // 3.7 Ueber Bytegrenzen (FB-128): Die Bitnummern zaehlen im Wert des
    // Traegers, `layout` liest ihn aus den Bytes. `3A BC` ist unter `big`
    // 0x3ABC (chan 3, value 0xABC), unter `little` 0xBC3A (chan 0xB, value
    // 0xC3A); `encode` schreibt in derselben Ordnung zurueck. Dazu die
    // Raender: alle Bits gesetzt, nur das oberste Nibble, ein Wert, der
    // genau ueber die Grenze reicht (0x801), und ein Schreiben von `value`,
    // das `chan` stehen laesst.
    let Some(trace) = agree(
        "\
record Big layout big:
    raw : u16 with bits:
        chan  : u8  at 12..15
        value : u16 at 0..11

record Little layout little:
    raw : u16 with bits:
        chan  : u8  at 12..15
        value : u16 at 0..11

output big_chan     : u8  @ hw(\"o/bc\")  with safe = 0
output big_value    : u16 @ hw(\"o/bv\")  with safe = 0
output little_chan  : u8  @ hw(\"o/lc\")  with safe = 0
output little_value : u16 @ hw(\"o/lv\")  with safe = 0
output big_e0       : u8  @ hw(\"o/b0\")  with safe = 0
output big_e1       : u8  @ hw(\"o/b1\")  with safe = 0
output little_e0    : u8  @ hw(\"o/l0\")  with safe = 0
output little_e1    : u8  @ hw(\"o/l1\")  with safe = 0
output top_big      : u8  @ hw(\"o/tb\")  with safe = 0
output top_little   : u16 @ hw(\"o/tl\")  with safe = 0
output max_e0       : u8  @ hw(\"o/m0\")  with safe = 0
output max_e1       : u8  @ hw(\"o/m1\")  with safe = 0
output cross_big    : u8  @ hw(\"o/xb\")  with safe = 0
output cross_little : u8  @ hw(\"o/xl\")  with safe = 0
output kept_chan    : u8  @ hw(\"o/kc\")  with safe = 0
output kept_raw     : u16 @ hw(\"o/kr\")  with safe = 0

machine m:
    var wire : bytes<2> = default
    var top : bytes<2> = default
    var ok : bool = false
    initial RUN

    state RUN:
        enter:
            ok = wire.push(0x3A)
            ok = wire.push(0xBC)
            ok = top.push(0xF0)
            ok = top.push(0x00)
        loop:
            var b = Big.decode(wire).or(default)
            var l = Little.decode(wire).or(default)
            big_chan = b.raw.chan
            big_value = b.raw.value
            little_chan = l.raw.chan
            little_value = l.raw.value
            var be = b.encode()
            var le = l.encode()
            big_e0 = be[0]
            big_e1 = be[1]
            little_e0 = le[0]
            little_e1 = le[1]
            top_big = Big.decode(top).or(default).raw.chan
            top_little = Little.decode(top).or(default).raw.value
            var full : Big = default
            full.raw.chan = 15
            full.raw.value = 4095
            var fe = full.encode()
            max_e0 = fe[0]
            max_e1 = fe[1]
            var cross : Big = default
            cross.raw.value = 0x801
            cross_big = cross.encode()[0]
            var lcross : Little = default
            lcross.raw.value = 0x801
            cross_little = lcross.encode()[1]
            b.raw.value = 7
            kept_chan = b.raw.chan
            kept_raw = b.raw
",
        "bytegrenzen",
        1,
    ) else {
        return;
    };
    for want in [
        "out big_chan 3\n",
        "out big_value 2748\n",
        "out little_chan 11\n",
        "out little_value 3130\n",
        "out big_e0 58\n",
        "out big_e1 188\n",
        "out little_e0 58\n",
        "out little_e1 188\n",
        "out top_big 15\n",
        "out top_little 240\n",
        "out max_e0 255\n",
        "out max_e1 255\n",
        "out cross_big 8\n",
        "out cross_little 8\n",
        "out kept_chan 3\n",
        "out kept_raw 12295\n",
    ] {
        assert!(trace.contains(want), "`{}` fehlt:\n{trace}", want.trim());
    }
}
