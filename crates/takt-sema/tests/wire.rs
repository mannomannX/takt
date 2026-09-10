//! Drahtformat der Records (Referenz 3.7, Pruefung 46): Byteplan, Roundtrip
//! `decode(encode(x)) == x`, und die Faelle, in denen `decode` `none` liefert.

use takt_diag::Policy;
use takt_interp::{RunOptions, Trace, run};
use takt_mir::Program;
use takt_sema::{Build, Options};

const HEAD: &str = "system:\n    language = 1\n    tick = 1 ms\n\n";

fn compile(body: &str) -> Program {
    let src = format!("{HEAD}{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "unerwartete Fehler:\n{}", errors.join("\n"));
    out.program.expect("Programm")
}

fn errors(body: &str) -> String {
    let src = format!("{HEAD}{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect::<Vec<_>>().join("\n")
}

fn simulate(body: &str, ticks: u64) -> String {
    let program = compile(body);
    let stim = Trace::parse("").expect("leer");
    run(&program, &stim, &RunOptions { ticks, ..Default::default() }).expect("Lauf").trace.render()
}

#[test]
fn the_byte_plan_places_fields_in_declaration_order() {
    let p = compile(
        "\
record Frame layout little:
    magic : u16 = 0x50AA
    kind  : u8
    value : u32

output v : bool @ hw(\"o/v\") with safe = false

machine m:
    initial RUN
    state RUN:
        loop: pass
",
    );
    let r = p.records.iter().find(|r| r.name == "Frame").expect("Frame");
    assert_eq!(r.wire_size, Some(7), "2 + 1 + 4 Byte");
    let offsets: Vec<Option<u32>> = r.fields.iter().map(|f| f.offset).collect();
    assert_eq!(offsets, vec![Some(0), Some(2), Some(3)]);
}

#[test]
fn align_rounds_up_the_total_length() {
    // 3.7: „`align` rundet die Gesamtlaenge".
    let p = compile(
        "\
record Small layout little, align = 4:
    a : u8
    b : u8

output v : bool @ hw(\"o/v\") with safe = false

machine m:
    initial RUN
    state RUN:
        loop: pass
",
    );
    let r = p.records.iter().find(|r| r.name == "Small").expect("Small");
    assert_eq!(r.wire_size, Some(4), "2 Byte auf 4 gerundet");
}

#[test]
fn a_record_without_layout_has_no_byte_plan() {
    // 3.7: „Ohne `layout` gibt es keine Byte-Repraesentation."
    let p = compile(
        "\
record Plain:
    a : u8

output v : bool @ hw(\"o/v\") with safe = false

machine m:
    initial RUN
    state RUN:
        loop: pass
",
    );
    let r = p.records.iter().find(|r| r.name == "Plain").expect("Plain");
    assert_eq!(r.wire_size, None);
}

#[test]
fn encode_and_decode_round_trip() {
    let trace = simulate(
        "\
record Frame layout little:
    magic : u16 = 0x50AA
    kind  : u8
    value : u32

output size : int in 0..64 @ hw(\"o/s\") with safe = 0
output kind : u8           @ hw(\"o/k\") with safe = 0
output val  : u32          @ hw(\"o/v\") with safe = 0

machine m:
    var f : Frame = default
    initial RUN
    state RUN:
        loop:
            f.kind = 7
            f.value = 1234
            var bytes = f.encode()
            size = bytes.len
            var again = Frame.decode(bytes)
            if again.valid:
                kind = again.or(default).kind
                val = again.or(default).value
",
        1,
    );
    assert!(trace.contains("out size 7\n"), "{trace}");
    assert!(trace.contains("out kind 7\n"), "Roundtrip: {trace}");
    assert!(trace.contains("out val 1234\n"), "Roundtrip: {trace}");
}

#[test]
fn decode_yields_none_on_a_short_buffer() {
    // 3.7: „`none` bei zu kurzem Puffer".
    let trace = simulate(
        "\
record Frame layout little:
    magic : u16 = 0x50AA
    kind  : u8
    value : u32

output ok : bool @ hw(\"o/ok\") with safe = false

machine m:
    var short : bytes<4> = default
    initial RUN
    state RUN:
        loop:
            ok = Frame.decode(short).valid
",
        1,
    );
    assert!(trace.contains("out ok false\n"), "{trace}");
}

#[test]
fn decode_yields_none_when_a_constant_field_does_not_match() {
    // 3.7: „Konstantenfelder werden bei `decode` geprueft".
    let trace = simulate(
        "\
record Frame layout little:
    magic : u16 = 0x50AA
    kind  : u8

output ok  : bool @ hw(\"o/ok\")  with safe = false
output bad : bool @ hw(\"o/bad\") with safe = false

machine m:
    var f : Frame = default
    var wrong : bytes<3> = default
    var filled : bool = false
    initial RUN
    state RUN:
        loop:
            var good = f.encode()
            ok = Frame.decode(good).valid
            # Drei Nullbytes: die Konstante magic trifft nicht.
            filled = wrong.push(0)
            filled = wrong.push(0)
            filled = wrong.push(0)
            bad = Frame.decode(wrong).valid
",
        1,
    );
    assert!(trace.contains("out ok true\n"), "gueltige Konstante: {trace}");
    assert!(trace.contains("out bad false\n"), "falsche Konstante: {trace}");
}

#[test]
fn big_endian_differs_from_little_endian() {
    let trace = simulate(
        "\
record Le layout little:
    v : u16

record Be layout big:
    v : u16

output a : u8 @ hw(\"o/a\") with safe = 0
output b : u8 @ hw(\"o/b\") with safe = 0

machine m:
    var l : Le = default
    var g : Be = default
    initial RUN
    state RUN:
        loop:
            l.v = 0x1234
            g.v = 0x1234
            a = l.encode()[0]
            b = g.encode()[0]
",
        1,
    );
    // Little: niederwertiges Byte zuerst; Big: hoeherwertiges.
    assert!(trace.contains("out a 52\n"), "0x34 zuerst: {trace}");
    assert!(trace.contains("out b 18\n"), "0x12 zuerst: {trace}");
}

#[test]
fn overlapping_offsets_are_rejected() {
    let diags = errors(
        "\
record Overlap layout little:
    a : u32 offset = 0
    b : u32 offset = 2

output v : bool @ hw(\"o/v\") with safe = false

machine m:
    initial RUN
    state RUN:
        loop: pass
",
    );
    assert!(diags.contains("SC-46"), "{diags}");
    assert!(diags.contains("ueberlappt"), "{diags}");
}

#[test]
fn a_nested_record_needs_its_own_layout() {
    let diags = errors(
        "\
record Inner:
    a : u8

record Outer layout little:
    i : Inner

output v : bool @ hw(\"o/v\") with safe = false

machine m:
    initial RUN
    state RUN:
        loop: pass
",
    );
    assert!(diags.contains("SC-46"), "{diags}");
}

#[test]
fn encode_needs_a_layout() {
    let diags = errors(
        "\
record Plain:
    a : u8

output n : int in 0..64 @ hw(\"o/n\") with safe = 0

machine m:
    var p : Plain = default
    initial RUN
    state RUN:
        loop:
            n = p.encode().len
",
    );
    assert!(diags.contains("kein `layout`"), "{diags}");
}

#[test]
fn a_frame_is_assembled_from_layout_and_push() {
    // 3.9: `reader`/`writer` sind v1.1 (Konstantenvariablen in Generics,
    // 3.12). Bis dahin traegt `layout` dieselbe Aufgabe — dieser Test haelt
    // fest, dass der Weg wirklich reicht: Kopf kodieren, Nutzlast anhaengen,
    // wieder dekodieren.
    let trace = simulate(
        "\
record LinkHeader layout little:
    magic : u8 = 0xA5
    kind  : u8
    seq   : u8
    len   : u16

output frame_len : int in 0..99 @ hw(\"o/len\") with safe = 0
output kind_ok   : bool @ hw(\"o/ok\") with safe = false

machine m:
    initial RUN
    state RUN:
        loop:
            var h = LinkHeader(kind = 0x02, seq = 7, len = 3)
            var buf : bytes<64> = default
            var wire = h.encode()
            for i in range(64):
                if i >= wire.len:
                    break
                var put = buf.push(wire[i])
            var p1 = buf.push(0x11)
            var p2 = buf.push(0x22)
            var p3 = buf.push(0x33)
            frame_len = buf.len as int
            var back = LinkHeader.decode(buf)
            if back.valid:
                kind_ok = back.kind == 0x02
",
        2,
    );
    // Kopf sind 5 Byte (magic, kind, seq, len als u16), dazu drei Nutzbytes.
    assert!(trace.contains("t=0 out frame_len 8\n"), "Kopf plus Nutzlast: {trace}");
    assert!(trace.contains("t=0 out kind_ok true\n"), "der Rahmen liest sich zurueck: {trace}");
}
