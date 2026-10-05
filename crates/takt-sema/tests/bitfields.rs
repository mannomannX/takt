//! Benannte Bitfelder in `layout`-Records (Referenz 3.7): Lesen, Schreiben
//! und der Weg ueber `encode`/`decode`.
//!
//! Ein Bitfeld ist eine *Sicht* auf sein Traegerfeld, kein eigener Speicher:
//! Lesen wird zu `bit`/`bits`, Schreiben zu `with_bit` beziehungsweise zum
//! Einsetzen mit Maske (3.10). Damit traegt der Codec sie ohne Zutun mit.

use takt_diag::Policy;
use takt_interp::{RunOptions, Trace, run};
use takt_sema::{Build, Options};

const HEAD: &str = "system:\n    language = 1\n    tick = 1 ms\n\n";

/// Ein Statusregister, wie es ein Treiber deklariert.
const STATUS: &str = "\
record Status layout little:
    flags : u16 with bits:
        ready : bool at 0
        busy  : bool at 3
        level : u8 at 4..7

";

fn simulate(body: &str, ticks: u64) -> String {
    let src = format!("{HEAD}{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "unerwartete Fehler:\n{}", errors.join("\n"));
    let program = out.program.expect("Programm");
    let stim = Trace::parse("").expect("leer");
    run(&program, &stim, &RunOptions { ticks, ..Default::default() }).expect("Lauf").trace.render()
}

fn errors(body: &str) -> String {
    let src = format!("{HEAD}{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
    let out = takt_sema::compile(&src, &options);
    out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect::<Vec<_>>().join("\n")
}

#[test]
fn a_named_bit_reads_as_bool_and_a_field_as_its_type() {
    // 3.7: `bool at 0` liest ein einzelnes Bit, `u8 at 4..7` einen Bereich.
    let trace = simulate(
        &format!(
            "{STATUS}\
output r : bool @ hw(\"o/r\") with safe = false
output b : bool @ hw(\"o/b\") with safe = false
output l : int in 0..99 @ hw(\"o/l\") with safe = 0

machine m:
    initial RUN
    state RUN:
        loop:
            var s : Status = Status(flags = 0x0059)
            r = s.flags.ready
            b = s.flags.busy
            l = s.flags.level as int
"
        ),
        2,
    );
    // 0x59 = 0101_1001: Bit 0 und 3 gesetzt, Bits 4..7 sind 5.
    assert!(trace.contains("t=0 out r true\n"), "Bit 0: {trace}");
    assert!(trace.contains("t=0 out b true\n"), "Bit 3: {trace}");
    assert!(trace.contains("t=0 out l 5\n"), "Bits 4..7: {trace}");
}

#[test]
fn writing_a_bitfield_leaves_its_neighbours_alone() {
    // 3.7: das Bitfeld ist eine Sicht; das Schreiben setzt genau seine Bits
    // und laesst den Rest des Traegers stehen.
    let trace = simulate(
        &format!(
            "{STATUS}\
output f : int in 0..65535 @ hw(\"o/f\") with safe = 0

machine m:
    initial RUN
    state RUN:
        loop:
            var s : Status = Status(flags = 0xFFFF)
            s.flags.level = 5
            f = s.flags as int
"
        ),
        2,
    );
    // 0xFFFF mit Bits 4..7 auf 5 ist 0xFF5F.
    assert!(trace.contains("t=0 out f 65375\n"), "nur die eigenen Bits: {trace}");
}

#[test]
fn a_bit_survives_encode_and_decode() {
    // 3.7: der Codec serialisiert das Traegerfeld, die Bitfelder fahren mit.
    let trace = simulate(
        &format!(
            "{STATUS}\
output r : bool @ hw(\"o/r\") with safe = false
output l : int in 0..99 @ hw(\"o/l\") with safe = 0
output n : int in 0..99 @ hw(\"o/n\") with safe = 0

machine m:
    initial RUN
    state RUN:
        loop:
            var s : Status = Status(flags = 0)
            s.flags.ready = true
            s.flags.level = 5
            var wire = s.encode()
            n = wire.len as int
            var back = Status.decode(wire)
            if back.valid:
                r = back.flags.ready
                l = back.flags.level as int
"
        ),
        2,
    );
    assert!(trace.contains("t=0 out n 2\n"), "zwei Byte Draht: {trace}");
    assert!(trace.contains("t=0 out r true\n"), "das Bit kam zurueck: {trace}");
    assert!(trace.contains("t=0 out l 5\n"), "das Feld kam zurueck: {trace}");
}

#[test]
fn an_unknown_bitfield_names_the_declared_ones() {
    let read = errors(&format!(
        "{STATUS}\
output n : int in 0..99 @ hw(\"o/n\") with safe = 0

machine m:
    initial RUN
    state RUN:
        loop:
            var s : Status = default
            n = 1 if s.flags.nope else 0
"
    ));
    assert!(read.contains("nope"), "der Name steht in der Meldung: {read}");

    let write = errors(&format!(
        "{STATUS}\
output n : int in 0..99 @ hw(\"o/n\") with safe = 0

machine m:
    var s : Status = default
    initial RUN
    state RUN:
        loop:
            s.flags.nope = true
            n = 1
"
    ));
    assert!(write.contains("Bitfeld `nope`"), "auch beim Schreiben: {write}");
}

#[test]
fn a_bitfield_takes_only_plain_assignment() {
    // `+=` und Verwandte muessten lesen, rechnen und einsetzen; die Absicht
    // bleibt lesbarer, wenn der Rechenschritt dasteht.
    let out = errors(&format!(
        "{STATUS}\
output n : int in 0..99 @ hw(\"o/n\") with safe = 0

machine m:
    var s : Status = default
    initial RUN
    state RUN:
        loop:
            s.flags.level += 1
            n = 1
"
    ));
    assert!(out.contains("nur `=`"), "zusammengesetzte Zuweisung abgelehnt: {out}");
}

/// Ein Register mit invertierten Feldern: Rohbits stehen im Traeger.
const INVERTED: &str = "\
record Ctrl layout little:
    flags : u16 with bits:
        nrst  : bool at 0 active_low
        level : u8 at 4..7 active_low
        code  : int at 8..11 active_low

";

#[test]
fn a_multi_bit_active_low_field_reads_inverted_within_its_width() {
    // 3.7: `active_low` invertiert beim Lesen genau die Bits des Felds. Die
    // Rohbits 0b0101 eines vierbittigen Felds lesen sich als 0b1010.
    let trace = simulate(
        &format!(
            "{INVERTED}\
output l : int in 0..99 @ hw(\"o/l\") with safe = 0
output c : int in 0..99 @ hw(\"o/c\") with safe = 0
output r : bool @ hw(\"o/r\") with safe = false

machine m:
    initial RUN
    state RUN:
        loop:
            var s : Ctrl = Ctrl(flags = 0x0351)
            l = s.flags.level as int
            c = s.flags.code
            r = s.flags.nrst
"
        ),
        1,
    );
    assert!(trace.contains("t=0 out l 10\n"), "0b0101 invertiert: {trace}");
    assert!(trace.contains("t=0 out c 12\n"), "0b0011 invertiert, ohne Vorzeichen: {trace}");
    assert!(trace.contains("t=0 out r false\n"), "Rohbit 1 liest sich als false: {trace}");
}

#[test]
fn writing_an_active_low_field_stores_the_raw_bits() {
    // 3.7: Beim Schreiben invertiert die Zuweisung, der Traeger bleibt roh:
    // `nrst = true` setzt Bit 0 auf 0, `level = 10` die Bits 4..7 auf 0b0101.
    let trace = simulate(
        &format!(
            "{INVERTED}\
output f : int in 0..65535 @ hw(\"o/f\") with safe = 0
output l : int in 0..99 @ hw(\"o/l\") with safe = 0

machine m:
    initial RUN
    state RUN:
        loop:
            var s : Ctrl = Ctrl(flags = 0x0001)
            s.flags.nrst = true
            s.flags.level = 10
            f = s.flags as int
            l = s.flags.level as int
"
        ),
        1,
    );
    assert!(trace.contains("t=0 out f 80\n"), "Rohbits 0x0050: {trace}");
    assert!(trace.contains("t=0 out l 10\n"), "zurueckgelesen: {trace}");
}

#[test]
fn a_big_endian_carrier_puts_its_high_byte_first_on_the_wire() {
    // 3.7: `layout big` schreibt den Traeger mit dem hoechsten Byte zuerst;
    // die Bitfelder fahren mit und kommen ueber `decode` zurueck.
    let trace = simulate(
        "\
record Status layout big:
    flags : u16 with bits:
        ready : bool at 0
        level : u8 at 8..11

output b0 : int in 0..255 @ hw(\"o/b0\") with safe = 0
output b1 : int in 0..255 @ hw(\"o/b1\") with safe = 0
output r : bool @ hw(\"o/r\") with safe = false
output l : int in 0..99 @ hw(\"o/l\") with safe = 0

machine m:
    initial RUN
    state RUN:
        loop:
            var s : Status = Status(flags = 0)
            s.flags.ready = true
            s.flags.level = 5
            var wire = s.encode()
            b0 = wire[0] as int
            b1 = wire[1] as int
            var back = Status.decode(wire)
            if back.valid:
                r = back.flags.ready
                l = back.flags.level as int
",
        1,
    );
    assert!(trace.contains("t=0 out b0 5\n"), "0x0501, hohes Byte zuerst: {trace}");
    assert!(trace.contains("t=0 out b1 1\n"), "{trace}");
    assert!(trace.contains("t=0 out r true\n"), "{trace}");
    assert!(trace.contains("t=0 out l 5\n"), "{trace}");
}

#[test]
fn every_compound_operator_is_refused_on_a_bitfield() {
    for op in ["+=", "-=", "*=", "/="] {
        let out = errors(&format!(
            "{STATUS}\
output n : int in 0..99 @ hw(\"o/n\") with safe = 0

machine m:
    var s : Status = default
    initial RUN
    state RUN:
        loop:
            s.flags.level {op} 1
            n = 1
"
        ));
        assert!(out.contains("[SC-3]") && out.contains("Bitfelder nehmen nur `=`"), "`{op}`: {out}");
    }
}

#[test]
fn an_unknown_bitfield_lists_every_declared_one() {
    for (what, line) in [("lesen", "n = 1 if s.flags.nope else 0"), ("schreiben", "s.flags.nope = true")] {
        let out = errors(&format!(
            "{STATUS}\
output n : int in 0..99 @ hw(\"o/n\") with safe = 0

machine m:
    var s : Status = default
    initial RUN
    state RUN:
        loop:
            {line}
            n = 1
"
        ));
        assert!(out.contains("[SC-3]") && out.contains("Bitfelder: ready, busy, level"), "{what}: {out}");
    }
}

/// Ein Register mit oberstem Traegerbit, Feldern unter `u8` und `i8` und
/// einem Feld ueber die volle Traegerbreite.
const WIDE: &str = "\
record Reg layout little:
    flags : u16 with bits:
        top   : bool at 15
        level : u8 at 4..7
        delta : i8 at 8..11
    whole : u16 with bits:
        all : u16 at 0..15

";

/// Ein Programm, das `body` jeden Tick ausfuehrt; `k` ist eine Variable,
/// deren Wert der Analyse nicht feststeht.
fn writes(body: &str) -> String {
    format!(
        "{WIDE}\
output f : int in 0..65535 @ hw(\"o/f\") with safe = 0
output w : int in 0..65535 @ hw(\"o/w\") with safe = 0
output l : int in 0..99 @ hw(\"o/l\") with safe = 0
output d : int in -128..127 @ hw(\"o/d\") with safe = 0

machine m:
    fault -> SAFE
    var k : int in -128..255 = 0
    var s : Reg = default
    initial RUN
    state RUN:
        loop:
{body}            f = s.flags as int
            w = s.whole as int
            l = s.flags.level as int
            d = s.flags.delta as int
    state SAFE:
        loop:
            l = 99
"
    )
}

#[test]
fn the_top_bit_and_the_full_carrier_width_are_writable() {
    // 3.7: Bit 15 ist das oberste eines `u16`; ein Feld ueber 0..15 fasst
    // den ganzen Wertebereich.
    let trace = simulate(&writes("            s.flags.top = true\n            s.whole.all = 65535\n"), 1);
    assert!(trace.contains("t=0 out f 32768\n"), "{trace}");
    assert!(trace.contains("t=0 out w 65535\n"), "{trace}");
}

#[test]
fn a_bitfield_under_a_signed_type_holds_the_unsigned_values_of_its_bits() {
    // 3.7: `delta : i8 at 8..11` fasst 0..15.
    let trace = simulate(&writes("            s.flags.delta = 15\n            s.flags.level = 15\n"), 1);
    assert!(trace.contains("t=0 out d 15\n") && trace.contains("t=0 out l 15\n"), "{trace}");
    assert!(trace.contains("t=0 out f 4080\n"), "0x0FF0: {trace}");
}

#[test]
fn a_constant_outside_a_bitfield_is_an_error() {
    // 3.7: „ein Wert ausserhalb ist beim Schreiben ein Range-Verstoss … ein
    // Fehler, wenn er feststeht".
    for line in ["s.flags.level = 16", "s.flags.delta = -1", "s.flags.delta = 16"] {
        let e = errors(&writes(&format!("            {line}\n")));
        assert_eq!(e.lines().count(), 1, "`{line}`: {e}");
        assert!(e.contains("[SC-3]"), "`{line}`: {e}");
    }
}

#[test]
fn a_dynamic_value_outside_a_bitfield_faults_instead_of_being_masked() {
    // 3.7: sonst ein impliziter Check mit `RangeFault`, nie eine stille
    // Maske. `k = 16` passt in `u8`, nicht in `level`.
    for (k, line) in [("16", "s.flags.level = k as u8"), ("-1", "s.flags.delta = k as i8")] {
        let body = format!("            k = {k}\n            {line}\n");
        let trace = simulate(&writes(&body), 1);
        assert!(trace.contains("t=0 fault m RangeFault"), "`{line}` mit k = {k}: {trace}");
        assert!(trace.contains("t=0 out l 99\n"), "`{line}`: {trace}");
    }
}

#[test]
fn a_bool_bitfield_has_exactly_one_position() {
    // 3.7: `bool` bei einer einzelnen Position, sonst der Integer-Typ.
    let e = errors(
        "\
record Sr layout little:
    flags : u8 with bits:
        wide : bool at 0..3

output n : int in 0..9 @ hw(\"o/n\") with safe = 0

machine m:
    var s : Sr = default
    initial RUN
    state RUN:
        loop:
            s.flags.wide = true
            n = 1
",
    );
    assert_eq!(e.lines().count(), 1, "{e}");
    assert!(e.contains("[SC-46]") && e.contains("`wide`"), "{e}");
}
