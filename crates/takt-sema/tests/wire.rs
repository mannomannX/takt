//! Drahtformat der Records (Referenz 3.7, Pruefung 46): Byteplan, Roundtrip
//! `decode(encode(x)) == x`, und die Faelle, in denen `decode` `none` liefert.

use takt_diag::Policy;
use takt_interp::{RunOptions, Trace, run};
use takt_mir::Program;
use takt_sema::{Build, Options};

const HEAD: &str = "system:\n    language = 1\n    tick = 1 ms\n\n";

fn compile(body: &str) -> Program {
    let src = format!("{HEAD}{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "unerwartete Fehler:\n{}", errors.join("\n"));
    out.program.expect("Programm")
}

fn errors(body: &str) -> String {
    let src = format!("{HEAD}{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
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

/// 3.9: `append` haengt eine ganze Folge an; die Kapazitaeten von Quelle und
/// Ziel duerfen sich unterscheiden.
#[test]
fn append_copies_a_whole_sequence() {
    let trace = simulate(
        "\
output len : int in 0..99 @ hw(\"o/len\") with safe = 0

machine m:
    var src : bytes<4> = default
    var dst : bytes<64> = default
    initial RUN
    state RUN:
        enter:
            src.push(1)
            src.push(2)
            src.push(3)
            dst.push(9)
            dst.append(src)
            len = dst.len
        after 3 ms: -> RUN
",
        1,
    );
    assert!(trace.contains("out len 4"), "1 + 3 Byte: {trace}");
}

/// 3.9: Passt die Quelle nicht vollstaendig, bleibt das Ziel unveraendert —
/// ein Teilanhang liesse einen halben Rahmen im Puffer zurueck.
#[test]
fn append_is_all_or_nothing() {
    let trace = simulate(
        "\
output len  : int in 0..99 @ hw(\"o/len\")  with safe = 0
output voll : bool         @ hw(\"o/voll\") with safe = false

machine m:
    var src : bytes<8> = default
    var dst : bytes<4> = default
    var ok  : bool = false
    initial RUN
    state RUN:
        enter:
            src.push(1)
            src.push(2)
            src.push(3)
            src.push(4)
            src.push(5)
            dst.push(9)
            ok   = dst.append(src)
            voll = not ok
            len  = dst.len
        after 3 ms: -> RUN
",
        1,
    );
    assert!(trace.contains("out voll true"), "der Anhang scheitert: {trace}");
    assert!(trace.contains("out len 1"), "das Ziel bleibt unveraendert: {trace}");
}

/// `append` traegt auch auf `vec<T, N>`; der Elementtyp muss passen, die
/// Kapazitaet nicht.
#[test]
fn append_works_on_vectors_too() {
    let trace = simulate(
        "\
output len : int in 0..99 @ hw(\"o/len\") with safe = 0

machine m:
    var a : vec<int, 4> = default
    var b : vec<int, 8> = default
    initial RUN
    state RUN:
        enter:
            a.push(7)
            a.push(8)
            b.push(1)
            b.append(a)
            len = b.len
        after 3 ms: -> RUN
",
        1,
    );
    assert!(trace.contains("out len 3"), "1 + 2 Elemente: {trace}");
}

/// Der Elementtyp muss passen, und `append` ist eine Anweisung (4.4).
#[test]
fn append_rejects_a_foreign_element_type_and_nesting() {
    let wrong = errors(
        "\
output v : bool @ hw(\"o/v\") with safe = false

machine m:
    var a : vec<bool, 4> = default
    var b : vec<int, 8> = default
    initial RUN
    state RUN:
        enter:
            b.append(a)
        after 3 ms: -> RUN
",
    );
    assert!(wrong.contains("`append` erwartet `vec<int, …>`"), "Elementtyp: {wrong}");

    let nested = errors(
        "\
output v : bool @ hw(\"o/v\") with safe = false

machine m:
    var a : bytes<4> = default
    var b : bytes<8> = default
    initial RUN
    state RUN:
        loop:
            if b.append(a):
                v = true
",
    );
    assert!(nested.contains("nur als Anweisung"), "keine Verschachtelung (4.4): {nested}");
}

/// TLV mit Laengenfeld (3.7, FB-413): Das Feld ist im Draht so lang, wie
/// das Laengenfeld sagt, die Felder danach ruecken nach; `decode` liefert
/// `none` ueber der Obergrenze und bei zu kurzem Puffer, `encode` setzt die
/// Laenge aus dem Wert und kuerzt die Ausgabe.
const TLV: &str = "\
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
";

#[test]
fn a_length_prefixed_field_is_as_long_as_its_length_field_says() {
    let trace = simulate(TLV, 0);
    for want in ["out len_out 3\n", "out tail_out 85\n", "out over false\n", "out short false\n"] {
        assert!(trace.contains(want), "decode: `{}` fehlt:\n{trace}", want.trim());
    }
    for want in ["out size_out 5\n", "out n_out 2\n", "out after_out 9\n"] {
        assert!(trace.contains(want), "encode: `{}` fehlt:\n{trace}", want.trim());
    }
}

/// Der Partitionseintrag aus 3.7 (`corpus-try/ref/3_7_02.takt`) in einer
/// Byte-Reihenfolge, dazu ein Record mit `offset`-Luecke und `align`.
fn partition_program(order: &str) -> String {
    format!(
        "\
enum ImageType layout u8: APP = 0x00, DATA = 0x01, BOOT = 0x02
record PartitionEntry layout {order}, align = 4:
    magic   : u16 = 0x50AA
    kind    : ImageType
    subtype : u8
    offset  : u32[B]
    size    : u32[B]
    label   : [16] u8
    flags   : u32 with bits:
        encrypted : bool at 0
        readonly  : bool at 1
        level     : u8 at 4..7
    _ : [4] u8

record Gapped layout {order}, align = 8:
    a : u8
    b : u16 offset = 4

output entry   : [36] u8 @ hw(\"o/entry[0:36]\") with safe = default
output gap     : [8] u8  @ hw(\"o/gap[0:8]\")    with safe = default
output sizes   : [2] int in 0..64 @ hw(\"o/sizes[0:2]\") with safe = [0, 0]
output back    : bool @ hw(\"o/back\") with safe = false

machine m:
    var e : PartitionEntry = default
    var g : Gapped = default
    var eb : [36] u8 = default
    var gb : [8] u8 = default
    initial RUN
    state RUN:
        loop:
            e.kind = BOOT
            e.subtype = 7
            e.offset = 0x12000 B
            e.size = 0x8000 B
            e.label[0] = 0x41
            e.label[15] = 0x5A
            e.flags.encrypted = true
            e.flags.level = 5
            g.a = 0x11
            g.b = 0xBEEF
            var x = e.encode()
            var y = g.encode()
            sizes = [x.len, y.len]
            for i in range(36):
                eb[i] = x[i]
            for i in range(8):
                gb[i] = y[i]
            entry = eb
            gap = gb
            var d = PartitionEntry.decode(x).or(default)
            var h = Gapped.decode(y).or(default)
            back = d.kind == BOOT and d.subtype == 7 and d.offset == 0x12000 B and d.size == 0x8000 B and d.label[0] == 0x41 and d.label[15] == 0x5A and d.flags.encrypted and not d.flags.readonly and d.flags.level == 5 and h.a == 0x11 and h.b == 0xBEEF
"
    )
}

/// 3.7: Der Byteplan des Partitionseintrags in beiden Reihenfolgen —
/// Konstantenfeld, Enum mit Drahtbreite, Einheitenfelder, Array fester
/// Laenge, Bitfelder im Traegerwort, Padding `_` als Nullen —, eine
/// `offset`-Luecke aus Nullen und `align` auf die Gesamtlaenge; `decode`
/// liefert jedes Feld zurueck.
#[test]
fn the_partition_entry_of_3_7_round_trips_in_both_byte_orders() {
    let word = |v: u32, big: bool| if big { v.to_be_bytes() } else { v.to_le_bytes() };
    for (order, big) in [("little", false), ("big", true)] {
        let mut entry: Vec<u8> = Vec::new();
        entry.extend(if big { 0x50AAu16.to_be_bytes() } else { 0x50AAu16.to_le_bytes() });
        entry.extend([0x02, 7]);
        entry.extend(word(0x12000, big));
        entry.extend(word(0x8000, big));
        let mut label = [0u8; 16];
        label[0] = 0x41;
        label[15] = 0x5A;
        entry.extend(label);
        entry.extend(word(1 | (5 << 4), big));
        entry.extend([0; 4]);
        let b = if big { 0xBEEFu16.to_be_bytes() } else { 0xBEEFu16.to_le_bytes() };
        let gap = [0x11, 0, 0, 0, b[0], b[1], 0, 0];
        let list = |bytes: &[u8]| bytes.iter().map(u8::to_string).collect::<Vec<_>>().join(", ");

        let t = simulate(&partition_program(order), 1);
        for line in [
            format!("t=0 out entry [{}]\n", list(&entry)),
            format!("t=0 out gap [{}]\n", list(&gap)),
            "t=0 out sizes [36, 8]\n".to_string(),
            "t=0 out back true\n".to_string(),
        ] {
            assert!(t.contains(&line), "{order}: `{}` fehlt:\n{t}", line.trim_end());
        }
    }
}
